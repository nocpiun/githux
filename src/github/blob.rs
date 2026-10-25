//! Bounded JSON Git blob reads. Never request a branch or a download_url.

use crate::auth::token::Pat;
use crate::domain::GitHubError;
use crate::domain::blob::{BlobBody, BlobIdentity, BlobLimits, BlobRead};
use crate::domain::repository::{ObjectSha, RepositoryError};
use crate::github::client::GitHubClient;
use crate::github::error::classify_transport;
use crate::github::repository::{check_single, endpoint, invalid_response};
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
    if cancel.is_cancelled() {
        return Err(GitHubError::Cancelled.into());
    }
    if let Some(size) = identity
        .size()
        .filter(|size| *size > limits.decoded_bytes as u64)
    {
        return Ok(too_large(identity, size, limits));
    }
    let url = endpoint(
        client,
        identity.repository().locator(),
        &["git", "blobs", identity.blob().as_str()],
    )?;
    let response = client
        .transport()
        .execute_bounded(
            // Force JSON even if the generic client was configured for raw media.
            {
                let mut request = client.build_get(url.as_str(), token);
                request
                    .headers
                    .retain(|(name, _)| !name.eq_ignore_ascii_case("accept"));
                request.with_header("Accept", "application/vnd.github+json")
            },
            cancel.clone(),
            limits.response_bytes,
        )
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
        return Ok(too_large(identity, raw.size, limits));
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
        return Ok(too_large(identity, decoded_size as u64, limits));
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
    Ok(BlobRead {
        identity: identity.clone(),
        body: BlobBody::Bytes(bytes),
    })
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
