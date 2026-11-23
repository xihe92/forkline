//! The admin listener: health, readiness, metrics, snapshot info, reload and
//! rollback. Bind it to a private address; it also checks an IP allowlist.

use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use axum::Router;
use axum::extract::{ConnectInfo, Request, State};
use axum::http::StatusCode;
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use forkline_engine::Engine;
use prometheus_client::registry::Registry as MetricsRegistry;
use serde_json::json;

/// Rebuilds the snapshot from the config files and swaps it in; returns the new version.
pub type Reloader = Arc<dyn Fn() -> Result<u64, String> + Send + Sync>;

#[derive(Clone)]
pub(crate) struct AdminState {
    pub(crate) engine: Arc<Engine>,
    pub(crate) reloader: Option<Reloader>,
    pub(crate) metrics: Option<Arc<MetricsRegistry>>,
    pub(crate) ready: Arc<AtomicBool>,
    pub(crate) allow: Arc<Vec<crate::allow::Network>>,
}

/// Parse allowlist entries (networks or single addresses). Validation has
/// already rejected bad ones; any that slip through are skipped.
pub(crate) fn parse_allow(entries: &[String]) -> Vec<crate::allow::Network> {
    entries.iter().filter_map(|e| e.parse().ok()).collect()
}

pub(crate) fn router(state: AdminState) -> Router {
    Router::new()
        .route("/healthz", get(|| async { "ok" }))
        .route("/readyz", get(readyz))
        .route("/metrics", get(metrics))
        .route("/admin/snapshot", get(snapshot))
        .route("/admin/reload", post(reload))
        .route("/admin/rollback", post(rollback))
        .layer(middleware::from_fn_with_state(state.clone(), allowlist))
        .with_state(state)
}

async fn allowlist(
    State(state): State<AdminState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    req: Request,
    next: Next,
) -> Response {
    let ip = peer.ip().to_canonical();
    if state.allow.iter().any(|net| net.contains(ip)) {
        next.run(req).await
    } else {
        tracing::warn!(%peer, "admin request from a disallowed address");
        (StatusCode::FORBIDDEN, "forbidden").into_response()
    }
}

async fn readyz(State(state): State<AdminState>) -> Response {
    if state.ready.load(Ordering::Acquire) {
        (StatusCode::OK, "ready").into_response()
    } else {
        (StatusCode::SERVICE_UNAVAILABLE, "shutting down").into_response()
    }
}

async fn metrics(State(state): State<AdminState>) -> Response {
    let Some(registry) = &state.metrics else {
        return (StatusCode::NOT_FOUND, "metrics are not enabled").into_response();
    };
    let mut body = String::new();
    match prometheus_client::encoding::text::encode(&mut body, registry) {
        Ok(()) => {
            ([(axum::http::header::CONTENT_TYPE, "application/openmetrics-text; version=1.0.0; charset=utf-8")], body)
                .into_response()
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, format!("cannot encode metrics: {e}")).into_response(),
    }
}

async fn snapshot(State(state): State<AdminState>) -> Response {
    let s = state.engine.snapshot();
    let services: Vec<_> = s.services.iter().map(|(name, ty)| json!({ "name": name, "type": ty })).collect();
    axum::Json(json!({
        "version": s.version,
        "config_hash": format!("{:016x}", s.config_hash),
        "plans": s.plans.keys().collect::<Vec<_>>(),
        "services": services,
        "history": state.engine.history(),
    }))
    .into_response()
}

async fn reload(State(state): State<AdminState>) -> Response {
    let Some(reloader) = state.reloader.clone() else {
        return (StatusCode::NOT_IMPLEMENTED, axum::Json(json!({ "error": "no reloader configured" }))).into_response();
    };
    match tokio::task::spawn_blocking(move || reloader()).await {
        Ok(Ok(version)) => axum::Json(json!({ "version": version })).into_response(),
        Ok(Err(e)) => {
            let errors: Vec<&str> = e.lines().collect();
            (StatusCode::BAD_REQUEST, axum::Json(json!({ "errors": errors }))).into_response()
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, axum::Json(json!({ "errors": [e.to_string()] }))).into_response(),
    }
}

async fn rollback(State(state): State<AdminState>) -> Response {
    match state.engine.rollback() {
        Some(version) => axum::Json(json!({ "version": version })).into_response(),
        None => (
            StatusCode::CONFLICT,
            axum::Json(json!({
                "error": "nothing to roll back to: no earlier snapshot, or it lacks a plan a listener serves"
            })),
        )
            .into_response(),
    }
}
