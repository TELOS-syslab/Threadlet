#!/usr/bin/env bash
set -euo pipefail


if [[ "$#" -gt 2 ]]; then
    echo "Usage: $0 [figure8_threadlet.log] [result.csv]" >&2
    exit 2
fi

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
LOG_FILE="${1:-${SCRIPT_DIR}/figure8_threadlet.log}"
RESULT_FILE="${2:-}"
BOOT_LOG="${SCRIPT_DIR}/../tools/boot_fpga.log"
TMP_JSON="${SCRIPT_DIR}/../tools/tmp.json"
SCHEDULING_FDT_PATH="/home/qxh/asterinas_threadlet/tools/threadlet_intr_scheduling_4c.dtb"

PYTHONDONTWRITEBYTECODE=1 python3 - "${TMP_JSON}" "${SCHEDULING_FDT_PATH}" <<'PY_CONFIG'
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

echo "______________Stopping the FPGA simulation______________________"
"${SCRIPT_DIR}/kill_fpga.sh"

sleep 40

"${SCRIPT_DIR}/hw_syn.sh" > "${LOG_FILE}"

if [[ ! -f "${LOG_FILE}" ]]; then
    echo "[error] log file not found: ${LOG_FILE}" >&2
    exit 1
fi

if [[ -n "${RESULT_FILE}" ]]; then
    mkdir -p "$(dirname -- "${RESULT_FILE}")"
    exec 3> "${RESULT_FILE}"
else
    exec 3>&1
fi
PYTHONDONTWRITEBYTECODE=1 python3 \
    "${SCRIPT_DIR}/analysis_scheduling.py" "${LOG_FILE}" >&3
exec 3>&-
