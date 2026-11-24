//! What each protocol answers for every outcome: success, bad request,
//! module failure, panic, deadline, unknown route, overload. Plus the HTTP
//! body limit and the TCP adapter's connection handling.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, missing_docs)]

mod common;

use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use common::{grpc_call, plans, read_frame, registry, send_frame, start};
use forkline_engine::Engine;
use forkline_server::Hooks;
use tonic::Code;

const ALL_LISTENERS: &str = "[[http]]\naddr = \"127.0.0.1:0\"\nroutes = { \"/p\" = \"p\" }\nmax_body_bytes = 64\n\
     [[grpc]]\naddr = \"127.0.0.1:0\"\nmethods = { P = \"p\" }\n[[tcp]]\naddr = \"127.0.0.1:0\"\nplan = \"p\"\n";

fn engine(budget_ms: u64) -> Arc<Engine> {
    Arc::new(Engine::new(registry(), &plans(&[("p", "", 0, budget_ms)])).unwrap())
}

#[tokio::test]
async fn http_answers_each_outcome_with_its_status() {
    let s = start(ALL_LISTENERS, engine(100), Hooks::default()).await;
    let c = reqwest::Client::new();
    let post = |body: &'static str| c.post(s.http("/p")).body(body).send();
    let cases = [
        ("ok", 200, "ok"),
        ("bad", 400, "bad input"),
        ("fail", 500, "module failed"),
        ("panic", 500, "panicked: module panicked"),
        ("300|slow", 504, "deadline passed while the node was running"),
    ];
    for (body, status, text) in cases {
        let res = post(body).await.unwrap();
        assert_eq!(res.status(), status, "{body}");
        assert_eq!(res.text().await.unwrap(), text, "{body}");
    }

    assert_eq!(c.put(s.http("/p")).body("x").send().await.unwrap().status(), 405, "only GET and POST");
    assert_eq!(c.post(s.http("/nope")).body("x").send().await.unwrap().status(), 404);
    let res = c.post(s.http("/p")).body(vec![b'x'; 65]).send().await.unwrap();
    assert_eq!(res.status(), 413, "over max_body_bytes");
    assert_eq!(c.post(s.http("/p")).body(vec![b'x'; 64]).send().await.unwrap().status(), 200, "at the limit");
    drop(c);
    s.stop().await;
}

#[tokio::test]
async fn grpc_answers_each_outcome_with_its_code() {
    let s = start(ALL_LISTENERS, engine(100), Hooks::default()).await;
    assert_eq!(grpc_call(s.grpc(), "P", "ok").await.unwrap(), Bytes::from("ok"));
    let cases = [
        ("bad", Code::InvalidArgument, "bad input"),
        ("fail", Code::Internal, "module failed"),
        ("panic", Code::Internal, "panicked: module panicked"),
        ("300|slow", Code::DeadlineExceeded, "deadline passed while the node was running"),
    ];
    for (body, code, message) in cases {
        let status = grpc_call(s.grpc(), "P", body).await.unwrap_err();
        assert_eq!((status.code(), status.message()), (code, message), "{body}");
    }
    let status = grpc_call(s.grpc(), "Nope", "x").await.unwrap_err();
    assert_eq!(status.code(), Code::Unimplemented);
    assert_eq!(status.message(), "no plan is mapped to /forkline.v1.Plans/Nope");
    s.stop().await;
}

#[tokio::test]
async fn tcp_answers_each_outcome_with_its_status_byte() {
    let s = start(ALL_LISTENERS, engine(100), Hooks::default()).await;
    let mut conn = s.tcp().await;
    let cases: [(&[u8], u8, &str); 5] = [
        (b"ok", 0, "ok"),
        (b"bad", 1, "bad input"),
        (b"fail", 2, "module failed"),
        (b"panic", 2, "panicked: module panicked"),
        (b"300|slow", 3, "deadline passed while the node was running"),
    ];
    for (body, status, text) in cases {
        send_frame(&mut conn, body).await;
        assert_eq!(read_frame(&mut conn).await.unwrap(), (status, Bytes::from(text)), "{body:?}");
    }
    drop(conn);
    s.stop().await;
}

#[tokio::test]
async fn admission_refuses_on_every_protocol_and_recovers() {
    let config = format!("{ALL_LISTENERS}[admission]\nred = 2\ngreen = 1\n");
    let s = start(&config, engine(5000), Hooks::default()).await;
    let c = reqwest::Client::new();
    // Two slow requests fill the server (red = 2).
    let slow: Vec<_> = (0..2).map(|_| tokio::spawn(c.post(s.http("/p")).body("400|slow").send())).collect();
    tokio::time::sleep(Duration::from_millis(100)).await;

    let status = grpc_call(s.grpc(), "P", "x").await.unwrap_err();
    assert_eq!((status.code(), status.message()), (Code::ResourceExhausted, "overloaded"));
    let mut conn = s.tcp().await;
    send_frame(&mut conn, b"x").await;
    assert_eq!(read_frame(&mut conn).await.unwrap(), (4, Bytes::from("overloaded")));
    let res = c.post(s.http("/p")).body("x").send().await.unwrap();
    assert_eq!(res.status(), 503);
    assert_eq!(res.headers()["retry-after"], "1");

    for r in slow {
        assert_eq!(r.await.unwrap().unwrap().status(), 200);
    }
    // Back under green: everything is admitted again.
    assert_eq!(grpc_call(s.grpc(), "P", "x").await.unwrap(), Bytes::from("x"));
    send_frame(&mut conn, b"y").await;
    assert_eq!(read_frame(&mut conn).await.unwrap(), (0, Bytes::from("y")));
    drop(c);
    drop(conn);
    s.stop().await;
}

#[tokio::test]
async fn a_client_that_disconnects_frees_its_admission_slot() {
    let config = "[[http]]\naddr = \"127.0.0.1:0\"\nroutes = { \"/p\" = \"p\" }\n[admission]\nred = 2\ngreen = 1\n";
    let s = start(config, engine(5000), Hooks::default()).await;
    let impatient = reqwest::Client::builder().timeout(Duration::from_millis(100)).build().unwrap();
    for _ in 0..2 {
        assert!(impatient.post(s.http("/p")).body("2000|x").send().await.unwrap_err().is_timeout());
    }
    tokio::time::sleep(Duration::from_millis(100)).await;
    // Had the abandoned requests kept their slots, these would be refused.
    let c = reqwest::Client::new();
    let (a, b) = tokio::join!(c.post(s.http("/p")).body("50|a").send(), c.post(s.http("/p")).body("50|b").send());
    assert_eq!((a.unwrap().status().as_u16(), b.unwrap().status().as_u16()), (200, 200));
    drop(c);
    s.stop().await;
}

#[tokio::test]
async fn tcp_sends_the_responses_it_owes_before_closing_on_an_oversize_frame() {
    let s = start(ALL_LISTENERS, engine(1000), Hooks::default()).await;
    let mut conn = s.tcp().await;
    send_frame(&mut conn, b"50|owed").await;
    // Only the header: the server rejects the frame by its declared length.
    tokio::io::AsyncWriteExt::write_all(&mut conn, &(2u32 << 20).to_be_bytes()).await.unwrap();
    assert_eq!(read_frame(&mut conn).await.unwrap(), (0, Bytes::from("owed")));
    assert!(read_frame(&mut conn).await.is_none(), "then the connection closes");
    drop(conn);
    s.stop().await;
}

#[tokio::test]
async fn a_tcp_client_leaving_mid_pipeline_does_not_disturb_the_server() {
    let s = start(ALL_LISTENERS, engine(1000), Hooks::default()).await;
    let mut gone = s.tcp().await;
    send_frame(&mut gone, b"100|never read").await;
    send_frame(&mut gone, b"100|never read").await;
    drop(gone);
    tokio::time::sleep(Duration::from_millis(150)).await;

    let mut conn = s.tcp().await;
    send_frame(&mut conn, b"still here").await;
    assert_eq!(read_frame(&mut conn).await.unwrap(), (0, Bytes::from("still here")));
    drop(conn);
    s.stop().await;
}

#[tokio::test]
async fn tcp_reads_no_further_than_max_in_flight() {
    async fn two_slow_requests(max_in_flight: usize) -> Duration {
        let config =
            format!("[[tcp]]\naddr = \"127.0.0.1:0\"\nplan = \"p\"\nmax_in_flight_per_connection = {max_in_flight}\n");
        let s = start(&config, engine(5000), Hooks::default()).await;
        let mut conn = s.tcp().await;
        let started = Instant::now();
        send_frame(&mut conn, b"200|a").await;
        send_frame(&mut conn, b"200|b").await;
        assert_eq!(read_frame(&mut conn).await.unwrap(), (0, Bytes::from("a")));
        assert_eq!(read_frame(&mut conn).await.unwrap(), (0, Bytes::from("b")));
        let took = started.elapsed();
        drop(conn);
        s.stop().await;
        took
    }
    let one = two_slow_requests(1).await;
    assert!(one >= Duration::from_millis(390), "one at a time: {one:?}");
    let two = two_slow_requests(2).await;
    assert!(two < Duration::from_millis(390), "both at once: {two:?}");
}

#[tokio::test]
async fn a_failing_error_node_is_an_internal_error_on_every_protocol() {
    let engine = Arc::new(Engine::new(registry(), &common::broken_terminal_plan("p")).unwrap());
    let s = start(ALL_LISTENERS, engine, Hooks::default()).await;

    let res = reqwest::Client::new().post(s.http("/p")).body("fail").send().await.unwrap();
    assert_eq!(res.status(), 500);
    assert_eq!(res.text().await.unwrap(), "internal error");

    let status = grpc_call(s.grpc(), "P", "fail").await.unwrap_err();
    assert_eq!((status.code(), status.message()), (Code::Internal, "internal error"));

    let mut conn = s.tcp().await;
    send_frame(&mut conn, b"fail").await;
    assert_eq!(read_frame(&mut conn).await.unwrap(), (2, Bytes::from("internal error")));

    // Requests that succeed never touch the error node.
    send_frame(&mut conn, b"fine").await;
    assert_eq!(read_frame(&mut conn).await.unwrap(), (0, Bytes::from("fine")));
    drop(conn);
    s.stop().await;
}

#[tokio::test]
async fn a_grpc_request_whose_deadline_already_passed_never_runs() {
    // Two things race here, and both refuse the request without running it:
    // tonic enforces grpc-timeout outside our handler (CANCELLED, "Timeout
    // expired"), and when the body is already readable the engine sheds the
    // late request itself (RESOURCE_EXHAUSTED, "shed").
    let engine = Arc::new(Engine::new(registry(), &common::detached_plan("d", "expired-on-arrival", 0, 1000)).unwrap());
    let s = start("[[grpc]]\naddr = \"127.0.0.1:0\"\nmethods = { D = \"d\" }\n", engine, Hooks::default()).await;
    let channel =
        tonic::transport::Endpoint::from_shared(format!("http://{}", s.grpc())).unwrap().connect().await.unwrap();
    let mut client = tonic::client::Grpc::new(channel);
    client.ready().await.unwrap();
    let mut request = tonic::Request::new(Bytes::from("x"));
    request.metadata_mut().insert("grpc-timeout", "1n".parse().unwrap());
    let path = http::uri::PathAndQuery::from_static("/forkline.v1.Plans/D");
    let status = client.unary(request, path, forkline_server::grpc::BytesCodec).await.unwrap_err();
    let outcome = (status.code(), status.message());
    assert!(
        outcome == (Code::Cancelled, "Timeout expired") || outcome == (Code::ResourceExhausted, "shed"),
        "{outcome:?}"
    );
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(common::marks("expired-on-arrival"), 0, "the plan never ran");
    drop(client);
    s.stop().await;
}
