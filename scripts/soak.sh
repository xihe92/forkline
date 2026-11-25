#!/usr/bin/env bash
# Soak test: run the horoscope example under steady load and sample the
# server's memory, threads and open file descriptors. Fails if, between the
# second quarter of the run (after warm-up) and the last, memory grows more
# than 10% and more than 8 MB, open file descriptors more than 10% + 16, or threads more than
# 25% + 8; if fewer than 99.9% of requests succeed; or if the server panics
# or stops before the end.
#
#   scripts/soak.sh [duration_seconds=86400] [rate_per_second=5000] [out_dir=target/soak]
#
# Needs: a release build (cargo build --release -p forkline-horoscope) and oha.
# Runs on Linux and macOS. The nightly CI run includes it for 30 minutes when
# main changed; run the 24-hour soak before a release on fixed hardware.
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
  --no-tui --output-format json http://127.0.0.1:18181/query > "$OUT/oha.json" 2> "$OUT/oha.err" &
LOAD=$!

# Linux reads /proc; macOS has neither, so it asks ps and lsof (whose count
# includes mapped files, a constant offset).
threads() {
  if [ -r "/proc/$1/status" ]; then awk '/^Threads:/ { print $2 }' "/proc/$1/status"
  else ps -M -p "$1" 2>/dev/null | tail -n +2 | wc -l | tr -d ' '; fi
}
fds() {
  if [ -d "/proc/$1/fd" ]; then ls "/proc/$1/fd" | wc -l | tr -d ' '
  else lsof -p "$1" 2>/dev/null | wc -l | tr -d ' '; fi
}

echo "elapsed_s,rss_kb,threads,fds" > "$OUT/samples.csv"
START=$(date +%s)
while kill -0 "$LOAD" 2>/dev/null; do
  if ! kill -0 "$SERVER" 2>/dev/null; then
    echo "FAIL: the server stopped during the run; see $OUT/server.log"
    exit 1
  fi
  NOW=$(( $(date +%s) - START ))
  RSS=$(ps -o rss= -p "$SERVER" | tr -d ' ')
  echo "$NOW,$RSS,$(threads "$SERVER"),$(fds "$SERVER")" >> "$OUT/samples.csv"
  sleep "$INTERVAL"
done
wait "$LOAD" || true

curl -s http://127.0.0.1:16791/metrics > "$OUT/metrics.txt"
kill -TERM "$SERVER"; wait "$SERVER" || true

python3 -I - "$OUT" <<'PY' | tee "$OUT/summary.txt"
import csv, json, sys
out = sys.argv[1]
rows = list(csv.DictReader(open(f"{out}/samples.csv")))
oha_err = open(f"{out}/oha.err", errors="replace").read().strip()
if not open(f"{out}/oha.json").read().strip():
    sys.exit(f"FAIL: oha produced no report: {oha_err or 'no error output'}")
if len(rows) < 8:
    sys.exit(f"FAIL: only {len(rows)} samples; run longer")

def quarters(column):
    values = [int(r[column]) for r in rows if r[column]]
    q = max(1, len(values) // 4)
    return values, values[q:2 * q], values[-q:]   # skip the warm-up quarter

problems = []
rss, rss_early, rss_late = quarters("rss_kb")
early, late = sum(rss_early) / len(rss_early), sum(rss_late) / len(rss_late)
growth = (late - early) / early * 100 if early else 0
print(f"rss: start {rss[0]} kB, second quarter {early:.0f} kB, last quarter {late:.0f} kB, growth {growth:+.1f}%")
# Both a share and an amount: the allocator grows in steps of a few MB, which
# alone is over 10% of a 12 MB process, while a real leak at this rate
# (100 bytes a request is 900 MB in 30 minutes) is far past both.
if growth >= 10 and late - early > 8192:
    problems.append(f"memory grew {growth:+.1f}% ({(late - early) / 1024:.1f} MB)")
for column, pct, slack in (("fds", 10, 16), ("threads", 25, 8)):
    values, first_q, last_q = quarters(column)
    limit = max(first_q) * (1 + pct / 100) + slack
    print(f"{column}: start {values[0]}, second-quarter max {max(first_q)}, last-quarter max {max(last_q)} (limit {limit:.0f})")
    if max(last_q) > limit:
        problems.append(f"{column} grew from {max(first_q)} to {max(last_q)}")

d = json.load(open(f"{out}/oha.json")); s = d["summary"]; p = d["latencyPercentiles"]
total = sum(d["statusCodeDistribution"].values())
print(f"requests {total}, {s['requestsPerSec']:.0f}/s, success {s['successRate'] * 100:.3f}%, "
      f"p50 {p['p50'] * 1e3:.2f} ms, p99 {p['p99'] * 1e3:.2f} ms, status {d['statusCodeDistribution']}")
if total == 0:
    problems.append("no requests completed")
if s["successRate"] <= 0.999:
    problems.append(f"success rate {s['successRate'] * 100:.3f}% (connection errors)")
not_ok = sum(n for code, n in d["statusCodeDistribution"].items() if not code.startswith("2"))
if total and not_ok / total > 0.001:
    problems.append(f"{not_ok} of {total} answers were not 2xx")
log = open(f"{out}/server.log", errors="replace").read()
if "panicked" in log:
    problems.append("the server log has a panic")
if problems:
    sys.exit("FAIL: " + "; ".join(problems))
print("ok")
PY
