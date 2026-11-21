//! gRPC client service passing raw message bytes, and the bytes codec the
//! server's adapter shares.

use std::fmt;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;

use bytes::{Buf, BufMut, Bytes};
use forkline_core::Ctx;
use forkline_core::node::BoxFuture;
use serde::Deserialize;
use tonic::codec::{Codec, DecodeBuf, Decoder, EncodeBuf, Encoder};
use tower::util::BoxCloneSyncService;
use tower::{Service, ServiceExt};

use crate::error::{ServiceError, ServiceErrorKind};
use crate::stack::{StackConfig, layered};
use crate::trace::CallTimings;

/// A codec that passes message bytes through untouched.
#[derive(Clone, Copy, Debug, Default)]
pub struct BytesCodec;

impl Codec for BytesCodec {
    type Encode = Bytes;
    type Decode = Bytes;
    type Encoder = BytesCodec;
    type Decoder = BytesCodec;

    fn encoder(&mut self) -> Self::Encoder {
        BytesCodec
    }

    fn decoder(&mut self) -> Self::Decoder {
        BytesCodec
    }
}

impl Encoder for BytesCodec {
    type Item = Bytes;
    type Error = tonic::Status;

    fn encode(&mut self, item: Bytes, dst: &mut EncodeBuf<'_>) -> Result<(), tonic::Status> {
        dst.put(item);
        Ok(())
    }
}

impl Decoder for BytesCodec {
    type Item = Bytes;
    type Error = tonic::Status;

    fn decode(&mut self, src: &mut DecodeBuf<'_>) -> Result<Option<Bytes>, tonic::Status> {
        let len = src.remaining();
        Ok(Some(src.copy_to_bytes(len)))
    }
}

/// One unary call: full method path (`/package.Service/Method`) and message bytes.
#[derive(Clone, Debug)]
pub struct GrpcRequest {
    /// Method path.
    pub path: String,
    /// Encoded request message.
    pub body: Bytes,
    /// Request metadata.
    pub metadata: http::HeaderMap,
}

/// Settings for `kind = "grpc"`.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GrpcSettings {
    /// Endpoint, e.g. `http://backend:50051`.
    pub url: String,
    /// TCP connect timeout, in milliseconds.
    pub connect_timeout_ms: Option<u64>,
}

/// The tonic transport over a lazily connected channel.
#[derive(Clone, Debug)]
pub struct TonicTransport {
    channel: tonic::transport::Channel,
}

impl TonicTransport {
    /// Build from settings. Connects on first use.
    pub fn new(settings: &GrpcSettings) -> Result<Self, String> {
        let mut endpoint = tonic::transport::Endpoint::from_shared(settings.url.clone())
            .map_err(|e| format!("invalid url `{}`: {e}", settings.url))?;
        if let Some(ms) = settings.connect_timeout_ms {
            endpoint = endpoint.connect_timeout(Duration::from_millis(ms));
        }
        Ok(Self { channel: endpoint.connect_lazy() })
    }
}

fn from_status(status: &tonic::Status) -> ServiceError {
    let kind = match status.code() {
        tonic::Code::Unavailable | tonic::Code::ResourceExhausted => ServiceErrorKind::Unavailable,
        tonic::Code::DeadlineExceeded => ServiceErrorKind::Timeout,
        tonic::Code::Cancelled => ServiceErrorKind::Cancelled,
        _ => ServiceErrorKind::Status,
    };
    ServiceError::new(kind, format!("{:?}: {}", status.code(), status.message()))
}

impl Service<GrpcRequest> for TonicTransport {
    type Response = Bytes;
    type Error = ServiceError;
    type Future = BoxFuture<'static, Result<Bytes, ServiceError>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), ServiceError>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, req: GrpcRequest) -> Self::Future {
        let channel = self.channel.clone();
        Box::pin(async move {
            let path = http::uri::PathAndQuery::try_from(req.path.as_str())
                .map_err(|e| ServiceError::new(ServiceErrorKind::Other, format!("bad method path: {e}")))?;
            let mut grpc = tonic::client::Grpc::new(channel);
            grpc.ready().await.map_err(|e| ServiceError::new(ServiceErrorKind::Transport, e.to_string()))?;
            let mut request = tonic::Request::new(req.body);
            *request.metadata_mut() = tonic::metadata::MetadataMap::from_headers(req.metadata);
            grpc.unary(request, path, BytesCodec).await.map(tonic::Response::into_inner).map_err(|s| from_status(&s))
        })
    }
}

fn retryable(result: &Result<Bytes, ServiceError>) -> bool {
    result.as_ref().err().is_some_and(ServiceError::is_retryable)
}

/// A named gRPC client with its layer stack. Cheap to clone.
#[derive(Clone)]
pub struct GrpcClient {
    name: Arc<str>,
    svc: BoxCloneSyncService<GrpcRequest, Bytes, ServiceError>,
    timings: CallTimings,
}

impl GrpcClient {
    /// A client over any service, without layers.
    pub fn from_service<S>(name: &str, svc: S) -> Self
    where
        S: Service<GrpcRequest, Response = Bytes, Error = ServiceError> + Clone + Send + Sync + 'static,
        S::Future: Send + 'static,
    {
        Self { name: Arc::from(name), svc: BoxCloneSyncService::new(svc), timings: CallTimings::new(name) }
    }

    /// A tonic-backed client from settings.
    pub fn from_settings(name: &str, settings: &GrpcSettings, stack: &StackConfig) -> Result<Self, String> {
        Ok(Self {
            name: Arc::from(name),
            svc: layered(TonicTransport::new(settings)?, stack, retryable),
            timings: CallTimings::new(name),
        })
    }

    /// The service name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Call `path` with `body`, bounded by the request's deadline and
    /// cancellation. The inbound `traceparent` and `tracestate` are forwarded.
    pub async fn call(&self, cx: &Ctx, path: impl Into<String>, body: impl Into<Bytes>) -> Result<Bytes, ServiceError> {
        let mut metadata = http::HeaderMap::new();
        crate::trace::forward(cx, &mut metadata);
        let req = GrpcRequest { path: path.into(), body: body.into(), metadata };
        let started = std::time::Instant::now();
        let call = tokio::time::timeout_at(cx.deadline(), self.svc.clone().oneshot(req));
        let result = tokio::select! {
            biased;
            () = cx.cancellation().cancelled() => Err(ServiceError::new(ServiceErrorKind::Cancelled, "request cancelled")),
            r = call => r.unwrap_or_else(|_| Err(ServiceError::new(ServiceErrorKind::Deadline, "request deadline passed"))),
        };
        self.timings.record(result.is_ok(), started.elapsed());
        result.map_err(|e| e.with_service(&self.name))
    }
}

impl fmt::Debug for GrpcClient {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("GrpcClient").field("name", &self.name).finish()
    }
}
