use std::time::Duration;

use thiserror::Error;

const MAX_METADATA_BYTES: usize = 32 * 1024 * 1024;

/// An HTTP body could not be read safely.
#[derive(Debug, Error)]
pub(crate) enum BodyError {
    #[error(transparent)]
    Request(#[from] reqwest::Error),
    #[error("registry metadata exceeds 32 MiB")]
    TooLarge,
}

/// Common client defaults, leaving protocol and rate policies to each backend.
pub(crate) fn client_builder() -> reqwest::ClientBuilder {
    reqwest::Client::builder()
        .user_agent(crate::USER_AGENT)
        .timeout(Duration::from_secs(30))
        .http2_adaptive_window(true)
}

/// Read metadata with a limit on decompressed bytes, including chunked responses.
pub(crate) async fn read_body(response: &mut reqwest::Response) -> Result<Vec<u8>, BodyError> {
    if response
        .content_length()
        .is_some_and(|length| length > MAX_METADATA_BYTES as u64)
    {
        return Err(BodyError::TooLarge);
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        if chunk.len() > MAX_METADATA_BYTES - bytes.len() {
            return Err(BodyError::TooLarge);
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}
