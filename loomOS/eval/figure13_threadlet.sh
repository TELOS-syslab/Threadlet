#!/usr/bin/env bash
set -euo pipefail

if [[ "$#" -gt 2 ]]; then
  echo "Usage: $0 [figure13_threadlet.log] [result.csv]" >&2
  exit 2
fi

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
source "${SCRIPT_DIR}/../../script/internal-tools/env_fig13.sh"
LOG_FILE="${1:-${SCRIPT_DIR}/figure13_threadlet.log}"
RESULT_FILE="${2:-}"
BOOT_LOG="${SCRIPT_DIR}/../tools/boot_fpga_fig13.log"
TMP_JSON="${SCRIPT_DIR}/../tools/tmp_fig13.json"
cp "${ASTER_HOME}/tools/tmp.json" "${TMP_JSON}"
MCS_FDT_PATH="${ASTER_HOME}/tools/threadlet_intr_mcs_4c.dtb"
dtc -I dts -O dtb -o "${MCS_FDT_PATH}" "${ASTER_HOME}/tools/threadlet_intr_mcs_4c.dts"

PYTHONDONTWRITEBYTECODE=1 python3 - "${TMP_JSON}" "${MCS_FDT_PATH}" <<'PY_CONFIG'
from __future__ import annotations

import json
import os
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
config = json.loads(updated)
config['asterinas']['source'] = os.environ['ASTER_HOME']
config['firmware']['opensbi-src'] = str(Path(os.environ['ASTER_HOME']) / 'opensbi')
config['base-img'] = os.environ.get('LOOMOS_BASE_IMG', os.environ['FPGA_IMG'])
updated = json.dumps(config, indent=2) + '\n'

if updated != text:
    config_path.write_text(updated, encoding="utf-8")
PY_CONFIG

BOOT_PID=""
cleanup() {
    local status=$?
    trap - EXIT
    if [[ -n "${BOOT_PID}" ]]; then
        kill -TERM -- "-${BOOT_PID}" 2>/dev/null || true
        wait "${BOOT_PID}" || true
    fi
    exit "${status}"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM
setsid "${SCRIPT_DIR}/../tools/boot_fpga_fig13.sh" >"${BOOT_LOG}" 2>&1 &
BOOT_PID=$!

echo "————————————————————FPGA starts. Please wait for the simulation to end————————————————————"

deadline=$((SECONDS + ${FIG13_LOOMOS_BOOT_TIMEOUT_SECONDS:-1200}))
until grep -Fq "FireSim Simulation Status" "${BOOT_LOG}"; do
    if (( SECONDS >= deadline )); then
        echo "timed out booting LoomOS; inspect ${BOOT_LOG}" >&2
        exit 1
    fi
    if ! kill -0 "${BOOT_PID}" 2>/dev/null; then
        wait "${BOOT_PID}" || true
        BOOT_PID=""
        echo "LoomOS boot exited early; inspect ${BOOT_LOG}" >&2
        exit 1
    fi
    sleep 1
done

echo "————————————————————Kernel starts. Please wait for the simulation to end————————————————————"

sleep "${FIG13_LOOMOS_RUN_SECONDS:-35}"


echo "______________Stopping the FPGA simulation______________________"
cd "${SCRIPT_DIR}"
"${SCRIPT_DIR}/kill_fpga_fig13.sh"

wait "${BOOT_PID}" || true
BOOT_PID=""

## Get the hardware log
"${SCRIPT_DIR}/hw_syn_fig13.sh" > "${LOG_FILE}"



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
PYTHONDONTWRITEBYTECODE=1 python3 - \
  "${SCRIPT_DIR}" "${LOG_FILE}" >&3 <<'PY'
from __future__ import annotations

import sys
from pathlib import Path


script_dir = Path(sys.argv[1]).resolve()
log_path = Path(sys.argv[2]).resolve()
sys.path.insert(0, str(script_dir))

import analysis_mcs  # noqa: E402


GROUP_THREAD_COUNTS = [1, 2, 4, 8]
EXPECTED_SECTION_COUNTS = [40, 80, 160, 320]
GROUP_END_TAG = 9999


def fail(message: str) -> None:
    raise SystemExit(f"[error] {message}")


groups: list[list[str]] = []
current: list[str] = []
has_critical_event = False

with log_path.open("r", encoding="utf-8", errors="ignore") as log_file:
    for line in log_file:
        event = analysis_mcs.parse_event(line)
        if event is None or event.stage != analysis_mcs.MCS_STAGE:
            continue

        if event.tag == GROUP_END_TAG:
            if has_critical_event:
                groups.append(current)
                current = []
                has_critical_event = False
            elif len(groups) < len(GROUP_THREAD_COUNTS):
                fail("unexpected empty 9999 marker before all MCS groups completed")
            continue

        current.append(line)
        if event.tag in (
            analysis_mcs.TAG_CRITICAL_ENTER,
            analysis_mcs.TAG_CRITICAL_EXIT,
        ):
            has_critical_event = True

if has_critical_event:
    fail("the final MCS group has no 9999 end marker")
if len(groups) != len(GROUP_THREAD_COUNTS):
    fail(f"expected 4 nonempty MCS groups, found {len(groups)}")

rows: list[str] = []
for thread_count, expected_sections, group in zip(
    GROUP_THREAD_COUNTS,
    EXPECTED_SECTION_COUNTS,
    groups,
):
    sections, enter_count, exit_count = analysis_mcs.analyze(group)
    if not (
        len(sections) == expected_sections
        and enter_count == expected_sections
        and exit_count == expected_sections
    ):
        fail(
            f"threads_per_cpu={thread_count} expected {expected_sections} "
            f"critical sections: enter={enter_count}, exit={exit_count}, "
            f"paired={len(sections)}"
        )

    throughput = analysis_mcs.compute_throughput(sections)[4]
    latency = analysis_mcs.compute_exit_to_next_enter_latency(sections)[0]
    if throughput is None or latency is None:
        fail(f"threads_per_cpu={thread_count} has insufficient metric samples")

    rows.append(
        f"{thread_count},"
        f"{analysis_mcs.format_num(latency)},"
        f"{analysis_mcs.format_num(throughput)}"
    )

print("threads_per_cpu,latency_cycles,throughput_per_1m_cycles")
print("\n".join(rows))
PY
exec 3>&-
