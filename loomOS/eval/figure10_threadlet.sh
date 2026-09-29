#!/usr/bin/env bash
set -euo pipefail

if [[ "$#" -gt 2 ]]; then
    echo "Usage: $0 [figure10_threadlet.log] [result.csv]" >&2
    exit 2
fi

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
LOG_FILE="${1:-${SCRIPT_DIR}/figure10_threadlet.log}"
RESULT_FILE="${2:-}"
BOOT_LOG="${SCRIPT_DIR}/../tools/boot_fpga.log"


TMP_JSON="${SCRIPT_DIR}/../tools/tmp.json"
MCS_FDT_PATH="/home/qxh/asterinas_threadlet/tools/threadlet_intr_poll_4c.dtb"

PYTHONDONTWRITEBYTECODE=1 python3 - "${TMP_JSON}" "${MCS_FDT_PATH}" <<'PY_CONFIG'
from __future__ import annotations

import json
import re
import sys
from pathlib import Path


config_path = Path(sys.argv[1])
fdt_path = sys.argv[2]
if not config_path.is_file():
    raise SystemExit(f"[error] tmp.json not found: {config_path}")

text = config_path.read_text(encoding="utf-8")
updated, count = re.subn(
    r'("fdt_path"\s*:\s*)"[^"]*"',
    lambda match: f'{match.group(1)}"{fdt_path}"',
    text,
)
if count != 1:
    raise SystemExit(f"[error] expected one fdt_path in {config_path}, found {count}")
json.loads(updated)
if updated != text:
    config_path.write_text(updated, encoding="utf-8")
PY_CONFIG



SESSION="fpga_boot"
WINDOW="worker"
TASK="cd '${SCRIPT_DIR}/../tools' && exec ./boot_fpga_icenet.sh > ./boot_fpga.log"

: > "${BOOT_LOG}"
if tmux has-session -t "$SESSION" 2>/dev/null; then
    tmux new-window -d \
        -t "$SESSION" \
        -n "$WINDOW" \
        "bash -lc '$TASK'"
else
    tmux new-session -d \
        -s "$SESSION" \
        -n "$WINDOW" \
        "bash -lc '$TASK'"
fi

echo "————————————————————FPGA starts. Please wait for the simulation to end————————————————————"

until grep -Fq "FireSim Simulation Status" "${BOOT_LOG}"; do
    sleep 1
done

echo "————————————————————Kernel starts. Please wait for the simulation to end————————————————————"

sleep 35


echo "————————————————————Start networking tests.————————————————————"
cd "${SCRIPT_DIR}"

sudo ./figure10_bimodal_threadlet.sh
sleep 2
sudo ./figure10_heavy_threadlet.sh
sleep 2
sudo ./figure10_kv_threadlet.sh
sleep 2
sudo ./fpga-nic-test.sh  # Send some redundant packets to flush the hardware outputs.

echo "______________Test finished______________________"

sleep 3

echo "______________Stopping the FPGA simulation______________________"
"${SCRIPT_DIR}/kill_fpga.sh"

sleep 40

## Get the hardware log
"${SCRIPT_DIR}/hw_syn.sh" > "${LOG_FILE}"


if [[ -n "${RESULT_FILE}" ]]; then
    mkdir -p "$(dirname -- "${RESULT_FILE}")"
    OUTPUT_TO_FILE=1
    exec 3> "${RESULT_FILE}"
else
    OUTPUT_TO_FILE=0
    exec 3>&1
fi
PYTHONDONTWRITEBYTECODE=1 python3 - "${LOG_FILE}" "${OUTPUT_TO_FILE}" >&3 <<'PY'
from __future__ import annotations

import csv
import math
import sys
from pathlib import Path


log_path = Path(sys.argv[1]).resolve()
output_to_file = sys.argv[2] == "1"
script_dir = log_path.parent
sys.path.insert(0, str(script_dir))

import analysis_rpc  # noqa: E402


SHORT_REQ_CYCLES = 1_000
LONG_REQ_CYCLES = 100_000


def percentile(values: list[float], pct: float) -> float | None:
    if not values:
        return None
    ordered = sorted(values)
    rank = max(0, math.ceil(len(ordered) * pct) - 1)
    return ordered[rank]


def format_float(value: float | None) -> str:
    if value is None:
        return "NA"
    return f"{value:.2f}"


def format_int(value: float | int | None) -> str:
    if value is None:
        return "NA"
    return str(int(value))


def compute_p99_slowdown(
    records: list[analysis_rpc.PacketRecord],
    fanout: int,
    long_every: int,
) -> float | None:
    slowdowns: list[float] = []
    for offset, record in enumerate(records):
        latency = record.send_to_finish
        if latency is None:
            continue

        # The NIC rewrites dispatched ids after fanout. Use group-local order
        # to recover the original client request for service-time classification.
        original_request_index = offset // fanout
        service_cycles = (
            LONG_REQ_CYCLES
            if original_request_index % long_every == 0
            else SHORT_REQ_CYCLES
        )
        slowdowns.append(latency / float(service_cycles))

    return percentile(slowdowns, 0.99)


def compute_p99_tail_latency(records: list[analysis_rpc.PacketRecord]) -> int | None:
    latencies = [
        record.send_to_finish
        for record in records
        if record.send_to_finish is not None
    ]
    value = percentile([float(latency) for latency in latencies], 0.99)
    return None if value is None else int(value)


def throughput(records: list[analysis_rpc.PacketRecord]) -> float | None:
    return analysis_rpc.compute_throughput(records)[4]


def matched_count(records: list[analysis_rpc.PacketRecord]) -> int:
    return sum(1 for record in records if record.send_to_finish is not None)


with log_path.open("r", encoding="utf-8", errors="ignore") as log_file:
    lines = log_file.readlines()

send_events, finish_events, _ = analysis_rpc.parse_log_events(
    lines,
    send_stage=87,
    finish_stage=9,
    cnt_stage=21,
)
groups = analysis_rpc.build_record_groups(
    send_events,
    finish_events,
    packets_per_group=None,
    group_gap_cycles=analysis_rpc.DEFAULT_GROUP_GAP_CYCLES,
)

workloads = [
    {
        "subplot": "figure10a_bimodal",
        "group": "bimodal_test1",
        "metric": "slowdown",
        "config": "send_sleep_us=0",
        "fanout": 1,
        "long_every": 2,
    },
    {
        "subplot": "figure10a_bimodal",
        "group": "bimodal_test2",
        "metric": "slowdown",
        "config": "send_sleep_us=1500",
        "fanout": 1,
        "long_every": 2,
    },
    {
        "subplot": "figure10a_bimodal",
        "group": "bimodal_test3",
        "metric": "slowdown",
        "config": "send_sleep_us=5000",
        "fanout": 1,
        "long_every": 2,
    },
    {
        "subplot": "figure10b_heavy",
        "group": "heavy_test1",
        "metric": "slowdown",
        "config": "fanout=1,max_rpc=600",
        "fanout": 1,
        "long_every": 200,
    },
    {
        "subplot": "figure10b_heavy",
        "group": "heavy_test2",
        "metric": "slowdown",
        "config": "fanout=3,max_rpc=200",
        "fanout": 3,
        "long_every": 200,
    },
    {
        "subplot": "figure10b_heavy",
        "group": "heavy_test3",
        "metric": "slowdown",
        "config": "fanout=6,max_rpc=200",
        "fanout": 6,
        "long_every": 200,
    },
    {
        "subplot": "figure10c_kv",
        "group": "kv_test1",
        "metric": "tail_latency",
        "config": "fanout=1,max_rpc=600",
    },
    {
        "subplot": "figure10c_kv",
        "group": "kv_test2",
        "metric": "tail_latency",
        "config": "fanout=3,max_rpc=400",
    },
    {
        "subplot": "figure10c_kv",
        "group": "kv_test3",
        "metric": "tail_latency",
        "config": "fanout=6,max_rpc=200",
    },
]

if len(groups) != len(workloads):
    print(
        f"[warn] expected {len(workloads)} groups but found {len(groups)}",
        file=sys.stderr,
    )

header = [
    "subplot",
    "group",
    "config",
    "requests",
    "matched",
    "p99_slowdown",
    "p99_tail_latency_cycles",
    "throughput_pkt_per_ms",
]
writer = None
if output_to_file:
    writer = csv.writer(sys.stdout, lineterminator="\n")
    writer.writerow(header)
else:
    print("==== Figure 10 Threadlet Summary ====")
    print(f"log_path: {log_path}")
    print(
        "subplot,group,config,requests,matched,"
        "p99_slowdown,p99_tail_latency_cycles,throughput_pkt_per_ms"
    )

for workload, group in zip(workloads, groups):
    records = group.records
    p99_slowdown = "NA"
    p99_tail_latency = "NA"

    if workload["metric"] == "slowdown":
        p99_slowdown = format_float(
            compute_p99_slowdown(
                records,
                fanout=int(workload["fanout"]),
                long_every=int(workload["long_every"]),
            )
        )
    else:
        p99_tail_latency = format_int(compute_p99_tail_latency(records))

    row = [
        workload["subplot"],
        workload["group"],
        workload["config"],
        len(records),
        matched_count(records),
        p99_slowdown,
        p99_tail_latency,
        format_float(throughput(records)),
    ]
    if output_to_file:
        writer.writerow(row)
    else:
        print(
            f"{row[0]},{row[1]},{row[2]},{row[3]},{row[4]},"
            f"{row[5]},{row[6]},{row[7]}"
        )
PY
exec 3>&-
