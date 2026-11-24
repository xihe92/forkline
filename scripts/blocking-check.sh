#!/usr/bin/env bash
# Blocking check beyond nodes: build the echo example with
# --cfg tokio_unstable, so the server's runtime times every task poll
# (adapters, connections, timers, not only nodes), put it under HTTP and TCP
# load, and fail if any poll held a worker thread longer than the limit.
#
#   scripts/blocking-check.sh [limit_ms=50] [requests=4000] [out_dir=target/blocking-check]
#
# Needs curl and python3. Run from the repository root; CI runs it nightly.
set -euo pipefail

LIMIT_MS=${1:-50}
REQUESTS=${2:-4000}
OUT=${3:-target/blocking-check}
mkdir -p "$OUT"

RUSTFLAGS="-D warnings --cfg tokio_unstable" cargo build --release -p forkline-echo --target-dir target/unstable
BIN=target/unstable/release/forkline-echo

cat > "$OUT/server.toml" <<TOML
[runtime]
slow_poll_ms = $LIMIT_MS
[[http]]
addr = "127.0.0.1:18282"
routes = { "/echo" = "echo", "/slow" = "slow" }
[[tcp]]
addr = "127.0.0.1:17282"
plan = "echo"
[admin]
addr = "127.0.0.1:16792"
[telemetry]
log_format = "text"
filter = "warn"
TOML

"$BIN" serve --config "$OUT/server.toml" --plans examples/echo/plans.toml > "$OUT/server.log" 2>&1 &
SERVER=$!
trap 'kill $SERVER 2>/dev/null || true' EXIT
for _ in $(seq 1 100); do curl -sf -o /dev/null http://127.0.0.1:16792/readyz && break; sleep 0.1; done

python3 -I - "$REQUESTS" <<'PY'
import socket, struct, sys, threading, urllib.request

total = int(sys.argv[1])
failures = []

def http(n):
    for i in range(n):
        path = "/slow" if i % 10 == 0 else "/echo"
        req = urllib.request.Request(f"http://127.0.0.1:18282{path}", data=f"req {i}".encode(), method="POST")
        with urllib.request.urlopen(req, timeout=5) as res:
            if res.status != 200:
                failures.append(f"HTTP {res.status}")

def tcp(n):
    with socket.create_connection(("127.0.0.1", 17282), timeout=5) as s:
        for i in range(n):
            body = f"frame {i}".encode()
            s.sendall(struct.pack(">I", len(body)) + body)
            head = b""
            while len(head) < 4:
                head += s.recv(4 - len(head))
            (length,) = struct.unpack(">I", head)
            reply = b""
            while len(reply) < length:
                reply += s.recv(length - len(reply))
            if reply[0] != 0:
                failures.append(f"TCP status {reply[0]}")

workers = 16
threads = [threading.Thread(target=http, args=(total // (2 * workers),)) for _ in range(workers)]
threads += [threading.Thread(target=tcp, args=(total // (2 * workers),)) for _ in range(workers)]
for t in threads:
    t.start()
for t in threads:
    t.join()
if failures:
    sys.exit(f"{len(failures)} requests failed, e.g. {failures[0]}")
print(f"{total} requests answered")
PY

METRICS=$(curl -sf http://127.0.0.1:16792/metrics)
SLOW=$(printf '%s\n' "$METRICS" | grep -E '^forkline_(task|node)_slow_polls_total' || true)
COUNT=$(printf '%s\n' "$SLOW" | awk 'NF { s += $NF } END { print s + 0 }')
if [ "$COUNT" -gt 0 ]; then
  echo "FAIL: $COUNT poll(s) held a worker thread longer than ${LIMIT_MS} ms:"
  printf '%s\n' "$SLOW" | awk 'NF && $NF > 0'
  grep -i 'slow' "$OUT/server.log" || true
  exit 1
fi
echo "ok: no poll held a worker thread longer than ${LIMIT_MS} ms"
