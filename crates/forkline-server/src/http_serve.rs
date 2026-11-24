//! The accept loop for the HTTP, gRPC and admin listeners: HTTP/1.1 and
//! HTTP/2 without TLS (h2c), or HTTP/2 only for gRPC, with a header-read
//! timeout, an idle timeout and graceful shutdown.
//!
//! Neither `axum::serve` nor tonic's server closes idle connections, and
//! `axum::serve` sets no header-read timeout (hyper's needs a timer it
//! doesn't configure), so a client could hold a connection open forever by
//! sending nothing, or one byte at a time.

use std::convert::Infallible;
use std::net::SocketAddr;
use std::time::Duration;

use axum::extract::ConnectInfo;
use bytes::Bytes;
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

/// Which HTTP versions a listener speaks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Versions {
    /// HTTP/1.1 and HTTP/2 with prior knowledge (h2c).
    Both,
    /// HTTP/2 only (gRPC).
    Http2,
}

/// Serve `service` until `shutdown`; then stop accepting, close idle
/// connections, and wait for the others to finish their requests. Dropping
/// the future (the server's cut-off) closes every connection at once.
/// Requests carry the peer address as axum's `ConnectInfo<SocketAddr>` and
/// as tonic's `TcpConnectInfo`.
pub(crate) async fn serve<S, B>(
    listener: TcpListener,
    service: S,
    versions: Versions,
    timeouts: Timeouts,
    shutdown: CancellationToken,
) where
    S: tower::Service<http::Request<Incoming>, Response = http::Response<B>, Error = Infallible>
        + Clone
        + Send
        + 'static,
    S::Future: Send + 'static,
    B: http_body::Body<Data = Bytes> + Send + 'static,
    B::Error: Into<Box<dyn std::error::Error + Send + Sync>>,
{
    let builder = builder(versions, timeouts);
    let mut connections = JoinSet::new();
    loop {
        tokio::select! {
            () = shutdown.cancelled() => break,
            Some(_) = connections.join_next(), if !connections.is_empty() => {}
            accepted = listener.accept() => match accepted {
                Ok((stream, peer)) => {
                    let _ = stream.set_nodelay(true);
                    let local = stream.local_addr().ok();
                    let (builder, service, stop) = (builder.clone(), service.clone(), shutdown.clone());
                    connections.spawn(connection(builder, stream, peer, local, service, timeouts.idle, stop));
                }
                Err(e) => accept_failed(&e).await,
            },
        }
    }
    drop(listener);
    while connections.join_next().await.is_some() {}
}

fn builder(versions: Versions, timeouts: Timeouts) -> auto::Builder<TokioExecutor> {
    let mut builder = auto::Builder::new(TokioExecutor::new());
    builder.http1().timer(TokioTimer::new()).header_read_timeout(timeouts.header_read);
    // A timer for HTTP/2 too: hyper needs one for any HTTP/2 timeout.
    builder.http2().timer(TokioTimer::new());
    if versions == Versions::Http2 { builder.http2_only() } else { builder }
}

/// Serve one connection over any byte stream, as the listener would serve
/// an accepted socket. For fuzzing.
pub(crate) async fn serve_stream<IO, S, B>(stream: IO, service: S, versions: Versions, timeouts: Timeouts)
where
    IO: tokio::io::AsyncRead + tokio::io::AsyncWrite + Send + Unpin + 'static,
    S: tower::Service<http::Request<Incoming>, Response = http::Response<B>, Error = Infallible>
        + Clone
        + Send
        + 'static,
    S::Future: Send + 'static,
    B: http_body::Body<Data = Bytes> + Send + 'static,
    B::Error: Into<Box<dyn std::error::Error + Send + Sync>>,
{
    let peer = SocketAddr::from(([127, 0, 0, 1], 0));
    let builder = builder(versions, timeouts);
    connection(builder, stream, peer, None, service, timeouts.idle, CancellationToken::new()).await;
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

async fn connection<IO, S, B>(
    builder: auto::Builder<TokioExecutor>,
    stream: IO,
    peer: SocketAddr,
    local: Option<SocketAddr>,
    inner: S,
    idle: Duration,
    shutdown: CancellationToken,
) where
    IO: tokio::io::AsyncRead + tokio::io::AsyncWrite + Send + Unpin + 'static,
    S: tower::Service<http::Request<Incoming>, Response = http::Response<B>, Error = Infallible>
        + Clone
        + Send
        + 'static,
    S::Future: Send + 'static,
    B: http_body::Body<Data = Bytes> + Send + 'static,
    B::Error: Into<Box<dyn std::error::Error + Send + Sync>>,
{
    let tcp_info = tonic::transport::server::TcpConnectInfo { local_addr: local, remote_addr: Some(peer) };
    // How many requests are in flight on this connection, and since when.
    let (activity, mut watch) = watch::channel((0usize, Instant::now()));
    let service = tower::service_fn(move |mut request: http::Request<Incoming>| {
        request.extensions_mut().insert(ConnectInfo(peer));
        request.extensions_mut().insert(tcp_info.clone());
        let busy = Busy::start(&activity);
        let inner = inner.clone();
        async move {
            let response = inner.oneshot(request).await;
            drop(busy);
            response
        }
    });
    let conn = builder.serve_connection(TokioIo::new(stream), TowerToHyperService::new(service));
    tokio::pin!(conn);
    let mut closing = false;
    // Once asked to close, a connection gets CLOSE_GRACE to do so with no
    // request in flight; then it is dropped. hyper leaves an HTTP/2
    // connection whose client never sent the preface waiting for it.
    let grace = tokio::time::sleep(CLOSE_GRACE);
    tokio::pin!(grace);
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
                grace.as_mut().reset(Instant::now() + CLOSE_GRACE);
            }
            () = idle_for(&mut watch, idle), if !closing => {
                tracing::debug!(%peer, "closing an idle http connection");
                conn.as_mut().graceful_shutdown();
                closing = true;
                grace.as_mut().reset(Instant::now() + CLOSE_GRACE);
            }
            () = grace.as_mut(), if closing => {
                if watch.borrow().0 == 0 {
                    tracing::debug!(%peer, "dropping an http connection that did not close");
                    return;
                }
                grace.as_mut().reset(Instant::now() + CLOSE_GRACE);
            }
        }
    }
}

/// How long a connection asked to close may take with no request in flight.
const CLOSE_GRACE: Duration = Duration::from_secs(1);

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
