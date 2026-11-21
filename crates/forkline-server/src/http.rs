//! HTTP adapter: each configured path runs a plan on POST.

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::sync::Arc;

use axum::Router;
use axum::extract::ConnectInfo;
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use bytes::Bytes;
use forkline_core::{Protocol, Reply, RequestEnv, Status};
use forkline_engine::{Engine, EngineError, Request};
use http::{HeaderMap, StatusCode};
use tokio::time::Instant;

use crate::status::http_status;

/// Build the router for one listener. Serve it with
/// `into_make_service_with_connect_info::<SocketAddr>()`.
pub fn router(engine: Arc<Engine>, routes: &BTreeMap<String, String>) -> Router {
    let mut router = Router::new();
    for (path, plan) in routes {
        let engine = engine.clone();
        let plan: Arc<str> = Arc::from(plan.as_str());
        let route: Arc<str> = Arc::from(path.as_str());
        router = router.route(
            path,
            post(move |ConnectInfo(peer): ConnectInfo<SocketAddr>, headers: HeaderMap, body: Bytes| {
                let received_at = Instant::now();
                let env = RequestEnv {
                    protocol: Protocol::Http,
                    peer: Some(peer),
                    received_at,
                    inbound_deadline: None,
                    route: route.to_string(),
                    metadata: headers,
                };
                async move { to_response(engine.call(&plan, Request { body, env }).await) }
            }),
        );
    }
    router
}

fn to_response(result: Result<Reply, EngineError>) -> Response {
    match result {
        Ok(reply) => {
            let status = match reply.status {
                Status::Ok => StatusCode::OK,
                Status::Error(kind) => http_status(kind),
            };
            let mut response = (status, reply.body).into_response();
            response.headers_mut().extend(reply.headers);
            response
        }
        Err(EngineError::UnknownPlan(plan)) => {
            tracing::error!(%plan, "route names a plan that is no longer loaded");
            (StatusCode::NOT_FOUND, "plan not loaded").into_response()
        }
        Err(e) => {
            tracing::error!(error = %e, "request failed without a reply");
            (StatusCode::INTERNAL_SERVER_ERROR, "internal error").into_response()
        }
    }
}
