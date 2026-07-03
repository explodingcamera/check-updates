use std::collections::HashMap;

use crate::registry::cargo::CargoError;
use http::{Request, Response, StatusCode};
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
            let (name, response) = result.map_err(|e| {
                log::warn!("fetch task failed: {e}");
                CargoError::from(e)
            })?;
            let response = response.map_err(|e| {
                log::warn!("failed to fetch index for '{name}': {e}");
                e
            })?;

            results.insert(name, response);
        }
    }

    Ok(results)
}

async fn fetch_one(
    client: &reqwest::Client,
    request: Request<()>,
) -> Result<Response<Vec<u8>>, CargoError> {
    let mut req = client.get(request.uri().to_string());
    for (name, value) in request.headers() {
        req = req.header(name, value);
    }

    let resp = req.send().await?;
    let status =
        StatusCode::from_u16(resp.status().as_u16()).map_err(|e| CargoError::Http(e.into()))?;
    let mut builder = Response::builder().status(status);
    if let Some(headers) = builder.headers_mut() {
        for (name, value) in resp.headers() {
            headers.append(name, value.clone());
        }
    }
    let bytes = resp.bytes().await?;
    builder.body(bytes.to_vec()).map_err(CargoError::Http)
}
