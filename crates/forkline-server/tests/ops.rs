//! Phase 4 gate: reload under load drops nothing; 2x overload keeps admitted
//! requests in budget and refuses the rest at once. Plus the admin API, the
//! TCP adapter and the graceful drain.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, missing_docs)]

mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use bytes::Bytes;
use common::{plans, read_frame, registry, send_frame, start};
use forkline_engine::Engine;
use forkline_server::{Hooks, Reloader};

fn percentile(sorted: &[Duration], p: f64) -> Duration {
    let rank = ((p / 100.0) * sorted.len() as f64).ceil() as usize;
    sorted[rank.clamp(1, sorted.len()) - 1]
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_reload_under_full_load_drops_zero_requests() {
    let engine = Arc::new(Engine::new(registry(), &plans(&[("p", "-a", 1, 1000)])).unwrap());
    let flip = Arc::new(AtomicBool::new(false));
    let reloader: Reloader = {
        let (engine, flip) = (engine.clone(), flip.clone());
        Arc::new(move || {
            let suffix = if flip.fetch_xor(true, Ordering::SeqCst) { "-a" } else { "-b" };
            engine.reload(&plans(&[("p", suffix, 1, 1000)])).map_err(|d| d.to_string())
        })
    };
    let s = start(
        "[[http]]\naddr = \"127.0.0.1:0\"\nroutes = { \"/p\" = \"p\" }\n[admin]\naddr = \"127.0.0.1:0\"\n",
        engine.clone(),
        Hooks { reloader: Some(reloader), metrics: None },
    )
    .await;

    let client = reqwest::Client::new();
    let stop = Arc::new(AtomicBool::new(false));
    let (ok, bad, seen_a, seen_b) = (
        Arc::new(AtomicU64::new(0)),
        Arc::new(AtomicU64::new(0)),
        Arc::new(AtomicU64::new(0)),
        Arc::new(AtomicU64::new(0)),
    );
    let mut workers = Vec::new();
    for _ in 0..32 {
        let (client, url, stop) = (client.clone(), s.http("/p"), stop.clone());
        let (ok, bad, seen_a, seen_b) = (ok.clone(), bad.clone(), seen_a.clone(), seen_b.clone());
        workers.push(tokio::spawn(async move {
            while !stop.load(Ordering::Relaxed) {
                match client.post(&url).body("x").send().await {
                    Ok(res) if res.status() == 200 => match res.text().await.as_deref() {
                        Ok("x-a") => {
                            seen_a.fetch_add(1, Ordering::Relaxed);
                            ok.fetch_add(1, Ordering::Relaxed);
                        }
                        Ok("x-b") => {
                            seen_b.fetch_add(1, Ordering::Relaxed);
                            ok.fetch_add(1, Ordering::Relaxed);
                        }
                        other => panic!("unexpected body {other:?}"),
                    },
                    _ => {
                        bad.fetch_add(1, Ordering::Relaxed);
                    }
                }
            }
        }));
    }

    let mut reloads = 0;
    let until = Instant::now() + Duration::from_millis(2000);
    while Instant::now() < until {
        let res = client.post(s.admin("/admin/reload")).send().await.unwrap();
        assert_eq!(res.status(), 200, "{:?}", res.text().await);
        reloads += 1;
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    stop.store(true, Ordering::Relaxed);
    for w in workers {
        w.await.unwrap();
    }
    let (ok, bad) = (ok.load(Ordering::Relaxed), bad.load(Ordering::Relaxed));
    println!("reloads {reloads}, requests ok {ok}, failed {bad}");
    assert_eq!(bad, 0, "no request failed across {reloads} reloads");
    assert!(reloads >= 50, "{reloads}");
    assert!(ok >= 1000, "{ok}");
    assert!(seen_a.load(Ordering::Relaxed) > 0 && seen_b.load(Ordering::Relaxed) > 0);
    assert_eq!(engine.snapshot().version, reloads + 1);
    drop(client);
    s.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn at_twice_the_admission_limit_admitted_requests_stay_in_budget_and_the_rest_are_refused_at_once() {
    const RED: usize = 40;
    const BUDGET_MS: u64 = 500;
    let engine =
        Arc::new(Engine::new(registry(), &plans(&[("slow", "", 100, BUDGET_MS), ("fast", "", 0, BUDGET_MS)])).unwrap());
    let s = start(
        &format!(
            "[[http]]\naddr = \"127.0.0.1:0\"\nroutes = {{ \"/slow\" = \"slow\", \"/fast\" = \"fast\" }}\n\
             [admission]\nred = {RED}\ngreen = {}\n",
            RED / 2
        ),
        engine,
        Hooks::default(),
    )
    .await;

    // One warm connection per client, so the burst measures the server, not TCP setup.
    let clients: Vec<reqwest::Client> = (0..2 * RED).map(|_| reqwest::Client::new()).collect();
    for c in &clients {
        assert_eq!(c.post(s.http("/fast")).body("w").send().await.unwrap().status(), 200);
    }

    let barrier = Arc::new(tokio::sync::Barrier::new(2 * RED));
    let mut tasks = Vec::new();
    for c in clients.clone() {
        let (url, barrier) = (s.http("/slow"), barrier.clone());
        tasks.push(tokio::spawn(async move {
            barrier.wait().await;
            let started = Instant::now();
            let res = c.post(&url).body("x").send().await.unwrap();
            let status = res.status().as_u16();
            let _ = res.bytes().await;
            (status, started.elapsed())
        }));
    }
    let mut admitted = Vec::new();
    let mut refused = Vec::new();
    for t in tasks {
        let (status, took) = t.await.unwrap();
        match status {
            200 => admitted.push(took),
            503 => refused.push(took),
            other => panic!("unexpected status {other}"),
        }
    }
    admitted.sort_unstable();
    refused.sort_unstable();
    println!(
        "admitted {} (p50 {:?}, p99 {:?}), refused {} (p50 {:?}, p99 {:?}, max {:?})",
        admitted.len(),
        percentile(&admitted, 50.0),
        percentile(&admitted, 99.0),
        refused.len(),
        percentile(&refused, 50.0),
        percentile(&refused, 99.0),
        refused.last().unwrap()
    );
    assert!(admitted.len() <= RED, "never more than red admitted");
    assert_eq!(admitted.len() + refused.len(), 2 * RED);
    assert!(refused.len() >= RED, "the excess is refused");
    assert!(percentile(&admitted, 99.0) < Duration::from_millis(BUDGET_MS), "admitted p99 within the budget");
    assert!(percentile(&refused, 99.0) < Duration::from_millis(5), "refusals are immediate (client-observed)");

    // Admission resumes once the load is gone.
    let c = &clients[0];
    assert_eq!(c.post(s.http("/slow")).body("y").send().await.unwrap().status(), 200);
    drop(clients);
    s.stop().await;
}

#[tokio::test]
async fn the_admin_api() {
    let engine = Arc::new(Engine::new(registry(), &plans(&[("p", "-a", 0, 1000)])).unwrap());
    let calls = Arc::new(AtomicUsize::new(0));
    let reloader: Reloader = {
        let (engine, calls) = (engine.clone(), calls.clone());
        Arc::new(move || {
            if calls.fetch_add(1, Ordering::SeqCst) == 1 {
                return Err("plan `p`: something is wrong\nplan `p`: and something else".into());
            }
            engine.reload(&plans(&[("p", "-b", 0, 1000)])).map_err(|d| d.to_string())
        })
    };
    let s = start(
        "[[http]]\naddr = \"127.0.0.1:0\"\nroutes = { \"/p\" = \"p\" }\n[admin]\naddr = \"127.0.0.1:0\"\n",
        engine,
        Hooks { reloader: Some(reloader), metrics: None },
    )
    .await;
    let c = reqwest::Client::new();
    let get = |path: &str| c.get(s.admin(path)).send();

    assert_eq!(get("/healthz").await.unwrap().text().await.unwrap(), "ok");
    assert_eq!(get("/readyz").await.unwrap().status(), 200);
    assert_eq!(get("/metrics").await.unwrap().status(), 404, "no recorder in this test");
    assert_eq!(c.post(s.admin("/admin/rollback")).send().await.unwrap().status(), 409);

    let res = c.post(s.admin("/admin/reload")).send().await.unwrap();
    assert_eq!(res.text().await.unwrap(), r#"{"version":2}"#);
    let res = c.post(s.admin("/admin/reload")).send().await.unwrap();
    assert_eq!(res.status(), 400);
    let errors: serde_json::Value = serde_json::from_str(&res.text().await.unwrap()).unwrap();
    assert_eq!(errors["errors"].as_array().unwrap().len(), 2);

    let snap: serde_json::Value =
        serde_json::from_str(&get("/admin/snapshot").await.unwrap().text().await.unwrap()).unwrap();
    assert_eq!(snap["version"], 2);
    assert_eq!(snap["plans"], serde_json::json!(["p"]));
    assert_eq!(snap["history"], serde_json::json!([1]));
    assert_eq!(c.post(s.http("/p")).body("x").send().await.unwrap().text().await.unwrap(), "x-b");

    let res = c.post(s.admin("/admin/rollback")).send().await.unwrap();
    assert_eq!(res.text().await.unwrap(), r#"{"version":1}"#);
    assert_eq!(c.post(s.http("/p")).body("x").send().await.unwrap().text().await.unwrap(), "x-a");
    drop(c);
    s.stop().await;
}

#[tokio::test]
async fn the_admin_allowlist_refuses_other_addresses() {
    let engine = Arc::new(Engine::new(registry(), &plans(&[("p", "", 0, 1000)])).unwrap());
    let s = start(
        "[[http]]\naddr = \"127.0.0.1:0\"\nroutes = { \"/p\" = \"p\" }\n\
         [admin]\naddr = \"127.0.0.1:0\"\nallow = [\"10.0.0.0/8\"]\n",
        engine,
        Hooks::default(),
    )
    .await;
    let res = reqwest::get(s.admin("/healthz")).await.unwrap();
    assert_eq!(res.status(), 403);
    s.stop().await;
}

#[tokio::test]
async fn the_tcp_adapter_pipelines_in_order_and_maps_statuses() {
    let engine = Arc::new(Engine::new(registry(), &plans(&[("p", "!", 0, 1000)])).unwrap());
    let s =
        start("[[tcp]]\naddr = \"127.0.0.1:0\"\nplan = \"p\"\nmax_frame_bytes = 64\n", engine, Hooks::default()).await;
    let mut conn = tokio::net::TcpStream::connect(s.addrs.tcp.unwrap()).await.unwrap();

    // Three pipelined requests; the first is slowest, yet replies keep request order.
    send_frame(&mut conn, b"60|one").await;
    send_frame(&mut conn, b"10|two").await;
    send_frame(&mut conn, b"three").await;
    assert_eq!(read_frame(&mut conn).await.unwrap(), (0, Bytes::from("one!")));
    assert_eq!(read_frame(&mut conn).await.unwrap(), (0, Bytes::from("two!")));
    assert_eq!(read_frame(&mut conn).await.unwrap(), (0, Bytes::from("three!")));

    send_frame(&mut conn, b"bad").await;
    assert_eq!(read_frame(&mut conn).await.unwrap(), (1, Bytes::from("bad input")));

    // An oversize frame closes the connection.
    send_frame(&mut conn, &[b'x'; 100]).await;
    assert!(read_frame(&mut conn).await.is_none());
    drop(conn);
    s.stop().await;
}

#[tokio::test]
async fn shutdown_lets_in_flight_requests_finish_and_flips_readiness() {
    let engine = Arc::new(Engine::new(registry(), &plans(&[("p", "", 0, 5000)])).unwrap());
    let s = start(
        "[[http]]\naddr = \"127.0.0.1:0\"\nroutes = { \"/p\" = \"p\" }\n[admin]\naddr = \"127.0.0.1:0\"\n\
         [shutdown]\ndrain_timeout_ms = 3000\n",
        engine,
        Hooks::default(),
    )
    .await;
    let c = reqwest::Client::new();
    let slow = tokio::spawn({
        let (c, url) = (c.clone(), s.http("/p"));
        async move { c.post(url).body("300|late").send().await.unwrap().text().await.unwrap() }
    });
    tokio::time::sleep(Duration::from_millis(50)).await;
    s.shutdown.cancel();
    tokio::time::sleep(Duration::from_millis(20)).await;
    assert_eq!(c.get(s.admin("/readyz")).send().await.unwrap().status(), 503, "not ready while draining");
    assert_eq!(slow.await.unwrap(), "late", "the in-flight request finished");
    drop(c);
    s.finish().await;
}

#[tokio::test]
async fn the_drain_timeout_bounds_shutdown() {
    let engine = Arc::new(Engine::new(registry(), &plans(&[("p", "", 0, 60_000)])).unwrap());
    let s = start(
        "[[http]]\naddr = \"127.0.0.1:0\"\nroutes = { \"/p\" = \"p\" }\n[shutdown]\ndrain_timeout_ms = 200\n",
        engine,
        Hooks::default(),
    )
    .await;
    let c = reqwest::Client::new();
    let stuck = tokio::spawn({
        let (c, url) = (c.clone(), s.http("/p"));
        async move { c.post(url).body("30000|never").send().await }
    });
    tokio::time::sleep(Duration::from_millis(50)).await;
    let started = Instant::now();
    s.shutdown.cancel();
    let res = stuck.await.unwrap().unwrap();
    assert_eq!(res.status(), 503, "the stuck request was cut off");
    assert_eq!(res.text().await.unwrap(), "shutting down");
    drop(c);
    s.finish().await;
    assert!(started.elapsed() < Duration::from_millis(1000), "{:?}", started.elapsed());
}

#[tokio::test]
async fn http_routes_accept_get_and_pass_the_query_string() {
    let engine = Arc::new(Engine::new(registry(), &plans(&[("p", "-a", 0, 1000)])).unwrap());
    let s = start("[[http]]\naddr = \"127.0.0.1:0\"\nroutes = { \"/p\" = \"p\" }\n", engine, Hooks::default()).await;
    let client = reqwest::Client::new();

    let res = client.get(s.http("/p?user=7&event=click")).send().await.unwrap();
    assert_eq!(res.status(), 200);
    assert_eq!(res.text().await.unwrap(), "user=7&event=click-a");

    let res = client.post(s.http("/p?ignored=1")).body("body").send().await.unwrap();
    assert_eq!(res.text().await.unwrap(), "body-a", "a POST body wins over the query");

    let res = client.put(s.http("/p")).body("x").send().await.unwrap();
    assert_eq!(res.status(), 405);
    drop(client);
    s.stop().await;
}
