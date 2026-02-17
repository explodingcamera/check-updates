use std::{collections::HashMap, time::Duration};

use curl::{
    easy::{Easy2, Handler, HttpVersion, List, WriteError},
    multi::Multi,
};
use http::{Request, Response, StatusCode};

use crate::registry::cargo::CargoError;

struct ResponseCollector {
    body: Vec<u8>,
}

impl Handler for ResponseCollector {
    fn write(&mut self, data: &[u8]) -> Result<usize, WriteError> {
        self.body.extend_from_slice(data);
        Ok(data.len())
    }
}

fn setup_easy(easy: &mut Easy2<ResponseCollector>, request: Request<()>) -> Result<(), CargoError> {
    let url = request.uri().to_string();
    easy.url(&url)?;
    easy.http_version(HttpVersion::V2)?;

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
        let mut easy = Easy2::new(ResponseCollector { body: Vec::new() });
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

    while multi.perform().unwrap_or(0) > 0 {
        multi.wait(&mut [], Duration::from_millis(100)).ok();
    }

    for (handle, name) in handles {
        let mut transfer_err: Option<curl::Error> = None;
        multi.messages(|msg| {
            if msg.is_for2(&handle)
                && let Some(result) = msg.result_for2(&handle)
                && let Err(e) = result
            {
                transfer_err = Some(e);
            }
        });

        if let Some(e) = transfer_err {
            results.insert(name, Err(e.into()));
            // still need to remove the handle to avoid leaking it
            multi.remove2(handle).ok();
            continue;
        }

        let response = match multi.remove2(handle) {
            Ok(easy) => {
                let code = match easy.response_code() {
                    Ok(c) => c,
                    Err(e) => {
                        results.insert(name, Err(e.into()));
                        continue;
                    }
                };
                let status = match StatusCode::from_u16(code as u16) {
                    Ok(s) => s,
                    Err(e) => {
                        results.insert(name, Err(CargoError::Http(e.into())));
                        continue;
                    }
                };
                Response::builder()
                    .status(status)
                    .body(easy.get_ref().body.clone())
                    .map_err(CargoError::Http)
            }
            Err(e) => Err(e.into()),
        };
        results.insert(name, response);
    }

    results
}
