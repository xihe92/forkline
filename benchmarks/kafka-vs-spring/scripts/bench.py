#!/usr/bin/env python3
"""Benchmark the Forkline and Spring MVC Kafka-publishing services.

For each target: start it, record startup time and idle memory, warm up, then
run fixed-rate scenarios (same load for every target: compare latency and CPU)
and saturation scenarios (fixed connections, no rate limit: compare
throughput). For every scenario it records throughput, latency percentiles,
the service's CPU time and RSS, the broker's CPU time, and checks that the
number of records Kafka gained equals the number of 200 responses.

With --mode async the services reply before Kafka acknowledges (fire and
forget). Each scenario then also has a drain phase: after oha stops, the runner
polls the service's /stats until no record is pending, and records the drain
time, the peak backlog, memory during and after the drain, and checks Kafka's
offsets against the acknowledged count. A safety stop interrupts oha if the
service passes 10 GB RSS or the machine drops below 1.5 GB available memory.

Needs Kafka running (scripts/kafka.sh start), oha, and both services built.
    scripts/bench.py [--targets forkline,spring,spring-vt] [--quick] [--mode wait|async]
"""

import argparse
import json
import os
import re
import signal
import subprocess
import sys
import threading
import time
import urllib.request
from datetime import datetime
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
TARGET = ROOT / "target"
RESULTS = ROOT / "results"
JAVA = os.environ.get("BENCH_JAVA", "/opt/homebrew/opt/openjdk@25/bin/java")
OHA = os.environ.get("OHA", str(Path.home() / ".cargo/bin/oha"))
PATH_REGEX = (
    r"/publish\?user_id=[1-9][0-9]{0,6}&event=(click|view|buy|share)"
    r"&amount=[1-9][0-9]{0,2}[.][0-9]{2}&currency=(USD|EUR|JPY)"
)

SPRING_COMMON = [
    JAVA,
    "-jar",
    str(ROOT / "spring-svc/target/spring-svc.jar"),
    # Tomcat closes a keep-alive connection after 100 requests by default; on
    # one machine the reconnects exhaust client ports, so keep connections open
    # like hyper does.
    "--server.tomcat.max-keep-alive-requests=-1",
]

TARGETS = {
    "forkline": {
        "port": 8081,
        "cmd": [
            str(ROOT / "forkline-svc/target/release/forkline-kafka-svc"),
            "serve",
            "--config",
            "server.toml",
            "--plans",
            "plans.toml",
        ],
        "cwd": ROOT / "forkline-svc",
    },
    # The same service with four librdkafka producers (records spread by key):
    # shows how much of the single-producer ceiling is the client's.
    "forkline-4p": {
        "port": 8081,
        "cmd": [
            str(ROOT / "forkline-svc/target/release/forkline-kafka-svc"),
            "serve",
            "--config",
            "server.toml",
            "--plans",
            "plans.toml",
        ],
        "cwd": ROOT / "forkline-svc",
        "env": {"KAFKA_PRODUCERS": "4"},
    },
    "spring": {"port": 8082, "cmd": SPRING_COMMON, "cwd": ROOT},
    "spring-vt": {
        "port": 8082,
        "cmd": SPRING_COMMON + ["--spring.threads.virtual.enabled=true"],
        "cwd": ROOT,
    },
}

FIXED_RATES = [1_000, 5_000, 10_000, 20_000]
FIXED_CONNECTIONS = 256
SATURATION = [64, 256, 1024]
# Async mode only: one offered rate above what Kafka acknowledges, the same for
# every target, so the backlogs are comparable.
OVERLOAD_RATE = 150_000
OVERLOAD_CONNECTIONS = 1024

# Async mode settings per target (see application-async.properties).
ASYNC_ENV = {"PUBLISH_MODE": "async"}
SPRING_ASYNC_JVM = ["-Xmx8g"]
SPRING_ASYNC_ARGS = ["--spring.profiles.active=async"]
JCMD = str(Path(JAVA).with_name("jcmd"))

# Safety stop: protects the machine, not the service.
SAFETY_RSS_MB = 10 * 1024
SAFETY_AVAILABLE_MB = 1536
DRAIN_TIMEOUT_S = 600
# A drain longer than this counts as a backlog: wait for memory to settle.
BACKLOG_DRAIN_S = 1.0
SETTLE_S = 30


def log(msg):
    print(f"[{datetime.now():%H:%M:%S}] {msg}", flush=True)


def cpu_seconds(pid):
    """Cumulative user+system CPU time of a process, from ps."""
    out = subprocess.run(["ps", "-o", "time=", "-p", str(pid)], capture_output=True, text=True).stdout.strip()
    if not out:
        return None
    days, _, rest = out.rpartition("-")
    parts = [float(p) for p in rest.split(":")]
    secs = 0.0
    for p in parts:
        secs = secs * 60 + p
    return secs + (int(days) * 86400 if days else 0)


def rss_mb(pid):
    out = subprocess.run(["ps", "-o", "rss=", "-p", str(pid)], capture_output=True, text=True).stdout.strip()
    return int(out) / 1024 if out else None


def available_mb():
    """Memory macOS considers free, reclaimable memory included: the
    kern.memorystatus_level percentage (what `memory_pressure` reports) of RAM."""

    def sysctl(k):
        return int(subprocess.run(["sysctl", "-n", k], capture_output=True, text=True).stdout.strip())

    return sysctl("kern.memorystatus_level") / 100 * sysctl("hw.memsize") / 2**20


def service_stats(port, timeout=2):
    """The service's /stats JSON, or None if it didn't answer in time."""
    try:
        with urllib.request.urlopen(f"http://127.0.0.1:{port}/stats", timeout=timeout) as r:
            return json.loads(r.read())
    except Exception:
        return None


def java_heap_mb(pid):
    """(used, committed) heap in MB, from jcmd GC.heap_info; None if not a JVM."""
    out = subprocess.run([JCMD, str(pid), "GC.heap_info"], capture_output=True, text=True).stdout
    m = re.search(r"committed (\d+)K, used (\d+)K", out)
    return (round(int(m.group(2)) / 1024, 1), round(int(m.group(1)) / 1024, 1)) if m else None


class RssSampler(threading.Thread):
    """Samples RSS every period. With a port it also polls /stats for the peak
    backlog, and with an oha process set it applies the safety stop."""

    def __init__(self, pid, period=0.5, port=None):
        super().__init__(daemon=True)
        self.pid, self.period, self.port = pid, period, port
        self.samples, self.running = [], True
        self.peak_pending, self.oha, self.stopped = 0, None, None

    def run(self):
        while self.running:
            r = rss_mb(self.pid)
            if r is not None:
                self.samples.append(r)
            if self.port is not None:
                s = service_stats(self.port, timeout=self.period)
                if s:
                    self.peak_pending = max(self.peak_pending, s["pending"])
                self.check_safety(r)
            time.sleep(self.period)

    def check_safety(self, rss):
        if self.stopped or self.oha is None or self.oha.poll() is not None:
            return
        avail = available_mb()
        if (rss or 0) > SAFETY_RSS_MB or avail < SAFETY_AVAILABLE_MB:
            self.stopped = f"rss {rss:.0f} MB, available {avail:.0f} MB"
            log(f"  SAFETY STOP: {self.stopped}; interrupting oha")
            self.oha.send_signal(signal.SIGINT)

    def stop(self):
        self.running = False
        self.join()
        return self.samples


def kafka_records():
    out = subprocess.run(
        ["kafka-get-offsets", "--bootstrap-server", "127.0.0.1:9092", "--topic", "events"],
        capture_output=True,
        text=True,
    ).stdout
    return sum(int(line.rsplit(":", 1)[1]) for line in out.splitlines() if line.startswith("events:"))


def kafka_pids():
    return [int(p.read_text().strip()) for p in sorted(TARGET.glob("kafka-*.pid"))]


def kafka_cpu_seconds():
    return sum(cpu_seconds(p) or 0.0 for p in kafka_pids())


def cluster():
    """The cluster's shape, as scripts/kafka.sh wrote it."""
    try:
        return json.loads((TARGET / "kafka-cluster.json").read_text())
    except (OSError, ValueError):
        return {"brokers": 1, "partitions": 6, "replication_factor": 1, "jmx_ports": []}


PRODUCE_METRIC = "kafka.network:type=RequestMetrics,name=RequestsPerSec,request=Produce,version=*"


def produce_requests():
    """Produce requests the brokers have handled so far, summed over every
    broker, or None without JMX (KAFKA_JMX_PORT when starting Kafka)."""
    ports = cluster().get("jmx_ports") or []
    if not ports:
        return None
    procs = [
        subprocess.Popen(
            [
                "kafka-run-class",
                "org.apache.kafka.tools.JmxTool",
                "--jmx-url",
                f"service:jmx:rmi:///jndi/rmi://127.0.0.1:{port}/jmxrmi",
                "--one-time",
                "true",
                "--report-format",
                "properties",
                "--attributes",
                "Count",
                "--object-name",
                PRODUCE_METRIC,
            ],
            stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL,
            text=True,
        )
        for port in ports
    ]
    total = 0.0
    for p in procs:
        out, _ = p.communicate()
        total += sum(float(line.rsplit("=", 1)[1]) for line in out.splitlines() if ":Count=" in line)
    return total


def oha(port, seconds, connections, rate=None, sampler=None):
    cmd = [
        OHA,
        "--no-tui",
        "-z",
        f"{seconds}s",
        "-c",
        str(connections),
        "-w",  # let in-flight requests finish, so every sent request is counted
        "--output-format",
        "json",
        "--rand-regex-url",
        f"http://127.0.0.1:{port}{PATH_REGEX}",
    ]
    if rate:
        cmd += ["-q", str(rate), "--latency-correction"]
    proc = subprocess.Popen(cmd, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
    if sampler is not None:
        sampler.oha = proc
    out, err = proc.communicate()
    if proc.returncode != 0 and not (sampler and sampler.stopped and out.strip()):
        raise RuntimeError(f"oha failed: {err}")
    return json.loads(out)


def drain(port):
    """Poll /stats until nothing is pending. Returns (seconds, timed_out)."""
    t0 = time.monotonic()
    while time.monotonic() - t0 < DRAIN_TIMEOUT_S:
        s = service_stats(port)
        if s and s["pending"] == 0:
            return time.monotonic() - t0, False
        time.sleep(0.1)
    return time.monotonic() - t0, True


def delta(after, before, key):
    return after[key] - before[key]


def scenario(name, pid, port, seconds, connections, rate=None, mode="wait", java=False):
    before = kafka_records()
    preq0 = produce_requests()
    stats0 = service_stats(port) if mode == "async" else None
    cpu0, kcpu0 = cpu_seconds(pid), kafka_cpu_seconds()
    sampler = RssSampler(pid, port=port if mode == "async" else None)
    sampler.start()
    t0 = time.monotonic()
    d = oha(port, seconds, connections, rate, sampler if mode == "async" else None)
    wall = time.monotonic() - t0
    drain_s, drain_timeout = None, False
    if mode == "async":
        cpu_load = cpu_seconds(pid)
        drain_s, drain_timeout = drain(port)
    rss = sampler.stop()
    cpu1, kcpu1 = cpu_seconds(pid), kafka_cpu_seconds()
    time.sleep(1)
    after = kafka_records()
    preq1 = produce_requests()
    preqs = preq1 - preq0 if preq0 is not None and preq1 is not None else None
    codes = {str(k): v for k, v in d["statusCodeDistribution"].items()}
    ok = codes.get("200", 0)
    total = sum(codes.values())
    cpu = cpu1 - cpu0
    # Broker CPU covers the drain too, when there is one.
    busy = wall + (drain_s or 0)
    lat = {k: round(v * 1000, 3) for k, v in d["latencyPercentiles"].items()}
    result = {
        "scenario": name,
        "connections": connections,
        "rate": rate,
        "seconds": round(wall, 2),
        "requests": total,
        "ok": ok,
        "errors": d.get("errorDistribution", {}),
        "status": codes,
        "rps": round(ok / wall, 1),
        "latency_ms": lat,
        "latency_mean_ms": round(d["summary"]["average"] * 1000, 3),
        "cpu_seconds": round(cpu, 2),
        "cpu_cores": round(cpu / wall, 2),
        "cpu_us_per_request": round(cpu / ok * 1e6, 1) if ok else None,
        "rss_mb_max": round(max(rss), 1) if rss else None,
        "rss_mb_avg": round(sum(rss) / len(rss), 1) if rss else None,
        "kafka_cpu_cores": round((kcpu1 - kcpu0) / busy, 2),
        "kafka_records": after - before,
        "kafka_matches_ok": after - before == ok,
        "produce_requests": int(preqs) if preqs is not None else None,
        "records_per_request": round((after - before) / preqs, 1) if preqs else None,
    }
    if mode == "async":
        stats1 = service_stats(port) or {}
        acked = delta(stats1, stats0, "acked") if stats1 and stats0 else None
        failed = delta(stats1, stats0, "failed") if stats1 and stats0 else None
        accepted = delta(stats1, stats0, "accepted") if stats1 and stats0 else None
        load_cpu = cpu_load - cpu0
        result.update(
            {
                # In async mode a 200 means accepted; Kafka must match the
                # acknowledged count, and accepted = acked + failed = 200s.
                "kafka_matches_ok": acked is not None and after - before == acked and accepted == ok == acked + failed,
                "accepted": accepted,
                "acked": acked,
                "failed": failed,
                "refused": delta(stats1, stats0, "refused") if stats1 and stats0 else None,
                "drain_seconds": round(drain_s, 2),
                "drain_timed_out": drain_timeout,
                "peak_pending": sampler.peak_pending,
                "safety_stop": sampler.stopped,
                "acked_per_second": round(acked / busy, 1) if acked else None,
                "load_cpu_us_per_request": round(load_cpu / ok * 1e6, 1) if ok else None,
                "drain_cpu_seconds": round(cpu - load_cpu, 2),
                "kafka_cpu_seconds": round(kcpu1 - kcpu0, 2),
                "rss_mb_drained": round(rss_mb(pid), 1),
            }
        )
        if java:
            result["heap_mb_drained"] = java_heap_mb(pid)
        if drain_s > BACKLOG_DRAIN_S:
            time.sleep(SETTLE_S)
            result["rss_mb_settled"] = round(rss_mb(pid), 1)
            if java:
                result["heap_mb_settled"] = java_heap_mb(pid)
    log(
        f"  {name:<14} {result['rps']:>9,.0f} req/s  p50 {lat['p50']:>7.2f} ms  p99 {lat['p99']:>7.2f} ms  "
        f"p99.9 {lat['p99.9']:>8.2f} ms  cpu {result['cpu_cores']:>5.2f} cores  "
        f"{result['cpu_us_per_request'] or 0:>6.1f} µs/req  rss {result['rss_mb_max']} MB  "
        f"kafka {'ok' if result['kafka_matches_ok'] else 'MISMATCH ' + str(after - before)}"
        + (f"  {result['records_per_request']} rec/produce" if result["records_per_request"] else "")
    )
    if mode == "async":
        log(
            f"  {'':<14} drain {result['drain_seconds']:>6.2f} s  peak backlog {result['peak_pending']:,}  "
            f"acked {result['acked_per_second'] or 0:>9,.0f}/s  failed {result['failed']}  "
            f"refused {result['refused']}  rss drained {result['rss_mb_drained']} MB"
            + (f"  settled {result['rss_mb_settled']} MB" if "rss_mb_settled" in result else "")
            + (f"  heap {result['heap_mb_drained']}" if java else "")
            + (f"  STOPPED ({result['safety_stop']})" if result["safety_stop"] else "")
            + ("  DRAIN TIMED OUT" if drain_timeout else "")
        )
    time.sleep(3)
    return result


def wait_ready(port, timeout=60):
    url = f"http://127.0.0.1:{port}/publish?user_id=1&event=ready"
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        try:
            with urllib.request.urlopen(url, timeout=2) as r:
                if r.status == 200:
                    return True
        except Exception:
            time.sleep(0.05)
    return False


def run_target(name, quick, linger, mode):
    spec = TARGETS[name]
    port = spec["port"]
    cmd = list(spec["cmd"])
    java = name.startswith("spring")
    if java:
        cmd.append(f"--spring.kafka.producer.properties.linger.ms={linger}")
    env = dict(os.environ, KAFKA_LINGER_MS=str(linger), **spec.get("env", {}))
    if mode == "async":
        env.update(ASYNC_ENV)
        if java:
            cmd = cmd[:1] + SPRING_ASYNC_JVM + cmd[1:] + SPRING_ASYNC_ARGS
    logf = open(TARGET / f"{name}.log", "w")
    t0 = time.monotonic()
    proc = subprocess.Popen(cmd, cwd=spec["cwd"], env=env, stdout=logf, stderr=subprocess.STDOUT)
    try:
        if not wait_ready(port):
            raise RuntimeError(f"{name} did not become ready; see target/{name}.log")
        startup = time.monotonic() - t0
        time.sleep(5)
        idle = rss_mb(proc.pid)
        log(f"{name}: ready in {startup:.2f}s, idle RSS {idle:.1f} MB (pid {proc.pid})")
        mode_of = (service_stats(port) or {}).get("mode")
        if mode_of != mode:
            raise RuntimeError(f"{name} reports mode {mode_of!r}, expected {mode!r}")
        secs = 10 if quick else 30
        opts = {"mode": mode, "java": java}
        warm = scenario("warmup", proc.pid, port, 10 if quick else 60, 64, **opts)
        runs = []
        for rate in FIXED_RATES:
            runs.append(scenario(f"rate-{rate // 1000}k", proc.pid, port, secs, FIXED_CONNECTIONS, rate, **opts))
        if mode == "async":
            runs.append(
                scenario(
                    f"overload-{OVERLOAD_RATE // 1000}k",
                    proc.pid,
                    port,
                    secs,
                    OVERLOAD_CONNECTIONS,
                    OVERLOAD_RATE,
                    **opts,
                )
            )
        for c in SATURATION:
            runs.append(scenario(f"conns-{c}", proc.pid, port, secs, c, **opts))
        time.sleep(5)
        return {
            "target": name,
            "mode": mode,
            "startup_seconds": round(startup, 2),
            "idle_rss_mb": round(idle, 1),
            "after_rss_mb": round(rss_mb(proc.pid), 1),
            "warmup": warm,
            "runs": runs,
        }
    finally:
        proc.terminate()
        try:
            proc.wait(15)
        except subprocess.TimeoutExpired:
            proc.kill()
        logf.close()
        time.sleep(3)


def machine():
    def sysctl(k):
        return subprocess.run(["sysctl", "-n", k], capture_output=True, text=True).stdout.strip()

    java = subprocess.run([JAVA, "-version"], capture_output=True, text=True).stderr.splitlines()[0]
    return {
        "cpu": sysctl("machdep.cpu.brand_string"),
        "cores": sysctl("hw.ncpu"),
        "memory_gb": int(sysctl("hw.memsize")) // 2**30,
        "os": subprocess.run(["sw_vers", "-productVersion"], capture_output=True, text=True).stdout.strip(),
        "java": java,
    }


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--targets", default="forkline,spring,spring-vt")
    ap.add_argument("--quick", action="store_true", help="10-second scenarios, to check the setup")
    ap.add_argument("--linger", type=int, default=5, help="producer linger.ms for every target (default 5)")
    ap.add_argument(
        "--mode",
        choices=["wait", "async"],
        default="wait",
        help="wait: reply after Kafka acknowledges (default); async: reply at once, measure the drain",
    )
    args = ap.parse_args()
    TARGET.mkdir(exist_ok=True)
    RESULTS.mkdir(exist_ok=True)
    shape = cluster()
    report = {
        "started": datetime.now().isoformat(timespec="seconds"),
        "machine": machine(),
        "linger_ms": args.linger,
        "mode": args.mode,
        "kafka": {k: v for k, v in shape.items() if k != "jmx_ports"},
        "targets": [],
    }
    for name in args.targets.split(","):
        log(
            f"=== {name} ({args.mode}, linger.ms={args.linger}, {shape['brokers']} broker(s), "
            f"{shape['partitions']} partitions)"
        )
        report["targets"].append(run_target(name, args.quick, args.linger, args.mode))
    suffix = (
        f"-b{shape['brokers']}p{shape['partitions']}rf{shape['replication_factor']}-linger{args.linger}"
        + ("-async" if args.mode == "async" else "")
        + ("-quick" if args.quick else "")
    )
    out = RESULTS / f"run-{datetime.now():%Y%m%d-%H%M%S}{suffix}.json"
    out.write_text(json.dumps(report, indent=2))
    log(f"wrote {out}")


if __name__ == "__main__":
    sys.exit(main())
