#!/usr/bin/env bash
# HTTP/2 conformance: run h2spec (github.com/summerwind/h2spec) against the
# echo example's HTTP listener (h2c) and gRPC listener, and fail on any
# failure not in scripts/h2spec-allow.txt.
#
#   scripts/h2spec-check.sh [out_dir=target/h2spec]
#
# Uses $H2SPEC if set; otherwise downloads the h2spec 2.6.0 Linux binary.
# Needs curl and python3. Run from the repository root; CI runs it nightly.
set -euo pipefail

OUT=${1:-target/h2spec}
mkdir -p "$OUT"
if [ -z "${H2SPEC:-}" ]; then
  curl -sSfL https://github.com/summerwind/h2spec/releases/download/v2.6.0/h2spec_linux_amd64.tar.gz \
    | tar xz -C "$OUT" h2spec
  H2SPEC="$OUT/h2spec"
fi

cargo build --release -p forkline-echo
cat > "$OUT/server.toml" <<TOML
[[http]]
addr = "127.0.0.1:18383"
routes = { "/echo" = "echo" }
[[grpc]]
addr = "127.0.0.1:18384"
methods = { Echo = "echo" }
[admin]
addr = "127.0.0.1:16793"
[telemetry]
log_format = "text"
filter = "warn"
TOML
target/release/forkline-echo serve --config "$OUT/server.toml" --plans examples/echo/plans.toml > "$OUT/server.log" 2>&1 &
SERVER=$!
trap 'kill $SERVER 2>/dev/null || true' EXIT
for _ in $(seq 1 100); do curl -sf -o /dev/null http://127.0.0.1:16793/readyz && break; sleep 0.1; done

"$H2SPEC" -h 127.0.0.1 -p 18383 -P /echo -o 2 -j "$OUT/http.xml" > "$OUT/http.txt" 2>&1 || true
"$H2SPEC" -h 127.0.0.1 -p 18384 -P /forkline.v1.Plans/Echo -o 2 -j "$OUT/grpc.xml" > "$OUT/grpc.txt" 2>&1 || true
tail -n 1 "$OUT/http.txt" | sed 's/^/http: /'
tail -n 1 "$OUT/grpc.txt" | sed 's/^/grpc: /'

python3 -I - "$OUT" scripts/h2spec-allow.txt <<'PY'
import sys
import xml.etree.ElementTree as ET

out, allow_file = sys.argv[1], sys.argv[2]
import re

lines = [line.strip() for line in open(allow_file) if line.strip() and not line.startswith("#")]
flaky = {line.removeprefix("flaky ") for line in lines if line.startswith("flaky ")}
allowed = {line.removeprefix("flaky ") for line in lines}
failed, ran = set(), set()
for listener in ("http", "grpc"):
    # h2spec writes frame payloads, NUL bytes included, into its report.
    text = re.sub(r"[\x00-\x08\x0b\x0c\x0e-\x1f]", "?", open(f"{out}/{listener}.xml", encoding="utf-8", errors="replace").read())
    for case in ET.fromstring(text).iter("testcase"):
        name = f"{listener} {case.get('package')} | {case.get('classname')}"
        ran.add(name)
        if case.find("error") is not None or case.find("failure") is not None:
            failed.add(name)
if not ran:
    sys.exit("h2spec ran no tests")
new = sorted(failed - allowed)
fixed = sorted((allowed & ran) - failed - flaky)
for name in fixed:
    print(f"now passes, remove from {allow_file}: {name}")
for name in new:
    print(f"FAIL: {name}")
if new:
    sys.exit(f"{len(new)} h2spec failure(s) not in {allow_file}; details in {out}/*.txt")
print(f"ok: {len(ran)} h2spec tests, {len(failed)} expected failure(s), no new ones")
PY
