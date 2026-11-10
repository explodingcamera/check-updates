use std::collections::HashMap;

use super::CargoError;
use http::{Request, Response};
use tokio::task::JoinSet;

const MAX_CONCURRENT_FETCHES: usize = 16;

pub async fn fetch_all(
    client: &reqwest::Client,
    requests: Vec<(String, Request<()>)>,
) -> Result<HashMap<String, Response<Vec<u8>>>, CargoError> {
    let total = requests.len();
    let mut requests = requests.into_iter();
    let mut tasks = JoinSet::new();
    let mut results = HashMap::with_capacity(total);

    while results.len() < total {
        while tasks.len() < MAX_CONCURRENT_FETCHES {
            let Some((name, request)) = requests.next() else {
                break;
            };

            let client = client.clone();
            tasks.spawn(async move { (name, fetch_one(&client, request).await) });
        }

        if let Some(result) = tasks.join_next().await {
            let (name, response) = result.inspect_err(|e| log::warn!("fetch task failed: {e}"))?;
            let response =
                response.inspect_err(|e| log::warn!("failed to fetch index for '{name}': {e}"))?;

            results.insert(name, response);
        }
    }

    Ok(results)
}

async fn fetch_one(
    client: &reqwest::Client,
    request: Request<()>,
) -> Result<Response<Vec<u8>>, CargoError> {
    let resp = client
        .get(request.uri().to_string())
        .headers(request.headers().clone())
        .send()
        .await?;
    let mut builder = Response::builder().status(resp.status());
    if let Some(headers) = builder.headers_mut() {
        *headers = resp.headers().clone();
    }
    builder
        .body(resp.bytes().await?.to_vec())
        .map_err(CargoError::Http)
}
