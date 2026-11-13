//! Raw media for fixed image blobs; text/content callers keep the JSON adapter.
use super::*;
#[cfg(test)]
mod tests;
use crate::network::transport::HttpResponse;

pub async fn fetch_image_blob(
    client: &GitHubClient,
    token: &Pat,
    identity: &BlobIdentity,
    limits: &BlobLimits,
    cancel: CancellationToken,
) -> Result<(BlobRead, RepositoryResponseMetadata), RepositoryError> {
    if cancel.is_cancelled() {
        return Err(GitHubError::Cancelled.into());
    }
    if let Some(size) = identity
        .size()
        .filter(|size| *size > limits.decoded_bytes as u64)
    {
        return Ok((
            too_large(identity, size, limits),
            RepositoryResponseMetadata::default(),
        ));
    }
    let url = endpoint(
        client,
        identity.repository().locator(),
        &["git", "blobs", identity.blob().as_str()],
    )?;
    let mut request = client.build_get(url.as_str(), token);
    request
        .headers
        .retain(|(name, _)| !name.eq_ignore_ascii_case("accept"));
    request = request.with_header("Accept", "application/vnd.github.raw+json");
    let response = match client
        .transport()
        .execute_bytes_bounded(request, cancel.clone(), limits.decoded_bytes)
        .await
    {
        Ok(response) => response,
        // Existing injected transports explicitly fall back to the bounded JSON
        // contract. A production binary transport never buffers a text fallback.
        Err(TransportError::BoundedReadUnsupported) => {
            let (read, metadata) =
                fetch_blob_with_metadata(client, token, identity, limits, None, cancel).await?;
            return Ok((
                read.ok_or_else(|| invalid_response("unexpected image blob 304"))?,
                metadata,
            ));
        }
        Err(TransportError::ResponseTooLarge { limit }) => {
            return Err(RepositoryError::LimitExceeded {
                resource: "raw image blob bytes",
                limit,
            });
        }
        Err(error) => return Err(classify_transport(error).into()),
    };
    if cancel.is_cancelled() {
        return Err(GitHubError::Cancelled.into());
    }
    if response.body.len() > limits.decoded_bytes {
        return Err(RepositoryError::LimitExceeded {
            resource: "raw image blob bytes",
            limit: limits.decoded_bytes,
        });
    }
    let metadata_response = HttpResponse {
        status: response.status,
        headers: response.headers,
        body: String::new(),
    };
    if matches!(metadata_response.status, 406 | 415) {
        let (read, metadata) =
            fetch_blob_with_metadata(client, token, identity, limits, None, cancel).await?;
        return Ok((
            read.ok_or_else(|| invalid_response("unexpected image blob 304"))?,
            metadata,
        ));
    }
    check_single(&metadata_response)?;
    let metadata = response_metadata(&metadata_response);
    if identity
        .size()
        .is_some_and(|size| size != response.body.len() as u64)
    {
        return Err(invalid_response(
            "raw image blob size differs from descriptor",
        ));
    }
    let mut hash = sha1_smol::Sha1::new();
    hash.update(format!("blob {}\0", response.body.len()).as_bytes());
    hash.update(&response.body);
    if hash.digest().to_string() != identity.blob().as_str() {
        return Err(invalid_response(
            "raw image blob SHA does not match request",
        ));
    }
    Ok((
        BlobRead {
            identity: identity.clone(),
            body: BlobBody::Bytes(response.body),
        },
        metadata,
    ))
}
