//! The real binary: `check` and `serve` exit codes, reload on SIGHUP and on
//! file change, rejected reloads, and a clean exit on SIGTERM.
#![cfg(unix)]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, missing_docs)]

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::sync::mpsc::{Receiver, channel};
use std::time::{Duration, Instant};

const BIN: &str = env!("CARGO_BIN_EXE_forkline-echo");

struct Dir(PathBuf);
impl Dir {
    fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("forkline-cli-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }
    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
    fn write(&self, name: &str, text: &str) -> PathBuf {
        let path = self.path(name);
        // Write then rename, so a watcher never sees half a file.
        let tmp = self.path(&format!("{name}.tmp"));
        std::fs::write(&tmp, text).unwrap();
        std::fs::rename(&tmp, &path).unwrap();
        path
    }
}
impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn plans(name: &str, prefix: &str) -> String {
    format!(
        "[plans.{name}]\nroot = \"decode\"\nterminal = \"error\"\nlinks = [\"decode -> echo\", \"echo -> encode\"]\n\
         [plans.{name}.nodes]\ndecode = {{ module = \"BodyDecoder\" }}\n\
         echo = {{ module = \"Echo\", config = {{ prefix = \"{prefix}\" }} }}\n\
         encode = {{ module = \"ReplyEncoder\" }}\nerror = {{ module = \"ErrorReply\" }}\n"
    )
}

fn server_toml(watch_ms: u64) -> String {
    format!(
        "[[http]]\naddr = \"127.0.0.1:0\"\nroutes = {{ \"/echo\" = \"echo\" }}\n\
         [reload]\nwatch_interval_ms = {watch_ms}\n[telemetry]\nlog_format = \"json\"\nfilter = \"info\"\n"
    )
}

fn run(args: &[&str]) -> Output {
    Command::new(BIN).args(args).env_remove("RUST_LOG").output().unwrap()
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

/// A running `serve` process and its log lines.
struct Server {
    child: Child,
    logs: Receiver<String>,
    seen: Vec<String>,
    url: String,
}

impl Server {
    fn start(config: &Path, plans: &Path) -> Self {
        let mut child = Command::new(BIN)
            .args(["serve", "--config"])
            .arg(config)
            .arg("--plans")
            .arg(plans)
            .env_remove("RUST_LOG")
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap();
        let (tx, logs) = channel();
        let stdout = child.stdout.take().unwrap();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if tx.send(line).is_err() {
                    break;
                }
            }
        });
        let mut server = Self { child, logs, seen: Vec::new(), url: String::new() };
        let line = server.wait_for_log("http listening");
        let json: serde_json::Value = serde_json::from_str(&line).unwrap();
        server.url = format!("http://{}/echo", json["fields"]["addr"].as_str().unwrap());
        server
    }

    /// The next log line (from now on) whose message contains `needle`.
    fn wait_for_log(&mut self, needle: &str) -> String {
        let until = Instant::now() + Duration::from_secs(10);
        while let Some(left) = until.checked_duration_since(Instant::now()) {
            match self.logs.recv_timeout(left) {
                Ok(line) => {
                    self.seen.push(line.clone());
                    if line.contains(needle) {
                        return line;
                    }
                }
                Err(_) => break,
            }
        }
        panic!("no log line with {needle:?}; saw:\n{}", self.seen.join("\n"));
    }

    fn signal(&self, name: &str) {
        let status = Command::new("kill").arg(format!("-{name}")).arg(self.child.id().to_string()).status().unwrap();
        assert!(status.success());
    }

    async fn echo(&self) -> String {
        reqwest::Client::new().post(&self.url).body("hi").send().await.unwrap().text().await.unwrap()
    }

    /// Wait until the reply is `want` (a reload is asynchronous).
    async fn wait_for_reply(&self, want: &str) {
        let until = Instant::now() + Duration::from_secs(10);
        loop {
            let got = self.echo().await;
            if got == want {
                return;
            }
            assert!(Instant::now() < until, "reply stayed {got:?}, wanted {want:?}");
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    fn stop(mut self) -> (std::process::ExitStatus, Vec<String>) {
        self.signal("TERM");
        let status = self.child.wait().unwrap();
        self.seen.extend(self.logs.try_iter());
        (status, std::mem::take(&mut self.seen))
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
    }
}

#[test]
fn check_exits_zero_on_valid_plans_and_one_on_invalid() {
    let dir = Dir::new("check");
    let good = dir.write("good.toml", &plans("echo", ""));
    let out = run(&["check", "--plans", good.to_str().unwrap()]);
    assert!(out.status.success(), "{}", text(&out.stderr));
    assert_eq!(text(&out.stdout), format!("ok: {} (1 plans, 0 services)\n", good.display()));

    let bad = dir.write("bad.toml", &plans("echo", "").replace("\"Echo\"", "\"Nope\""));
    let out = run(&["check", "--plans", bad.to_str().unwrap()]);
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(text(&out.stderr), "error: plan `echo`, node `echo`: module `Nope` is not registered\n");

    let missing = dir.path("missing.toml");
    let out = run(&["check", "--plans", missing.to_str().unwrap()]);
    assert_eq!(out.status.code(), Some(1));
    assert!(text(&out.stderr).starts_with("error: "), "{}", text(&out.stderr));
}

#[test]
fn serve_exits_one_when_it_cannot_start() {
    let dir = Dir::new("nostart");
    let good_plans = dir.write("plans.toml", &plans("echo", ""));
    let config = dir.write("server.toml", &server_toml(0));

    let out = run(&["serve", "--config", dir.path("missing.toml").to_str().unwrap()]);
    assert_eq!(out.status.code(), Some(1));
    assert!(text(&out.stderr).starts_with("error: cannot read "), "{}", text(&out.stderr));

    let bad_plans = dir.write("bad.toml", &plans("echo", "").replace("\"Echo\"", "\"Nope\""));
    let out = run(&["serve", "--config", config.to_str().unwrap(), "--plans", bad_plans.to_str().unwrap()]);
    assert_eq!(out.status.code(), Some(1));
    assert!(text(&out.stdout).contains("module `Nope` is not registered"), "{}", text(&out.stdout));

    let other_plan = dir.write("other.toml", &plans("other", ""));
    let out = run(&["serve", "--config", config.to_str().unwrap(), "--plans", other_plan.to_str().unwrap()]);
    assert_eq!(out.status.code(), Some(1), "a route naming an unknown plan");
    assert!(text(&out.stdout).contains("names unknown plan `echo`"), "{}", text(&out.stdout));

    let busy = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let taken = server_toml(0).replace("127.0.0.1:0", &busy.local_addr().unwrap().to_string());
    let config = dir.write("taken.toml", &taken);
    let out = run(&["serve", "--config", config.to_str().unwrap(), "--plans", good_plans.to_str().unwrap()]);
    assert_eq!(out.status.code(), Some(1), "the port is taken");
    assert!(text(&out.stdout).contains("cannot start"), "{}", text(&out.stdout));
}

#[tokio::test]
async fn sighup_reloads_rejects_bad_configs_and_sigterm_stops_cleanly() {
    let dir = Dir::new("sighup");
    let plans_path = dir.write("plans.toml", &plans("echo", "v1: "));
    let mut s = Server::start(&dir.write("server.toml", &server_toml(0)), &plans_path);
    assert_eq!(s.echo().await, "v1: hi");

    dir.write("plans.toml", &plans("echo", "v2: "));
    s.signal("HUP");
    s.wait_for_log("reloaded");
    s.wait_for_reply("v2: hi").await;

    // An invalid file is rejected and the running plans stay.
    dir.write("plans.toml", &plans("echo", "").replace("\"Echo\"", "\"Nope\""));
    s.signal("HUP");
    assert!(s.wait_for_log("reload rejected:").contains("module `Nope` is not registered"));
    assert_eq!(s.echo().await, "v2: hi");

    // So is a valid file that drops the plan the route serves.
    dir.write("plans.toml", &plans("renamed", ""));
    s.signal("HUP");
    assert!(s.wait_for_log("reload rejected:").contains("is served by a listener"));
    assert_eq!(s.echo().await, "v2: hi");

    let (status, logs) = s.stop();
    assert!(status.success(), "{status:?}");
    assert!(logs.iter().any(|l| l.contains("\"stopped\"")), "{}", logs.join("\n"));
}

#[tokio::test]
async fn a_changed_file_is_reloaded_when_watching() {
    let dir = Dir::new("watch");
    let plans_path = dir.write("plans.toml", &plans("echo", "before: "));
    let mut s = Server::start(&dir.write("server.toml", &server_toml(100)), &plans_path);
    assert_eq!(s.echo().await, "before: hi");

    dir.write("plans.toml", &plans("echo", "after: "));
    assert!(s.wait_for_log("reloaded").contains("file change"));
    s.wait_for_reply("after: hi").await;

    let (status, _) = s.stop();
    assert!(status.success(), "{status:?}");
}
