#!/usr/bin/env bash
# Soak test: run the horoscope example under steady load and sample the
# server's memory, threads and open file descriptors. Fails if memory grows
# by more than 10% between the first and last quarter of the run.
#
#   scripts/soak.sh [duration_seconds=86400] [rate_per_second=5000] [out_dir=target/soak]
#
# Needs: a release build (cargo build --release -p forkline-horoscope) and oha.
set -euo pipefail

DURATION=${1:-86400}
RATE=${2:-5000}
OUT=${3:-target/soak}
INTERVAL=$(( DURATION / 120 > 5 ? DURATION / 120 : 5 ))
BIN=target/release
mkdir -p "$OUT"

cat > "$OUT/server.toml" <<TOML
[[http]]
addr = "127.0.0.1:18181"
routes = { "/query" = "qp1" }
[admin]
addr = "127.0.0.1:16791"
[admission]
red = 4000
green = 3000
[telemetry]
log_format = "json"
filter = "warn"
TOML

"$BIN/horoscope-backend" > "$OUT/backend.log" 2>&1 &
BACKEND=$!
trap 'kill $BACKEND $SERVER $LOAD 2>/dev/null || true' EXIT
for _ in $(seq 1 100); do curl -sf -o /dev/null http://127.0.0.1:9001/time && break; sleep 0.1; done

"$BIN/forkline-horoscope" serve --config "$OUT/server.toml" \
  --plans examples/horoscope/plans.toml --services examples/horoscope/services.toml > "$OUT/server.log" 2>&1 &
SERVER=$!
for _ in $(seq 1 100); do curl -sf -o /dev/null http://127.0.0.1:16791/healthz && break; sleep 0.1; done

oha -z "${DURATION}s" -q "$RATE" -c 64 -m POST -d '{"want":"horoscope","sign":"leo"}' \
  --no-tui --output-format json http://127.0.0.1:18181/query > "$OUT/oha.json" 2>/dev/null &
LOAD=$!

echo "elapsed_s,rss_kb,threads,fds" > "$OUT/samples.csv"
START=$(date +%s)
while kill -0 "$LOAD" 2>/dev/null; do
  NOW=$(( $(date +%s) - START ))
  RSS=$(ps -o rss= -p "$SERVER" | tr -d ' ')
  THREADS=$(ps -M -p "$SERVER" 2>/dev/null | tail -n +2 | wc -l | tr -d ' ' || echo 0)
  FDS=$(lsof -p "$SERVER" 2>/dev/null | wc -l | tr -d ' ')
  echo "$NOW,$RSS,$THREADS,$FDS" >> "$OUT/samples.csv"
  sleep "$INTERVAL"
done
wait "$LOAD" || true

curl -s http://127.0.0.1:16791/metrics > "$OUT/metrics.txt"
kill -TERM "$SERVER"; wait "$SERVER" || true

python3 - "$OUT" <<'PY'
import csv, json, sys
out = sys.argv[1]
rows = list(csv.DictReader(open(f"{out}/samples.csv")))
rss = [int(r["rss_kb"]) for r in rows if r["rss_kb"]]
q = max(1, len(rss) // 4)
first, last = sum(rss[q:2*q]) / q, sum(rss[-q:]) / q   # skip the warm-up quarter
growth = (last - first) / first * 100 if first else 0
d = json.load(open(f"{out}/oha.json")); s = d["summary"]; p = d["latencyPercentiles"]
print(f"requests/s {s['requestsPerSec']:.0f}, success {s['successRate']*100:.3f}%, p50 {p['p50']*1e3:.2f} ms, "
      f"p99 {p['p99']*1e3:.2f} ms, status {d['statusCodeDistribution']}")
print(f"rss: start {rss[0]} kB, second quarter {first:.0f} kB, last quarter {last:.0f} kB, growth {growth:+.1f}%")
print(f"fds: first {rows[0]['fds']}, last {rows[-1]['fds']}; threads: first {rows[0]['threads']}, last {rows[-1]['threads']}")
sys.exit(0 if growth < 10 and s['successRate'] > 0.999 else 1)
PY
