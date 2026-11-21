//! Length-delimited TCP adapter.
//!
//! Each frame (4-byte big-endian length, then the body) is one request to
//! the listener's plan. Requests on one connection may be pipelined; their
//! responses come back in request order. A response frame is one status byte
//! and the reply body:
//!
//! | byte | meaning |
//! | --- | --- |
//! | 0 | ok |
//! | 1 | bad request |
//! | 2 | failed (module error, panic, merge conflict, internal) |
//! | 3 | deadline |
//! | 4 | overloaded: refused by admission control or shed |
//!
//! A frame larger than `max_frame_bytes` ends the connection after the
//! responses already owed are sent.

use std::net::SocketAddr;
use std::sync::Arc;

use bytes::{BufMut, Bytes, BytesMut};
use forkline_core::{ErrorKind, Protocol, RequestEnv, Status};
use forkline_engine::{Engine, EngineError, Request};
use futures_util::SinkExt;
use futures_util::stream::{FuturesOrdered, StreamExt};
use http::HeaderMap;
use tokio::net::{TcpListener, TcpStream};
use tokio::time::Instant;
use tokio_util::codec::{Framed, LengthDelimitedCodec};
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;

use crate::admission::Gate;
use crate::config::TcpListenerConfig;

/// Status byte: ok.
pub const STATUS_OK: u8 = 0;
/// Status byte: bad request.
pub const STATUS_BAD_REQUEST: u8 = 1;
/// Status byte: failed.
pub const STATUS_FAILED: u8 = 2;
/// Status byte: deadline.
pub const STATUS_DEADLINE: u8 = 3;
/// Status byte: overloaded.
pub const STATUS_OVERLOADED: u8 = 4;

#[derive(Clone)]
struct Conn {
    engine: Arc<Engine>,
    gate: Gate,
    plan: Arc<str>,
    max_frame: usize,
    max_in_flight: usize,
}

/// Accept connections until `shutdown`, then wait for open connections to
/// send what they owe.
pub(crate) async fn serve(
    listener: TcpListener,
    engine: Arc<Engine>,
    gate: Gate,
    config: TcpListenerConfig,
    shutdown: CancellationToken,
) -> std::io::Result<()> {
    let conn = Conn {
        engine,
        gate,
        plan: Arc::from(config.plan.as_str()),
        max_frame: config.max_frame_bytes,
        max_in_flight: config.max_in_flight_per_connection,
    };
    let connections = TaskTracker::new();
    loop {
        tokio::select! {
            () = shutdown.cancelled() => break,
            accepted = listener.accept() => match accepted {
                Ok((stream, peer)) => {
                    let _ = stream.set_nodelay(true);
                    connections.spawn(connection(stream, peer, conn.clone(), shutdown.clone()));
                }
                Err(e) => tracing::warn!(error = %e, "tcp accept failed"),
            },
        }
    }
    connections.close();
    connections.wait().await;
    Ok(())
}

fn frame(status: u8, body: &[u8]) -> Bytes {
    let mut out = BytesMut::with_capacity(1 + body.len());
    out.put_u8(status);
    out.put_slice(body);
    out.freeze()
}

async fn handle(conn: Conn, peer: SocketAddr, body: Bytes) -> Bytes {
    let received_at = Instant::now();
    let Some(_pass) = conn.gate.enter() else {
        return frame(STATUS_OVERLOADED, b"overloaded");
    };
    let env = RequestEnv {
        protocol: Protocol::Tcp,
        peer: Some(peer),
        received_at,
        inbound_deadline: None,
        route: conn.plan.to_string(),
        metadata: HeaderMap::new(),
    };
    let Some(result) = conn.gate.guard(conn.engine.call(&conn.plan, Request { body, env })).await else {
        return frame(STATUS_OVERLOADED, b"shutting down");
    };
    match result {
        Ok(reply) => {
            let status = match reply.status {
                Status::Ok => STATUS_OK,
                Status::Error(ErrorKind::BadRequest) => STATUS_BAD_REQUEST,
                Status::Error(ErrorKind::Deadline) => STATUS_DEADLINE,
                Status::Error(_) => STATUS_FAILED,
            };
            frame(status, &reply.body)
        }
        Err(EngineError::Shed { .. }) => frame(STATUS_OVERLOADED, b"shed"),
        Err(e) => {
            tracing::error!(error = %e, "request failed without a reply");
            frame(STATUS_FAILED, b"internal error")
        }
    }
}

async fn connection(stream: TcpStream, peer: SocketAddr, conn: Conn, shutdown: CancellationToken) {
    let codec = LengthDelimitedCodec::builder().max_frame_length(conn.max_frame).new_codec();
    let (mut sink, mut frames) = Framed::new(stream, codec).split();
    let mut pending = FuturesOrdered::new();
    let mut reading = true;
    loop {
        if !reading && pending.is_empty() {
            break;
        }
        tokio::select! {
            biased;
            Some(response) = pending.next(), if !pending.is_empty() => {
                if let Err(e) = sink.send(response).await {
                    tracing::debug!(%peer, error = %e, "tcp write failed");
                    return;
                }
            }
            next = frames.next(), if reading && pending.len() < conn.max_in_flight => match next {
                Some(Ok(body)) => pending.push_back(handle(conn.clone(), peer, body.freeze())),
                Some(Err(e)) => {
                    tracing::debug!(%peer, error = %e, "tcp frame rejected; closing after owed responses");
                    reading = false;
                }
                None => reading = false,
            },
            () = shutdown.cancelled(), if reading => reading = false,
            else => break,
        }
    }
    let _ = sink.close().await;
}
