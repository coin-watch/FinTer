//! HTTP client implementation for GPUI image loading.

use anyhow::Result;
use gpui::http_client::http::HeaderValue;
use gpui::http_client::{AsyncBody, HttpClient, Request, Response, Url};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use crate::data::client::tokio_runtime;

/// A simple HTTP client implementation using reqwest.
///
/// This wraps reqwest and ensures all HTTP calls run on our Tokio runtime,
/// since GPUI uses its own async executor that is not Tokio-compatible.
pub struct ReqwestHttpClient {
    client: reqwest::Client,
}

impl ReqwestHttpClient {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            client: reqwest::Client::builder()
                .user_agent("FinTer/0.1.4")
                .build()
                .expect("Failed to create reqwest client"),
        })
    }
}

impl HttpClient for ReqwestHttpClient {
    fn type_name(&self) -> &'static str {
        "ReqwestHttpClient"
    }

    fn user_agent(&self) -> Option<&HeaderValue> {
        None
    }

    fn proxy(&self) -> Option<&Url> {
        None
    }

    fn send(
        &self,
        req: Request<AsyncBody>,
    ) -> Pin<Box<dyn Future<Output = Result<Response<AsyncBody>>> + Send + 'static>> {
        let client = self.client.clone();

        // Spawn the HTTP request on our Tokio runtime since GPUI's executor
        // is not Tokio-compatible and reqwest/hyper require Tokio.
        let handle = tokio_runtime().handle().clone();

        Box::pin(async move {
            // Run the actual HTTP work on Tokio runtime
            handle
                .spawn(async move {
                    // Convert GPUI request to reqwest request
                    let method = req.method().clone();
                    let uri = req.uri().to_string();

                    let mut reqwest_req = client.request(method, &uri);

                    // Copy headers
                    for (name, value) in req.headers() {
                        reqwest_req = reqwest_req.header(name, value);
                    }

                    // Send request
                    let response = reqwest_req
                        .send()
                        .await
                        .map_err(|e| anyhow::anyhow!("HTTP request failed: {}", e))?;

                    // Convert reqwest response to GPUI response
                    let status = response.status();
                    let mut builder = gpui::http_client::http::Response::builder().status(status);

                    // Copy headers
                    for (name, value) in response.headers() {
                        builder = builder.header(name, value);
                    }

                    // Get body
                    let body_bytes = response
                        .bytes()
                        .await
                        .map_err(|e| anyhow::anyhow!("Failed to read response body: {}", e))?;

                    let body = AsyncBody::from(body_bytes.to_vec());

                    builder
                        .body(body)
                        .map_err(|e| anyhow::anyhow!("Failed to build response: {}", e))
                })
                .await
                .map_err(|e| anyhow::anyhow!("Tokio task failed: {}", e))?
        })
    }
}
