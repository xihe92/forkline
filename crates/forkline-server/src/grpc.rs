//! gRPC adapter: a generic bytes service, `forkline.v1.Plans`.
//!
//! Each configured method runs a plan with the raw request message as its
//! body and returns the reply body as the raw response message, so no
//! protobuf code generation is needed. Typed APIs can implement their own
//! tonic services and call [`Engine::call`] from the handlers.

use std::collections::{BTreeMap, HashMap};
use std::convert::Infallible;
use std::fmt;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;

use bytes::Bytes;
use forkline_core::node::BoxFuture;
use forkline_core::{Protocol, RequestEnv, Status as ReplyStatus};
use forkline_engine::{Engine, EngineError, Request};
use tokio::time::Instant;
use tonic::metadata::MetadataMap;

pub use forkline_services::BytesCodec;

use crate::status::grpc_code;

/// The gRPC service name.
pub const SERVICE_NAME: &str = "forkline.v1.Plans";

/// Parse a `grpc-timeout` header value (`<1-8 digits><H|M|S|m|u|n>`).
pub fn parse_grpc_timeout(value: &str) -> Option<Duration> {
    if value.len() < 2 || value.len() > 9 {
        return None;
    }
    let (digits, unit) = value.split_at(value.len() - 1);
    if !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let n: u64 = digits.parse().ok()?;
    Some(match unit {
        "H" => Duration::from_secs(n.checked_mul(3600)?),
        "M" => Duration::from_secs(n.checked_mul(60)?),
        "S" => Duration::from_secs(n),
        "m" => Duration::from_millis(n),
        "u" => Duration::from_micros(n),
        "n" => Duration::from_nanos(n),
        _ => return None,
    })
}

/// The `forkline.v1.Plans` service for one listener.
#[derive(Clone)]
pub struct GrpcPlans {
    engine: Arc<Engine>,
    methods: Arc<HashMap<String, Arc<str>>>,
}

impl GrpcPlans {
    /// Map `/forkline.v1.Plans/<method>` to plans.
    pub fn new(engine: Arc<Engine>, methods: &BTreeMap<String, String>) -> Self {
        let methods = methods.iter().map(|(m, p)| (format!("/{SERVICE_NAME}/{m}"), Arc::from(p.as_str()))).collect();
        Self { engine, methods: Arc::new(methods) }
    }
}

impl fmt::Debug for GrpcPlans {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("GrpcPlans").field("methods", &self.methods.keys()).finish()
    }
}

impl tonic::server::NamedService for GrpcPlans {
    const NAME: &'static str = SERVICE_NAME;
}

impl<B> tower::Service<http::Request<B>> for GrpcPlans
where
    B: http_body::Body + Send + 'static,
    B::Error: Into<Box<dyn std::error::Error + Send + Sync>> + Send + 'static,
{
    type Response = http::Response<tonic::body::Body>;
    type Error = Infallible;
    type Future = BoxFuture<'static, Result<Self::Response, Infallible>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Infallible>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, req: http::Request<B>) -> Self::Future {
        let path = req.uri().path();
        let Some(plan) = self.methods.get(path).cloned() else {
            let status = tonic::Status::unimplemented(format!("no plan is mapped to {path}"));
            return Box::pin(async move { Ok(status.into_http()) });
        };
        let unary = PlanUnary { engine: self.engine.clone(), plan, route: Arc::from(path) };
        Box::pin(async move {
            let mut grpc = tonic::server::Grpc::new(BytesCodec);
            Ok(grpc.unary(unary, req).await)
        })
    }
}

struct PlanUnary {
    engine: Arc<Engine>,
    plan: Arc<str>,
    route: Arc<str>,
}

impl tonic::server::UnaryService<Bytes> for PlanUnary {
    type Response = Bytes;
    type Future = BoxFuture<'static, Result<tonic::Response<Bytes>, tonic::Status>>;

    fn call(&mut self, request: tonic::Request<Bytes>) -> Self::Future {
        let received_at = Instant::now();
        let engine = self.engine.clone();
        let plan = self.plan.clone();
        let route = self.route.to_string();
        let peer = request.remote_addr();
        let (metadata, _extensions, body) = request.into_parts();
        let headers = metadata.into_headers();
        let inbound_deadline = headers
            .get("grpc-timeout")
            .and_then(|v| v.to_str().ok())
            .and_then(parse_grpc_timeout)
            .map(|t| received_at + t);
        let env =
            RequestEnv { protocol: Protocol::Grpc, peer, received_at, inbound_deadline, route, metadata: headers };
        Box::pin(async move {
            match engine.call(&plan, Request { body, env }).await {
                Ok(reply) => match reply.status {
                    ReplyStatus::Ok => {
                        let mut response = tonic::Response::new(reply.body);
                        *response.metadata_mut() = MetadataMap::from_headers(reply.headers);
                        Ok(response)
                    }
                    ReplyStatus::Error(kind) => Err(tonic::Status::with_metadata(
                        grpc_code(kind),
                        String::from_utf8_lossy(&reply.body).into_owned(),
                        MetadataMap::from_headers(reply.headers),
                    )),
                },
                Err(EngineError::UnknownPlan(plan)) => {
                    tracing::error!(%plan, "method names a plan that is no longer loaded");
                    Err(tonic::Status::unimplemented("plan not loaded"))
                }
                Err(e) => {
                    tracing::error!(error = %e, "request failed without a reply");
                    Err(tonic::Status::internal("internal error"))
                }
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grpc_timeouts_parse() {
        assert_eq!(parse_grpc_timeout("100m"), Some(Duration::from_millis(100)));
        assert_eq!(parse_grpc_timeout("2S"), Some(Duration::from_secs(2)));
        assert_eq!(parse_grpc_timeout("1H"), Some(Duration::from_secs(3600)));
        assert_eq!(parse_grpc_timeout("5u"), Some(Duration::from_micros(5)));
        assert_eq!(parse_grpc_timeout("m"), None);
        assert_eq!(parse_grpc_timeout("123456789m"), None, "at most 8 digits");
        assert_eq!(parse_grpc_timeout("10x"), None);
        assert_eq!(parse_grpc_timeout("-1m"), None);
    }
}
