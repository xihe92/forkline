#!/usr/bin/env python3
"""Print Markdown tables from benchmark result files.
    scripts/report.py results/run-*.json
"""

import json
import sys

LABELS = {
    "forkline": "Forkline",
    "forkline-4p": "Forkline, 4 producers",
    "spring": "Spring MVC",
    "spring-vt": "Spring MVC + virtual threads",
}


def table(headers, rows):
    out = ["| " + " | ".join(headers) + " |", "|" + "|".join("---:" if i else "---" for i in range(len(headers))) + "|"]
    out += ["| " + " | ".join(str(c) for c in r) + " |" for r in rows]
    return "\n".join(out)


def in_kafka(r, mode):
    if mode != "async":
        return "yes" if r["kafka_matches_ok"] and not r["errors"] else f"NO {r['errors']}"
    # Async: Kafka must hold exactly what the service accepted and the broker
    # acknowledged. oha can lose a few replies to connection errors; those
    # requests were still accepted, so they are noted, not failed.
    ok = r["kafka_records"] == r["acked"] == r["accepted"] and r["failed"] == 0
    lost = r["accepted"] - r["ok"] if r["accepted"] is not None else 0
    if not ok:
        return f"NO (kafka {r['kafka_records']}, acked {r['acked']}, accepted {r['accepted']})"
    return f"yes; oha lost {lost} replies" if lost else "yes"


def report(path):
    d = json.load(open(path))
    m = d["machine"]
    k = d.get("kafka", {"brokers": 1, "partitions": 6, "replication_factor": 1})
    mode = d.get("mode", "wait")
    print(
        f"## {k['brokers']} broker(s), {k['partitions']} partitions, {k['replication_factor']} replica(s), "
        f"linger.ms = {d['linger_ms']}, {'fire and forget' if mode == 'async' else 'wait for ack'}  ({path})\n"
    )
    print(f"{m['cpu']}, {m['cores']} cores, {m['memory_gb']} GB, macOS {m['os']}; Spring on {m['java']}\n")
    print(
        table(
            ["Target", "Startup (s)", "Idle RSS (MB)", "RSS after run (MB)"],
            [
                [LABELS[t["target"]], t["startup_seconds"], t["idle_rss_mb"], t["after_rss_mb"]]
                for t in d["targets"]
            ],
        )
    )
    scenarios = [r["scenario"] for r in d["targets"][0]["runs"]]
    for s in scenarios:
        rows = []
        for t in d["targets"]:
            r = next(r for r in t["runs"] if r["scenario"] == s)
            lat = r["latency_ms"]
            rows.append(
                [
                    LABELS[t["target"]],
                    f"{r['rps']:,.0f}",
                    lat["p50"],
                    lat["p99"],
                    lat["p99.9"],
                    r["cpu_cores"],
                    r["cpu_us_per_request"],
                    r["rss_mb_max"],
                    r.get("records_per_request") or "–",
                    in_kafka(r, mode),
                ]
            )
        first = d["targets"][0]["runs"][[x["scenario"] for x in d["targets"][0]["runs"]].index(s)]
        title = (
            f"{first['rate']:,} req/s offered, {first['connections']} connections"
            if first["rate"]
            else f"{first['connections']} connections, as fast as possible"
        )
        print(f"\n### {s}: {title}\n")
        print(
            table(
                [
                    "Target",
                    "req/s",
                    "p50 ms",
                    "p99 ms",
                    "p99.9 ms",
                    "CPU cores",
                    "CPU µs/req",
                    "Peak RSS MB",
                    "Records/produce req",
                    "All in Kafka",
                ],
                rows,
            )
        )
        if mode == "async":
            print("\nBacklog and drain:\n")
            print(table(DRAIN_HEADERS, [drain_row(t, s) for t in d["targets"]]))
    print()


DRAIN_HEADERS = [
    "Target",
    "Acked/s",
    "Peak backlog",
    "Drain s",
    "Accept CPU µs/req",
    "Drain CPU s",
    "RSS drained MB",
    "RSS +30 s MB",
    "Heap used drained / +30 s MB",
    "Broker CPU cores",
    "Failed",
]


def drain_row(t, s):
    r = next(r for r in t["runs"] if r["scenario"] == s)

    def heap(h):
        return f"{h[0]:,.0f}" if h else "–"

    heaps = (
        f"{heap(r.get('heap_mb_drained'))} / {heap(r.get('heap_mb_settled'))}" if "heap_mb_drained" in r else "–"
    )
    note = []
    if r.get("safety_stop"):
        note.append("stopped")
    if r.get("drain_timed_out"):
        note.append("drain timed out")
    return [
        LABELS[t["target"]],
        f"{r['acked_per_second'] or 0:,.0f}",
        f"{r['peak_pending']:,}",
        r["drain_seconds"],
        r["load_cpu_us_per_request"],
        r["drain_cpu_seconds"],
        r["rss_mb_drained"],
        r.get("rss_mb_settled", "–"),
        heaps,
        r["kafka_cpu_cores"],
        f"{r['failed']}" + (f" ({', '.join(note)})" if note else ""),
    ]


for p in sys.argv[1:]:
    report(p)
