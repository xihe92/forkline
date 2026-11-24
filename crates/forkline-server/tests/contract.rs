//! The contract every listener must pass, run against HTTP, gRPC and TCP
//! through one client, the way Spring runs one suite against Tomcat, Jetty
//! and Netty: outcomes, admission, clients that leave, graceful shutdown,
//! limits, slow and idle clients, malformed input, and connection handling.
//! Every test ends with the leak checks in `Running::stop`/`finish`.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, missing_docs)]

mod common;

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use common::{Running, plans, registry, start};
use forkline_engine::Engine;
use forkline_server::Hooks;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tonic::Code;

/// One listener of each kind, all running plan `p`, with the default
/// timeouts.
fn listeners(extra: &str) -> String {
    listeners_timing_out_after(60_000, extra)
}

/// As [`listeners`], with header-read and idle timeouts of `ms`.
fn listeners_timing_out_after(ms: u64, extra: &str) -> String {
    format!(
        "[[http]]\naddr = \"127.0.0.1:0\"\nroutes = {{ \"/p\" = \"p\" }}\nmax_body_bytes = 64\n\
         header_read_timeout_ms = {ms}\nidle_timeout_ms = {ms}\n\
         [[grpc]]\naddr = \"127.0.0.1:0\"\nmethods = {{ P = \"p\" }}\nidle_timeout_ms = {ms}\n\
         [[tcp]]\naddr = \"127.0.0.1:0\"\nplan = \"p\"\nmax_frame_bytes = 64\nidle_timeout_ms = {ms}\n{extra}"
    )
}

/// The timeouts the slow and idle client cases use.
const SHORT_MS: u64 = 300;

/// Plan `p` (see `common::Work`): replies with its input; `bad`, `fail` and
/// `panic` fail that way; `<ms>|text` waits `<ms>` first.
fn engine(budget_ms: u64) -> Arc<Engine> {
    Arc::new(Engine::new(registry(), &plans(&[("p", "", 0, budget_ms)])).unwrap())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Proto {
    Http,
    Grpc,
    Tcp,
}

const PROTOCOLS: [Proto; 3] = [Proto::Http, Proto::Grpc, Proto::Tcp];

/// What a listener answered, in terms common to the three protocols.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Outcome {
    Ok,
    BadRequest,
    Failed,
    Deadline,
    Overloaded,
    ShuttingDown,
    /// No answer: the connection failed or closed.
    NoAnswer,
    /// Anything else, as the protocol reported it.
    Other(String),
}

/// One client connection to one listener.
enum Client {
    Http { client: reqwest::Client, url: String },
    Grpc(tonic::client::Grpc<tonic::transport::Channel>),
    Tcp(TcpStream),
}

impl Client {
    async fn connect(s: &Running, proto: Proto) -> Self {
        match proto {
            Proto::Http => Client::Http { client: reqwest::Client::new(), url: s.http("/p") },
            Proto::Grpc => {
                let endpoint = tonic::transport::Endpoint::from_shared(format!("http://{}", s.grpc())).unwrap();
                Client::Grpc(tonic::client::Grpc::new(endpoint.connect().await.unwrap()))
            }
            Proto::Tcp => Client::Tcp(s.tcp().await),
        }
    }

    async fn call(&mut self, body: &str) -> (Outcome, String) {
        match self {
            Client::Http { client, url } => {
                let res = match client.post(url.as_str()).body(body.to_owned()).send().await {
                    Ok(res) => res,
                    Err(e) => return (Outcome::NoAnswer, e.to_string()),
                };
                let status = res.status().as_u16();
                let text = res.text().await.unwrap_or_default();
                let outcome = match (status, text.as_str()) {
                    (200, _) => Outcome::Ok,
                    (400, _) => Outcome::BadRequest,
                    (500, _) => Outcome::Failed,
                    (504, _) => Outcome::Deadline,
                    (503, "shutting down") => Outcome::ShuttingDown,
                    (503, _) => Outcome::Overloaded,
                    (other, _) => Outcome::Other(format!("HTTP {other}")),
                };
                (outcome, text)
            }
            Client::Grpc(grpc) => {
                if let Err(e) = grpc.ready().await {
                    return (Outcome::NoAnswer, e.to_string());
                }
                let path = http::uri::PathAndQuery::from_static("/forkline.v1.Plans/P");
                let request = tonic::Request::new(Bytes::from(body.to_owned()));
                match grpc.unary(request, path, forkline_server::grpc::BytesCodec).await {
                    Ok(reply) => (Outcome::Ok, String::from_utf8_lossy(reply.get_ref()).into_owned()),
                    Err(status) => {
                        let message = status.message().to_owned();
                        let outcome = match (status.code(), message.as_str()) {
                            (Code::InvalidArgument, _) => Outcome::BadRequest,
                            (Code::Internal, _) => Outcome::Failed,
                            (Code::DeadlineExceeded, _) => Outcome::Deadline,
                            (Code::ResourceExhausted, _) => Outcome::Overloaded,
                            (Code::Unavailable, "shutting down") => Outcome::ShuttingDown,
                            (Code::Unavailable | Code::Unknown, _) => Outcome::NoAnswer,
                            (code, _) => Outcome::Other(format!("gRPC {code:?}")),
                        };
                        (outcome, message)
                    }
                }
            }
            Client::Tcp(stream) => {
                common::send_frame(stream, body.as_bytes()).await;
                match common::read_frame(stream).await {
                    None => (Outcome::NoAnswer, String::new()),
                    Some((status, body)) => {
                        let text = String::from_utf8_lossy(&body).into_owned();
                        let outcome = match (status, text.as_str()) {
                            (0, _) => Outcome::Ok,
                            (1, _) => Outcome::BadRequest,
                            (2, _) => Outcome::Failed,
                            (3, _) => Outcome::Deadline,
                            (4, "shutting down") => Outcome::ShuttingDown,
                            (4, _) => Outcome::Overloaded,
                            (other, _) => Outcome::Other(format!("TCP status {other}")),
                        };
                        (outcome, text)
                    }
                }
            }
        }
    }
}

fn addr(s: &Running, proto: Proto) -> SocketAddr {
    match proto {
        Proto::Http => s.addrs.http.unwrap(),
        Proto::Grpc => s.addrs.grpc.unwrap(),
        Proto::Tcp => s.addrs.tcp.unwrap(),
    }
}

/// Read until the server closes the connection or `within` passes. Returns
/// what was read and whether it closed.
async fn read_until_closed(stream: &mut TcpStream, within: Duration) -> (String, bool) {
    let mut out = Vec::new();
    let read = tokio::time::timeout(within, async {
        let mut buf = [0u8; 4096];
        loop {
            match stream.read(&mut buf).await {
                Ok(0) | Err(_) => return,
                Ok(n) => out.extend_from_slice(&buf[..n]),
            }
        }
    })
    .await;
    (String::from_utf8_lossy(&out).into_owned(), read.is_ok())
}

// ---------------------------------------------------------------- outcomes

#[tokio::test]
async fn every_outcome_has_the_same_meaning_on_every_protocol() {
    let s = start(&listeners(""), engine(100), Hooks::default()).await;
    for proto in PROTOCOLS {
        let mut c = Client::connect(&s, proto).await;
        for (body, outcome, text) in [
            ("ok", Outcome::Ok, "ok"),
            ("bad", Outcome::BadRequest, "bad input"),
            ("fail", Outcome::Failed, "module failed"),
            ("panic", Outcome::Failed, "panicked: module panicked"),
            ("300|slow", Outcome::Deadline, "deadline passed while the node was running"),
        ] {
            assert_eq!(c.call(body).await, (outcome, text.to_owned()), "{proto:?} {body}");
        }
    }
    s.stop().await;
}

#[tokio::test]
async fn unknown_routes_and_methods() {
    let s = start(&listeners(""), engine(100), Hooks::default()).await;
    let c = reqwest::Client::new();
    assert_eq!(c.post(s.http("/nope")).body("x").send().await.unwrap().status(), 404);
    assert_eq!(c.put(s.http("/p")).body("x").send().await.unwrap().status(), 405, "only GET and POST");
    drop(c);
    let status = common::grpc_call(s.grpc(), "Nope", "x").await.unwrap_err();
    assert_eq!(
        (status.code(), status.message()),
        (Code::Unimplemented, "no plan is mapped to /forkline.v1.Plans/Nope")
    );
    // TCP: one plan per listener, nothing to route.
    s.stop().await;
}

#[tokio::test]
async fn admission_refuses_at_once_on_every_protocol_and_recovers() {
    let s = start(&listeners("[admission]\nred = 2\ngreen = 1\n"), engine(5000), Hooks::default()).await;
    // Two slow requests fill the server (red = 2).
    let fill: Vec<_> = (0..2)
        .map(|_| {
            let c = reqwest::Client::new();
            let url = s.http("/p");
            tokio::spawn(async move { c.post(url).body("400|slow").send().await.unwrap().status() })
        })
        .collect();
    tokio::time::sleep(Duration::from_millis(100)).await;
    let mut clients = Vec::new();
    for proto in PROTOCOLS {
        let mut c = Client::connect(&s, proto).await;
        let started = Instant::now();
        assert_eq!(c.call("x").await.0, Outcome::Overloaded, "{proto:?}");
        assert!(started.elapsed() < Duration::from_millis(100), "{proto:?}: refused without waiting");
        clients.push(c);
    }
    for f in fill {
        assert_eq!(f.await.unwrap(), 200);
    }
    for (proto, c) in PROTOCOLS.iter().zip(&mut clients) {
        assert_eq!(c.call("again").await, (Outcome::Ok, "again".to_owned()), "{proto:?}: back under green");
    }
    drop(clients);
    s.stop().await;
}

#[tokio::test]
async fn a_client_that_leaves_mid_request_frees_its_slot() {
    // `stop` fails unless every listener is back to 0 in flight within 1 s.
    // HTTP and gRPC cancel the request as soon as the client leaves, long
    // before the abandoned 5-second request would end. TCP can't tell a
    // client that left from one that only closed its sending side and still
    // wants its replies (see the half-close case), so the request runs to
    // its end (here 500 ms), the reply's write fails, and the slot frees then.
    // Admission control is on, so its count is checked too.
    let s = start(&listeners("[admission]\nred = 50\ngreen = 10\n"), engine(10_000), Hooks::default()).await;
    for proto in PROTOCOLS {
        let mut c = Client::connect(&s, proto).await;
        let body = if proto == Proto::Tcp { "500|abandoned" } else { "5000|abandoned" };
        let call = tokio::spawn(async move { c.call(body).await });
        tokio::time::sleep(Duration::from_millis(100)).await;
        call.abort();
        assert!(call.await.unwrap_err().is_cancelled(), "{proto:?}");
    }
    s.stop().await;
}

// ---------------------------------------------------------------- shutdown

#[tokio::test]
async fn a_request_in_flight_when_shutdown_starts_completes() {
    for proto in PROTOCOLS {
        let s = start(&listeners("[shutdown]\ndrain_timeout_ms = 5000\n"), engine(5000), Hooks::default()).await;
        let mut c = Client::connect(&s, proto).await;
        let call = tokio::spawn(async move {
            let answer = c.call("300|late").await;
            (answer, c)
        });
        tokio::time::sleep(Duration::from_millis(100)).await;
        s.shutdown.cancel();
        let (answer, c) = call.await.unwrap();
        assert_eq!(answer, (Outcome::Ok, "late".to_owned()), "{proto:?}");
        drop(c);
        s.finish().await;
    }
}

#[tokio::test]
async fn a_request_still_running_at_the_drain_timeout_is_cut_off() {
    for proto in PROTOCOLS {
        let s = start(&listeners("[shutdown]\ndrain_timeout_ms = 200\n"), engine(60_000), Hooks::default()).await;
        let mut c = Client::connect(&s, proto).await;
        let call = tokio::spawn(async move {
            let answer = c.call("30000|never").await;
            (answer, c)
        });
        tokio::time::sleep(Duration::from_millis(100)).await;
        let started = Instant::now();
        s.shutdown.cancel();
        let (answer, c) = call.await.unwrap();
        assert_eq!(answer, (Outcome::ShuttingDown, "shutting down".to_owned()), "{proto:?}");
        drop(c);
        s.finish().await;
        assert!(started.elapsed() < Duration::from_millis(1500), "{proto:?}: {:?}", started.elapsed());
    }
}

#[tokio::test]
async fn an_idle_connection_does_not_hold_up_shutdown() {
    // Connections with nothing in flight close within a second; shutdown
    // does not wait for the 10 s drain timeout.
    for proto in PROTOCOLS {
        let mut s = start(&listeners("[shutdown]\ndrain_timeout_ms = 10000\n"), engine(5000), Hooks::default()).await;
        let mut c = Client::connect(&s, proto).await;
        assert_eq!(c.call("warm").await.0, Outcome::Ok, "{proto:?}");
        // And one that never sent anything (for gRPC: not even the preface).
        let silent = TcpStream::connect(addr(&s, proto)).await.unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;
        let started = Instant::now();
        s.shutdown.cancel();
        s.server_stopped().await;
        assert!(started.elapsed() < Duration::from_millis(1500), "{proto:?}: {:?}", started.elapsed());
        drop((c, silent));
        s.finish().await;
    }
}

#[tokio::test]
async fn a_stop_racing_startup_completes() {
    for _ in 0..20 {
        let s = start(&listeners(""), engine(100), Hooks::default()).await;
        s.stop().await;
    }
}

#[tokio::test]
async fn new_connections_are_refused_while_draining() {
    for proto in PROTOCOLS {
        let s = start(&listeners("[shutdown]\ndrain_timeout_ms = 5000\n"), engine(5000), Hooks::default()).await;
        let mut busy = Client::connect(&s, proto).await;
        let call = tokio::spawn(async move {
            let answer = busy.call("500|draining").await;
            (answer, busy)
        });
        tokio::time::sleep(Duration::from_millis(100)).await;
        s.shutdown.cancel();
        tokio::time::sleep(Duration::from_millis(50)).await;
        let refused = TcpStream::connect(addr(&s, proto)).await;
        assert!(refused.is_err(), "{proto:?}: a new connection was accepted while draining");
        let (answer, busy) = call.await.unwrap();
        assert_eq!(answer.0, Outcome::Ok, "{proto:?}: the request in flight still completes");
        drop(busy);
        s.finish().await;
    }
}

// ---------------------------------------------------------------- limits

#[tokio::test]
async fn bodies_over_the_limit_are_refused() {
    let s = start(&listeners(""), engine(1000), Hooks::default()).await;
    let c = reqwest::Client::new();
    let res = c.post(s.http("/p")).body(vec![b'x'; 65]).send().await.unwrap();
    assert_eq!(res.status(), 413, "over max_body_bytes");
    assert_eq!(c.post(s.http("/p")).body(vec![b'x'; 64]).send().await.unwrap().status(), 200, "at the limit");
    drop(c);

    // gRPC: tonic's default 4 MiB decoding limit.
    let big = "x".repeat(5 << 20);
    let status = common::grpc_call(s.grpc(), "P", &big).await.unwrap_err();
    assert_eq!(status.code(), Code::OutOfRange, "{status:?}");
    assert_eq!(common::grpc_call(s.grpc(), "P", "after").await.unwrap(), Bytes::from("after"));

    // TCP: a frame over max_frame_bytes closes the connection.
    let mut tcp = s.tcp().await;
    common::send_frame(&mut tcp, &[b'x'; 65]).await;
    assert!(common::read_frame(&mut tcp).await.is_none());
    drop(tcp);
    s.stop().await;
}

#[tokio::test]
async fn too_many_or_too_large_headers_are_refused() {
    let s = start(&listeners(""), engine(1000), Hooks::default()).await;
    let http = s.addrs.http.unwrap();

    let mut many = String::from("POST /p HTTP/1.1\r\nhost: x\r\ncontent-length: 1\r\n");
    for i in 0..200 {
        many.push_str(&format!("x-h{i}: v\r\n"));
    }
    many.push_str("\r\nx");
    let mut conn = TcpStream::connect(http).await.unwrap();
    conn.write_all(many.as_bytes()).await.unwrap();
    let (text, closed) = read_until_closed(&mut conn, Duration::from_secs(2)).await;
    assert!(text.starts_with("HTTP/1.1 431"), "200 headers: {text:?}");
    assert!(closed);
    drop(conn);

    let huge = format!("POST /p HTTP/1.1\r\nhost: x\r\nx-big: {}\r\ncontent-length: 1\r\n\r\nx", "v".repeat(1 << 20));
    let mut conn = TcpStream::connect(http).await.unwrap();
    let _ = conn.write_all(huge.as_bytes()).await; // the server may close before reading it all
    let (text, closed) = read_until_closed(&mut conn, Duration::from_secs(2)).await;
    assert!(text.starts_with("HTTP/1.1 431") || text.is_empty(), "1 MiB header: {text:?}");
    assert!(closed);
    drop(conn);

    // gRPC: 1 MiB of metadata.
    let channel =
        tonic::transport::Endpoint::from_shared(format!("http://{}", s.grpc())).unwrap().connect().await.unwrap();
    let mut grpc = tonic::client::Grpc::new(channel);
    grpc.ready().await.unwrap();
    let mut request = tonic::Request::new(Bytes::from("x"));
    request.metadata_mut().insert("x-big", "v".repeat(1 << 20).parse().unwrap());
    let path = http::uri::PathAndQuery::from_static("/forkline.v1.Plans/P");
    let refused = grpc.unary(request, path, forkline_server::grpc::BytesCodec).await;
    assert!(refused.is_err(), "1 MiB of gRPC metadata was accepted");
    drop(grpc);
    assert_eq!(common::grpc_call(s.grpc(), "P", "after").await.unwrap(), Bytes::from("after"));
    s.stop().await;
}

// ---------------------------------------------------------------- slow and idle clients

#[tokio::test]
async fn a_connection_that_sends_nothing_is_closed() {
    let s = start(&listeners_timing_out_after(SHORT_MS, ""), engine(1000), Hooks::default()).await;
    for proto in PROTOCOLS {
        let mut conn = TcpStream::connect(addr(&s, proto)).await.unwrap();
        let started = Instant::now();
        let (_, closed) = read_until_closed(&mut conn, Duration::from_secs(2)).await;
        assert!(closed, "{proto:?}: a silent connection was still open after 2 s");
        assert!(started.elapsed() >= Duration::from_millis(250), "{proto:?}: closed too early");
    }
    s.stop().await;
}

#[tokio::test]
async fn headers_sent_one_byte_at_a_time_are_cut_off() {
    let s = start(&listeners_timing_out_after(SHORT_MS, ""), engine(1000), Hooks::default()).await;
    let mut conn = TcpStream::connect(s.addrs.http.unwrap()).await.unwrap();
    let request = b"POST /p HTTP/1.1\r\nhost: x\r\nx-slow: aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\r\n\r\n";
    let started = Instant::now();
    let mut cut_off = false;
    for byte in request {
        if conn.write_all(&[*byte]).await.is_err() {
            cut_off = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
        if started.elapsed() > Duration::from_secs(2) {
            break;
        }
    }
    let (text, closed) = read_until_closed(&mut conn, Duration::from_millis(500)).await;
    assert!(cut_off || closed, "a client trickling headers kept the connection for {:?}", started.elapsed());
    assert!(!text.contains("200 OK"), "{text:?}");
    assert!(started.elapsed() < Duration::from_millis(1000), "{:?}", started.elapsed());
    drop(conn);
    s.stop().await;
}

#[tokio::test]
async fn idle_kept_alive_connections_are_closed() {
    let s = start(&listeners_timing_out_after(SHORT_MS, ""), engine(1000), Hooks::default()).await;
    for proto in PROTOCOLS {
        let mut conns = Vec::new();
        for _ in 0..20 {
            let mut conn = TcpStream::connect(addr(&s, proto)).await.unwrap();
            match proto {
                Proto::Http => {
                    conn.write_all(b"POST /p HTTP/1.1\r\nhost: x\r\ncontent-length: 2\r\n\r\nhi").await.unwrap();
                    let mut buf = [0u8; 256];
                    let n = conn.read(&mut buf).await.unwrap();
                    assert!(buf[..n].starts_with(b"HTTP/1.1 200"));
                }
                Proto::Tcp => {
                    common::send_frame(&mut conn, b"hi").await;
                    assert_eq!(common::read_frame(&mut conn).await.unwrap().0, 0);
                }
                Proto::Grpc => {} // covered by the silent-connection case
            }
            conns.push(conn);
        }
        for conn in &mut conns {
            let (_, closed) = read_until_closed(conn, Duration::from_secs(2)).await;
            assert!(closed, "{proto:?}: an idle connection was still open after 2 s");
        }
    }
    s.stop().await;
}

// ---------------------------------------------------------------- malformed input

/// Send `raw` on a new HTTP connection; return the response text once the
/// server closes the connection (or after 2 s).
async fn raw_http(s: &Running, raw: &[u8]) -> String {
    let mut conn = TcpStream::connect(s.addrs.http.unwrap()).await.unwrap();
    let _ = conn.write_all(raw).await;
    let (text, _) = read_until_closed(&mut conn, Duration::from_secs(2)).await;
    text
}

#[tokio::test]
async fn malformed_http_requests_are_refused_and_never_smuggled() {
    let s = start(&listeners(""), engine(1000), Hooks::default()).await;
    // Both Content-Length and Transfer-Encoding: RFC 9112 (6.3) lets the
    // server go by Transfer-Encoding alone, as long as it then closes the
    // connection, so the bytes after the chunked body never become a request.
    let text = raw_http(
        &s,
        b"POST /p HTTP/1.1\r\nhost: x\r\ncontent-length: 4\r\ntransfer-encoding: chunked\r\n\r\n0\r\n\r\n\
          POST /p HTTP/1.1\r\nhost: x\r\ncontent-length: 8\r\n\r\nsmuggled",
    )
    .await;
    assert!(text.to_lowercase().contains("connection: close"), "{text:?}");
    assert_eq!(text.matches("HTTP/1.1 ").count(), 1, "a second request ran: {text:?}");
    assert!(!text.contains("smuggled"), "{text:?}");

    let cases: [(&str, &[u8]); 4] = [
        (
            "two content-lengths",
            b"POST /p HTTP/1.1\r\nhost: x\r\ncontent-length: 1\r\ncontent-length: 8\r\n\r\nsmuggled",
        ),
        (
            "a bad chunk size",
            b"POST /p HTTP/1.1\r\nhost: x\r\ntransfer-encoding: chunked\r\n\r\nzz\r\nsmuggled\r\n0\r\n\r\n",
        ),
        ("an invalid method", b"P@ST /p HTTP/1.1\r\nhost: x\r\ncontent-length: 1\r\n\r\nx"),
        ("a space in the header name", b"POST /p HTTP/1.1\r\nhost: x\r\ncontent-length : 1\r\n\r\nx"),
    ];
    for (what, raw) in cases {
        let text = raw_http(&s, raw).await;
        assert!(text.starts_with("HTTP/1.1 400") || text.is_empty(), "{what}: {text:?}");
        assert!(!text.contains("smuggled"), "{what}: a second request ran: {text:?}");
    }
    // The listener still serves.
    let text = raw_http(&s, b"POST /p HTTP/1.1\r\nhost: x\r\ncontent-length: 2\r\nconnection: close\r\n\r\nok").await;
    assert!(text.starts_with("HTTP/1.1 200") && text.ends_with("ok"), "{text:?}");
    s.stop().await;
}

#[tokio::test]
async fn malformed_grpc_input_is_refused() {
    let s = start(&listeners(""), engine(1000), Hooks::default()).await;
    // The HTTP/2 preface followed by garbage instead of a SETTINGS frame.
    let mut conn = TcpStream::connect(s.grpc()).await.unwrap();
    conn.write_all(b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n").await.unwrap();
    conn.write_all(&[0xff; 64]).await.unwrap();
    let (_, closed) = read_until_closed(&mut conn, Duration::from_secs(2)).await;
    assert!(closed, "the connection survived a garbage frame");
    drop(conn);

    // Not gRPC at all: HTTP/1.1 on the gRPC port.
    let mut conn = TcpStream::connect(s.grpc()).await.unwrap();
    conn.write_all(b"POST /forkline.v1.Plans/P HTTP/1.1\r\nhost: x\r\ncontent-length: 1\r\n\r\nx").await.unwrap();
    let (text, _) = read_until_closed(&mut conn, Duration::from_secs(2)).await;
    assert!(!text.contains("200 OK"), "{text:?}");
    drop(conn);

    // HTTP/2 without the gRPC content type or framing.
    let client = reqwest::Client::builder().http2_prior_knowledge().build().unwrap();
    let res = client
        .post(format!("http://{}/forkline.v1.Plans/P", s.grpc()))
        .header("content-type", "text/plain")
        .body("not a grpc message")
        .send()
        .await
        .unwrap();
    let grpc_status = res.headers().get("grpc-status").map(|v| v.to_str().unwrap().to_owned());
    assert!(res.status() != 200 || grpc_status.as_deref() != Some("0"), "{res:?}");
    drop(client);

    assert_eq!(common::grpc_call(s.grpc(), "P", "after").await.unwrap(), Bytes::from("after"));
    s.stop().await;
}

#[tokio::test]
async fn truncated_and_empty_tcp_frames() {
    let s = start(&listeners(""), engine(1000), Hooks::default()).await;
    // A length prefix promising 10 bytes, then 3, then the client leaves.
    let mut conn = s.tcp().await;
    conn.write_all(&10u32.to_be_bytes()).await.unwrap();
    conn.write_all(b"abc").await.unwrap();
    drop(conn);
    // A truncated length prefix.
    let mut conn = s.tcp().await;
    conn.write_all(&[0, 0]).await.unwrap();
    drop(conn);
    // An empty frame is a request with an empty body.
    let mut conn = s.tcp().await;
    common::send_frame(&mut conn, b"").await;
    assert_eq!(common::read_frame(&mut conn).await.unwrap(), (0, Bytes::new()));
    drop(conn);
    s.stop().await;
}

// ---------------------------------------------------------------- connections

#[tokio::test]
async fn http_keep_alive_serves_several_requests_on_one_connection() {
    let s = start(&listeners(""), engine(1000), Hooks::default()).await;
    let mut conn = TcpStream::connect(s.addrs.http.unwrap()).await.unwrap();
    for body in ["one", "two", "six"] {
        conn.write_all(format!("POST /p HTTP/1.1\r\nhost: x\r\ncontent-length: 3\r\n\r\n{body}").as_bytes())
            .await
            .unwrap();
        let mut buf = [0u8; 512];
        let n = conn.read(&mut buf).await.unwrap();
        let text = String::from_utf8_lossy(&buf[..n]);
        assert!(text.starts_with("HTTP/1.1 200") && text.ends_with(body), "{text:?}");
    }
    drop(conn);
    s.stop().await;
}

#[tokio::test]
async fn http2_without_tls_is_served_on_the_http_listener() {
    let s = start(&listeners(""), engine(1000), Hooks::default()).await;
    let client = reqwest::Client::builder().http2_prior_knowledge().build().unwrap();
    let res = client.post(s.http("/p")).body("h2c").send().await.unwrap();
    assert_eq!(res.version(), http::Version::HTTP_2);
    assert_eq!(res.status(), 200);
    assert_eq!(res.text().await.unwrap(), "h2c");
    drop(client);
    s.stop().await;
}

#[tokio::test]
async fn a_half_closed_tcp_connection_still_gets_its_replies() {
    let s = start(&listeners(""), engine(1000), Hooks::default()).await;
    let mut conn = s.tcp().await;
    common::send_frame(&mut conn, b"50|first").await;
    common::send_frame(&mut conn, b"second").await;
    conn.shutdown().await.unwrap(); // no more requests
    assert_eq!(common::read_frame(&mut conn).await.unwrap(), (0, Bytes::from("first")));
    assert_eq!(common::read_frame(&mut conn).await.unwrap(), (0, Bytes::from("second")));
    assert!(common::read_frame(&mut conn).await.is_none(), "then the server closes");
    drop(conn);
    s.stop().await;
}
