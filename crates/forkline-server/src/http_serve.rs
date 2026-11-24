//! The accept loop for the HTTP and admin listeners: HTTP/1.1 and HTTP/2
//! without TLS (h2c), with a header-read timeout, an idle timeout and
//! graceful shutdown.
//!
//! `axum::serve` sets neither timeout (hyper's header-read timeout needs a
//! timer it doesn't configure), so a client could hold a connection open
//! forever by sending nothing, or one byte at a time.

use std::net::SocketAddr;
use std::time::Duration;

use axum::Router;
use axum::extract::ConnectInfo;
use hyper::body::Incoming;
use hyper_util::rt::{TokioExecutor, TokioIo, TokioTimer};
use hyper_util::server::conn::auto;
use hyper_util::service::TowerToHyperService;
use tokio::net::TcpListener;
use tokio::sync::watch;
use tokio::task::JoinSet;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;
use tower::ServiceExt;

/// Per-connection limits.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Timeouts {
    /// HTTP/1: time to receive a request's complete headers, counted from
    /// when the server starts waiting for them (a new connection, or the end
    /// of the previous response on a kept-alive one).
    pub header_read: Duration,
    /// Close a connection with no request in flight for this long.
    pub idle: Duration,
}

/// Serve `router` until `shutdown`; then stop accepting, close idle
/// connections, and wait for the others to finish their requests. Dropping
/// the future (the server's cut-off) closes every connection at once.
pub(crate) async fn serve(listener: TcpListener, router: Router, timeouts: Timeouts, shutdown: CancellationToken) {
    let mut builder = auto::Builder::new(TokioExecutor::new());
    builder.http1().timer(TokioTimer::new()).header_read_timeout(timeouts.header_read);
    let mut connections = JoinSet::new();
    loop {
        tokio::select! {
            () = shutdown.cancelled() => break,
            Some(_) = connections.join_next(), if !connections.is_empty() => {}
            accepted = listener.accept() => match accepted {
                Ok((stream, peer)) => {
                    let _ = stream.set_nodelay(true);
                    let (builder, router, stop) = (builder.clone(), router.clone(), shutdown.clone());
                    connections.spawn(connection(builder, stream, peer, router, timeouts.idle, stop));
                }
                Err(e) => accept_failed(&e).await,
            },
        }
    }
    drop(listener);
    while connections.join_next().await.is_some() {}
}

/// Out of file descriptors and the like: wait before trying again instead of
/// spinning. A client that went away before we accepted it is not an error.
async fn accept_failed(e: &std::io::Error) {
    use std::io::ErrorKind::{ConnectionAborted, ConnectionRefused, ConnectionReset};
    if !matches!(e.kind(), ConnectionAborted | ConnectionRefused | ConnectionReset) {
        tracing::warn!(error = %e, "http accept failed; retrying in 1 s");
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}

async fn connection(
    builder: auto::Builder<TokioExecutor>,
    stream: tokio::net::TcpStream,
    peer: SocketAddr,
    router: Router,
    idle: Duration,
    shutdown: CancellationToken,
) {
    // How many requests are in flight on this connection, and since when.
    let (activity, mut watch) = watch::channel((0usize, Instant::now()));
    let service = tower::service_fn(move |mut request: http::Request<Incoming>| {
        request.extensions_mut().insert(ConnectInfo(peer));
        let busy = Busy::start(&activity);
        let router = router.clone();
        async move {
            let response = router.oneshot(request).await;
            drop(busy);
            response
        }
    });
    let conn = builder.serve_connection_with_upgrades(TokioIo::new(stream), TowerToHyperService::new(service));
    tokio::pin!(conn);
    let mut closing = false;
    loop {
        tokio::select! {
            result = conn.as_mut() => {
                if let Err(e) = result {
                    tracing::debug!(error = %e, %peer, "http connection ended with an error");
                }
                return;
            }
            () = shutdown.cancelled(), if !closing => {
                conn.as_mut().graceful_shutdown();
                closing = true;
            }
            () = idle_for(&mut watch, idle), if !closing => {
                tracing::debug!(%peer, "closing an idle http connection");
                conn.as_mut().graceful_shutdown();
                closing = true;
            }
        }
    }
}

/// Resolves once no request has been in flight for `idle`.
async fn idle_for(watch: &mut watch::Receiver<(usize, Instant)>, idle: Duration) {
    loop {
        let (in_flight, since) = *watch.borrow_and_update();
        if in_flight > 0 {
            if watch.changed().await.is_err() {
                std::future::pending::<()>().await;
            }
            continue;
        }
        tokio::select! {
            () = tokio::time::sleep_until(since + idle) => return,
            changed = watch.changed() => if changed.is_err() { std::future::pending::<()>().await },
        }
    }
}

/// One request in flight on a connection; dropping it (when the request ends
/// or is cancelled) restarts the idle clock once none are left.
struct Busy(watch::Sender<(usize, Instant)>);

impl Busy {
    fn start(activity: &watch::Sender<(usize, Instant)>) -> Self {
        activity.send_modify(|(n, since)| {
            *n += 1;
            *since = Instant::now();
        });
        Self(activity.clone())
    }
}

impl Drop for Busy {
    fn drop(&mut self) {
        self.0.send_modify(|(n, since)| {
            *n = n.saturating_sub(1);
            *since = Instant::now();
        });
    }
}
