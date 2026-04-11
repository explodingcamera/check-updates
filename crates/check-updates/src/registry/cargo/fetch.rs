use std::collections::HashMap;

use crate::registry::cargo::CargoError;
use http::{Request, Response, StatusCode};

pub async fn fetch_all(
    client: &reqwest::Client,
    requests: Vec<(String, Request<()>)>,
) -> HashMap<String, Result<Response<Vec<u8>>, CargoError>> {
    let mut results = HashMap::with_capacity(requests.len());
    for (name, request) in requests {
        let response = fetch_one(client, request).await;
        results.insert(name, response);
    }

    results
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
