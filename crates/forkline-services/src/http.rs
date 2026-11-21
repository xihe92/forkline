//! HTTP client service over reqwest.

use std::fmt;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;

use bytes::Bytes;
use forkline_core::Ctx;
use forkline_core::node::BoxFuture;
use http::{HeaderMap, HeaderName, HeaderValue, Method, StatusCode};
use serde::Deserialize;
use tower::util::BoxCloneSyncService;
use tower::{Service, ServiceExt};

use crate::error::{ServiceError, ServiceErrorKind};
use crate::stack::{StackConfig, layered};

/// An HTTP request relative to the service's base URL.
#[derive(Clone, Debug)]
pub struct HttpRequest {
    /// Method.
    pub method: Method,
    /// Path and query, relative to the base URL (a leading `/` is ignored).
    pub path: String,
    /// Headers.
    pub headers: HeaderMap,
    /// Body.
    pub body: Bytes,
}

impl HttpRequest {
    /// A GET request.
    pub fn get(path: impl Into<String>) -> Self {
        Self { method: Method::GET, path: path.into(), headers: HeaderMap::new(), body: Bytes::new() }
    }

    /// A POST request.
    pub fn post(path: impl Into<String>, body: impl Into<Bytes>) -> Self {
        Self { method: Method::POST, path: path.into(), headers: HeaderMap::new(), body: body.into() }
    }

    /// Add a header.
    pub fn header(mut self, name: HeaderName, value: HeaderValue) -> Self {
        self.headers.insert(name, value);
        self
    }
}

/// An HTTP response. 5xx statuses arrive as errors instead (see [`HttpClient`]).
#[derive(Clone, Debug)]
pub struct HttpResponse {
    /// Status.
    pub status: StatusCode,
    /// Headers.
    pub headers: HeaderMap,
    /// Body.
    pub body: Bytes,
}

impl HttpResponse {
    /// A 200 response with `body`.
    pub fn ok(body: impl Into<Bytes>) -> Self {
        Self { status: StatusCode::OK, headers: HeaderMap::new(), body: body.into() }
    }

    /// A response with `status` and `body`.
    pub fn with_status(status: StatusCode, body: impl Into<Bytes>) -> Self {
        Self { status, headers: HeaderMap::new(), body: body.into() }
    }

    /// The body as UTF-8 text.
    pub fn text(&self) -> Result<String, ServiceError> {
        String::from_utf8(self.body.to_vec())
            .map_err(|_| ServiceError::new(ServiceErrorKind::Other, "response body is not UTF-8"))
    }

    /// `Err` for any non-2xx status.
    pub fn error_for_status(self) -> Result<Self, ServiceError> {
        if self.status.is_success() {
            Ok(self)
        } else {
            Err(ServiceError::new(ServiceErrorKind::Status, format!("HTTP {}", self.status)))
        }
    }
}

/// Settings for `kind = "http"`.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HttpSettings {
    /// Base URL; request paths are joined onto it.
    pub url: String,
    /// TCP connect timeout, in milliseconds.
    pub connect_timeout_ms: Option<u64>,
    /// Idle connections kept per host.
    pub pool_max_idle_per_host: Option<usize>,
}

/// The reqwest transport: one shared connection pool per service.
#[derive(Clone, Debug)]
pub struct ReqwestTransport {
    client: reqwest::Client,
    base: reqwest::Url,
}

impl ReqwestTransport {
    /// Build from settings.
    pub fn new(settings: &HttpSettings) -> Result<Self, String> {
        let mut url = settings.url.clone();
        if !url.ends_with('/') {
            url.push('/');
        }
        let base = reqwest::Url::parse(&url).map_err(|e| format!("invalid url `{}`: {e}", settings.url))?;
        let mut builder = reqwest::Client::builder();
        if let Some(ms) = settings.connect_timeout_ms {
            builder = builder.connect_timeout(Duration::from_millis(ms));
        }
        if let Some(n) = settings.pool_max_idle_per_host {
            builder = builder.pool_max_idle_per_host(n);
        }
        let client = builder.build().map_err(|e| format!("cannot build HTTP client: {e}"))?;
        Ok(Self { client, base })
    }
}

impl Service<HttpRequest> for ReqwestTransport {
    type Response = HttpResponse;
    type Error = ServiceError;
    type Future = BoxFuture<'static, Result<HttpResponse, ServiceError>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), ServiceError>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, req: HttpRequest) -> Self::Future {
        let client = self.client.clone();
        let url = self.base.join(req.path.trim_start_matches('/'));
        Box::pin(async move {
            let url = url.map_err(|e| ServiceError::new(ServiceErrorKind::Other, format!("bad path: {e}")))?;
            let response = client
                .request(req.method, url)
                .headers(req.headers)
                .body(req.body)
                .send()
                .await
                .map_err(|e| ServiceError::new(ServiceErrorKind::Transport, e.to_string()))?;
            let status = response.status();
            let headers = response.headers().clone();
            let body =
                response.bytes().await.map_err(|e| ServiceError::new(ServiceErrorKind::Transport, e.to_string()))?;
            classify(HttpResponse { status, headers, body })
        })
    }
}

/// 502/503/504 become retryable `Unavailable` errors, other 5xx `Status` errors.
pub fn classify(response: HttpResponse) -> Result<HttpResponse, ServiceError> {
    let status = response.status;
    if !status.is_server_error() {
        return Ok(response);
    }
    let kind = match status {
        StatusCode::BAD_GATEWAY | StatusCode::SERVICE_UNAVAILABLE | StatusCode::GATEWAY_TIMEOUT => {
            ServiceErrorKind::Unavailable
        }
        _ => ServiceErrorKind::Status,
    };
    Err(ServiceError::new(kind, format!("HTTP {status}")))
}

fn retryable(result: &Result<HttpResponse, ServiceError>) -> bool {
    result.as_ref().err().is_some_and(ServiceError::is_retryable)
}

/// A named HTTP client with its layer stack. Cheap to clone.
#[derive(Clone)]
pub struct HttpClient {
    name: Arc<str>,
    svc: BoxCloneSyncService<HttpRequest, HttpResponse, ServiceError>,
}

impl HttpClient {
    /// A client over any service (for tests and custom transports), without layers.
    pub fn from_service<S>(name: &str, svc: S) -> Self
    where
        S: Service<HttpRequest, Response = HttpResponse, Error = ServiceError> + Clone + Send + Sync + 'static,
        S::Future: Send + 'static,
    {
        Self { name: Arc::from(name), svc: BoxCloneSyncService::new(svc) }
    }

    /// A client over `transport` wrapped in the layers `stack` asks for.
    pub fn layered<S>(name: &str, transport: S, stack: &StackConfig) -> Self
    where
        S: Service<HttpRequest, Response = HttpResponse, Error = ServiceError> + Clone + Send + Sync + 'static,
        S::Future: Send + 'static,
    {
        Self { name: Arc::from(name), svc: layered(transport, stack, retryable) }
    }

    /// A reqwest-backed client from settings.
    pub fn from_settings(name: &str, settings: &HttpSettings, stack: &StackConfig) -> Result<Self, String> {
        Ok(Self::layered(name, ReqwestTransport::new(settings)?, stack))
    }

    /// The service name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Send a request, bounded by the request's deadline and cancellation.
    pub async fn call(&self, cx: &Ctx, req: HttpRequest) -> Result<HttpResponse, ServiceError> {
        let call = tokio::time::timeout_at(cx.deadline(), self.svc.clone().oneshot(req));
        let result = tokio::select! {
            biased;
            () = cx.cancellation().cancelled() => Err(ServiceError::new(ServiceErrorKind::Cancelled, "request cancelled")),
            r = call => r.unwrap_or_else(|_| Err(ServiceError::new(ServiceErrorKind::Deadline, "request deadline passed"))),
        };
        result.map_err(|e| e.with_service(&self.name))
    }

    /// GET `path`.
    pub async fn get(&self, cx: &Ctx, path: impl Into<String>) -> Result<HttpResponse, ServiceError> {
        self.call(cx, HttpRequest::get(path)).await
    }

    /// POST `body` to `path`.
    pub async fn post(
        &self,
        cx: &Ctx,
        path: impl Into<String>,
        body: impl Into<Bytes>,
    ) -> Result<HttpResponse, ServiceError> {
        self.call(cx, HttpRequest::post(path, body)).await
    }
}

impl fmt::Debug for HttpClient {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HttpClient").field("name", &self.name).finish()
    }
}
