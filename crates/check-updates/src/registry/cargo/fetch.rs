use std::{collections::HashMap, time::Duration};

use curl::{
    easy::{Easy2, Handler, HttpVersion, List, WriteError},
    multi::Multi,
};
use http::{
    Request, Response, StatusCode,
    header::{HeaderMap, HeaderName, HeaderValue},
};

use crate::registry::cargo::CargoError;

const MULTI_WAIT_TIMEOUT: Duration = Duration::from_millis(20);

#[derive(Default)]
struct ResponseCollector {
    body: Vec<u8>,
    headers: HeaderMap,
}

impl Handler for ResponseCollector {
    fn write(&mut self, data: &[u8]) -> Result<usize, WriteError> {
        self.body.extend_from_slice(data);
        Ok(data.len())
    }

    fn header(&mut self, data: &[u8]) -> bool {
        if data.starts_with(b"HTTP/") {
            self.headers = HeaderMap::new();
            return true;
        }

        if data == b"\r\n" {
            return true;
        }

        let Some(idx) = data.iter().position(|b| *b == b':') else {
            return true;
        };

        let name = data[..idx].trim_ascii();
        let value = data[idx + 1..].trim_ascii();

        let Ok(name) = HeaderName::from_bytes(name) else {
            return true;
        };
        let Ok(value) = HeaderValue::from_bytes(value) else {
            return true;
        };

        self.headers.append(name, value);
        true
    }
}

fn setup_easy(easy: &mut Easy2<ResponseCollector>, request: Request<()>) -> Result<(), CargoError> {
    let url = request.uri().to_string();
    easy.url(&url)?;
    easy.http_version(HttpVersion::V2)?;
    easy.accept_encoding("")?;

    let mut headers = List::new();
    for (name, value) in request.headers() {
        let value_str = value.to_str().unwrap_or_default();
        let header = format!("{}: {value_str}", name);
        headers.append(&header)?;
    }
    easy.http_headers(headers)?;

    Ok(())
}

pub fn fetch_all(
    multi: &Multi,
    requests: Vec<(String, Request<()>)>,
) -> HashMap<String, Result<Response<Vec<u8>>, CargoError>> {
    let mut results = HashMap::new();
    let mut handles = Vec::new();

    for (name, request) in requests {
        let mut easy = Easy2::new(ResponseCollector::default());
        if let Err(e) = setup_easy(&mut easy, request) {
            results.insert(name, Err(e));
            continue;
        }

        match multi.add2(easy) {
            Ok(handle) => handles.push((handle, name)),
            Err(e) => {
                results.insert(name, Err(e.into()));
            }
        }
    }

    let mut multi_loop_failed = false;
    loop {
        match multi.perform() {
            Ok(0) => break,
            Ok(_) => {}
            Err(e) => {
                multi_loop_failed = true;
                log::warn!("cargo.fetch_all: multi.perform failed: {e}");
                break;
            }
        }

        if let Err(e) = multi.wait(&mut [], MULTI_WAIT_TIMEOUT) {
            multi_loop_failed = true;
            log::warn!("cargo.fetch_all: multi.wait failed: {e}");
            break;
        }
    }

    if multi_loop_failed {
        for (handle, name) in handles {
            let _ = multi.remove2(handle);
            results.insert(
                name,
                Err(CargoError::Metadata(
                    "curl multi loop failed while fetching crate index".to_string(),
                )),
            );
        }
        return results;
    }

    for (handle, name) in handles {
        let response = multi
            .remove2(handle)
            .map_err(CargoError::from)
            .and_then(|mut easy| {
                let code = easy.response_code()?;
                let status =
                    StatusCode::from_u16(code as u16).map_err(|e| CargoError::Http(e.into()))?;

                let collector = easy.get_mut();
                let mut response = Response::builder().status(status);
                if let Some(headers) = response.headers_mut() {
                    headers.extend(std::mem::take(&mut collector.headers));
                }

                response
                    .body(std::mem::take(&mut collector.body))
                    .map_err(CargoError::Http)
            });

        results.insert(name, response);
    }

    results
}
