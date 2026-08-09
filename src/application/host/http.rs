use super::AppState;
use crate::application::bindings;
use lockgate::HostContext;
use reqwest::{header::HeaderMap, redirect::Policy};
use std::time::Duration;

const MAX_RESPONSE_BYTES: usize = 2 * 1024 * 1024;

#[derive(Clone)]
pub(super) struct Client {
    inner: reqwest::Client,
}

struct Response {
    status: u16,
    body: String,
}

impl Client {
    pub(super) fn new() -> Result<Self, String> {
        let inner = reqwest::Client::builder()
            .redirect(Policy::none())
            .timeout(Duration::from_secs(10 * 60))
            .build()
            .map_err(|error| format!("failed to build the provider HTTP client: {error}"))?;
        Ok(Self { inner })
    }

    async fn post(
        &self,
        url: String,
        headers: Vec<(String, String)>,
        body: String,
    ) -> Result<Response, String> {
        let headers = headers
            .into_iter()
            .map(|(name, value)| {
                let name = name
                    .parse::<reqwest::header::HeaderName>()
                    .map_err(|error| format!("invalid HTTP header name: {error}"))?;
                let value = value
                    .parse::<reqwest::header::HeaderValue>()
                    .map_err(|error| format!("invalid HTTP header value: {error}"))?;
                Ok((name, value))
            })
            .collect::<Result<HeaderMap, String>>()?;
        let response = self
            .inner
            .post(url)
            .headers(headers)
            .body(body)
            .send()
            .await
            .map_err(|error| format!("provider HTTP request failed: {error}"))?;
        if response
            .content_length()
            .is_some_and(|length| length > MAX_RESPONSE_BYTES as u64)
        {
            return Err(format!(
                "provider response exceeded the {MAX_RESPONSE_BYTES}-byte limit"
            ));
        }
        let status = response.status().as_u16();
        let body = response
            .bytes()
            .await
            .map_err(|error| format!("failed to read provider response: {error}"))?;
        if body.len() > MAX_RESPONSE_BYTES {
            return Err(format!(
                "provider response exceeded the {MAX_RESPONSE_BYTES}-byte limit"
            ));
        }
        let body = String::from_utf8(body.to_vec())
            .map_err(|_| "provider response was not UTF-8".to_owned())?;
        Ok(Response { status, body })
    }
}

impl bindings::sage::agent::http_client::Host for HostContext<AppState> {}

impl bindings::sage::agent::http_client::HostWithStore<lockgate::__private::PluginStore<AppState>>
    for HostContext<AppState>
{
    async fn post(
        accessor: &wasmtime::component::Accessor<lockgate::__private::PluginStore<AppState>, Self>,
        url: String,
        headers: Vec<bindings::sage::agent::http_client::Header>,
        body: String,
    ) -> Result<bindings::sage::agent::http_client::Response, String> {
        let http = accessor.with(|mut access| {
            let context = access.get();
            context.state().http.clone()
        });
        let response = http
            .post(
                url,
                headers
                    .into_iter()
                    .map(|header| (header.name, header.value))
                    .collect(),
                body,
            )
            .await?;
        Ok(bindings::sage::agent::http_client::Response {
            status: response.status,
            body: response.body,
        })
    }
}
