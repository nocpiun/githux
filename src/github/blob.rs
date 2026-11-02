//! Bounded JSON Git blob reads. Never request a branch or a download_url.

use crate::auth::token::Pat;
use crate::domain::GitHubError;
use crate::domain::blob::{BlobBody, BlobIdentity, BlobLimits, BlobRead};
use crate::domain::repository::{ObjectSha, RepositoryError, RepositoryResponseMetadata};
use crate::github::client::GitHubClient;
use crate::github::error::classify_transport;
use crate::github::repository::{check_single, endpoint, invalid_response, response_metadata};
use crate::network::cancel::CancellationToken;
use crate::network::transport::TransportError;
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use serde::Deserialize;

#[derive(Deserialize)]
struct BlobResponse {
    sha: String,
    encoding: String,
    size: u64,
    content: String,
}

pub async fn fetch_blob(
    client: &GitHubClient,
    token: &Pat,
    identity: &BlobIdentity,
    limits: &BlobLimits,
    cancel: CancellationToken,
) -> Result<BlobRead, RepositoryError> {
    fetch_blob_with_metadata(client, token, identity, limits, None, cancel)
        .await
        .map(|(blob, _)| blob.expect("unconditional read cannot be 304"))
}

pub async fn fetch_blob_with_metadata(
    client: &GitHubClient,
    token: &Pat,
    identity: &BlobIdentity,
    limits: &BlobLimits,
    conditional: Option<&RepositoryResponseMetadata>,
    cancel: CancellationToken,
) -> Result<(Option<BlobRead>, RepositoryResponseMetadata), RepositoryError> {
    let local_metadata = RepositoryResponseMetadata::default();
    if cancel.is_cancelled() {
        return Err(GitHubError::Cancelled.into());
    }
    if let Some(size) = identity
        .size()
        .filter(|size| *size > limits.decoded_bytes as u64)
    {
        return Ok((Some(too_large(identity, size, limits)), local_metadata));
    }
    let url = endpoint(
        client,
        identity.repository().locator(),
        &["git", "blobs", identity.blob().as_str()],
    )?;
    // Force JSON even if the generic client was configured for raw media.
    let mut request = client.build_get(url.as_str(), token);
    request
        .headers
        .retain(|(name, _)| !name.eq_ignore_ascii_case("accept"));
    request = request.with_header("Accept", "application/vnd.github+json");
    if let Some(metadata) = conditional {
        if let Some(etag) = &metadata.etag {
            request = request.with_header("If-None-Match", etag);
        }
        if let Some(last_modified) = &metadata.last_modified {
            request = request.with_header("If-Modified-Since", last_modified);
        }
    }
    let response = client
        .transport()
        .execute_bounded(request, cancel.clone(), limits.response_bytes)
        .await
        .map_err(|error| match error {
            TransportError::ResponseTooLarge { limit } => RepositoryError::LimitExceeded {
                resource: "blob JSON bytes",
                limit,
            },
            error => classify_transport(error).into(),
        })?;
    if cancel.is_cancelled() {
        return Err(GitHubError::Cancelled.into());
    }
    // Defense for custom transports as well as the real receive-time bound.
    if response.body.len() > limits.response_bytes {
        return Err(RepositoryError::LimitExceeded {
            resource: "blob JSON bytes",
            limit: limits.response_bytes,
        });
    }
    let metadata = response_metadata(&response);
    if response.status == 304 {
        return Ok((None, metadata));
    }
    check_single(&response)?;
    // Don't copy untrusted field values into parse errors (they may echo secrets).
    let mut raw: BlobResponse =
        serde_json::from_str(&response.body).map_err(|_| invalid_response("invalid blob JSON"))?;
    drop(response);
    if ObjectSha::new(&raw.sha).ok().as_ref() != Some(identity.blob()) {
        return Err(invalid_response("blob SHA does not match request"));
    }
    if raw.encoding != "base64" {
        return Err(invalid_response("unsupported blob API encoding"));
    }
    if identity.size().is_some_and(|size| size != raw.size) {
        return Err(invalid_response("blob size differs from descriptor"));
    }
    if raw.size > limits.decoded_bytes as u64 {
        return Ok((Some(too_large(identity, raw.size, limits)), metadata));
    }
    // GitHub wraps base64 with newlines. Compact in-place, avoiding an extra
    // full encoded copy. Reject all other whitespace through the strict decoder.
    raw.content.retain(|c| c != '\r' && c != '\n');
    let encoded = raw.content.as_bytes();
    if !encoded.len().is_multiple_of(4) {
        return Err(invalid_response("invalid blob base64"));
    }
    let padding = encoded.iter().rev().take_while(|b| **b == b'=').count();
    if padding > 2 {
        return Err(invalid_response("invalid blob base64"));
    }
    let decoded_size = (encoded.len() / 4)
        .saturating_mul(3)
        .saturating_sub(padding);
    if decoded_size > limits.decoded_bytes {
        return Ok((
            Some(too_large(identity, decoded_size as u64, limits)),
            metadata,
        ));
    }
    let mut bytes = vec![0; decoded_size];
    let written = STANDARD
        .decode_slice(encoded, &mut bytes)
        .map_err(|_| invalid_response("invalid blob base64"))?;
    if written as u64 != raw.size {
        return Err(invalid_response(
            "decoded blob size does not match response",
        ));
    }
    bytes.truncate(written);
    if cancel.is_cancelled() {
        return Err(GitHubError::Cancelled.into());
    }
    Ok((
        Some(BlobRead {
            identity: identity.clone(),
            body: BlobBody::Bytes(bytes),
        }),
        metadata,
    ))
}

fn too_large(identity: &BlobIdentity, size: u64, limits: &BlobLimits) -> BlobRead {
    BlobRead {
        identity: identity.clone(),
        body: BlobBody::TooLarge {
            size,
            limit: limits.decoded_bytes,
        },
    }
}

#[cfg(test)]
mod tests;
