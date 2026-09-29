#!/usr/bin/env bash
set -euo pipefail
SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
source "${SCRIPT_DIR}/../../script/internal-tools/env_fig13.sh"
SYSTEM="${1:?Usage: collect.sh mutex|argobots|mcs-b}"
case "${SYSTEM}" in
    mutex) GUEST=fpga_fig13.sh; MARKER=MUTEX_FIG13 ;;
    argobots) GUEST=fpga_argobots_fig13.sh; MARKER=ARGOBOTS_FIG13 ;;
    mcs-b) GUEST=fpga_mcs_b.sh; MARKER=MCS_B_FIG13 ;;
    *) echo "unknown baseline: ${SYSTEM}" >&2; exit 2 ;;
esac
SESSION_NAME="${FIG13_SCREEN_SESSION:-fsim0}"
BOOT_TIMEOUT_SECONDS="${FIG13_BOOT_TIMEOUT_SECONDS:-600}"
RUN_TIMEOUT_SECONDS="${FIG13_RUN_TIMEOUT_SECONDS:-1800}"
POLL_INTERVAL_SECONDS="${FIG13_POLL_INTERVAL_SECONDS:-5}"
RUN_ID="$(date +%Y%m%d-%H%M%S)-$$"
FIG13_RESULT_DIR="${FIG13_RESULT_DIR:-${AE_DIR}/result/fig13}"
RAW_RESULT_DIR="${FIG13_RESULT_DIR}/raw/${SYSTEM}"
RAW_LOG="${RAW_RESULT_DIR}/screen-${RUN_ID}.log"
RAW_CSV="${RAW_RESULT_DIR}/runs-${RUN_ID}.csv"
mkdir -p "${RAW_RESULT_DIR}"

cleanup() {
    if [[ "${FIG13_SHARED_SESSION:-0}" != 1 ]]; then
        "${AE_DIR}/script/internal-tools/kill_fpga_fig13.sh" || true
    fi
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

wait_for_pattern() {
    local pattern="$1" timeout="$2" deadline=$((SECONDS + $2))
    while (( SECONDS < deadline )); do
        if screen -S "${SESSION_NAME}" -X hardcopy -h "${RAW_LOG}" >/dev/null 2>&1 &&
            grep -Eq "${pattern}" "${RAW_LOG}"; then
            return 0
        fi
        if [[ -n "${FIG13_SETUP_PID:-}" ]] && ! kill -0 "${FIG13_SETUP_PID}" 2>/dev/null; then
            echo "Linux setup exited before ${pattern}; inspect the setup log" >&2
            return 1
        fi
        sleep "${POLL_INTERVAL_SECONDS}"
    done
    echo "timed out after ${timeout}s waiting for ${pattern}; log: ${RAW_LOG}" >&2
    return 1
}

if [[ "${FIG13_SHELL_READY:-0}" != 1 ]]; then
    wait_for_pattern 'buildroot login:' "${BOOT_TIMEOUT_SECONDS}"
    screen -S "${SESSION_NAME}" -X scrollback 10000
    screen -S "${SESSION_NAME}" -X width -w 300
    screen -S "${SESSION_NAME}" -X stuff $'root\n'
    wait_for_pattern '^# ?$' 60
fi
screen -S "${SESSION_NAME}" -X stuff "/root/Threadlet-AE/tool/${GUEST} ${RUN_ID}"$'\n'
wait_for_pattern "^===${MARKER}_END:${RUN_ID}===$" "${RUN_TIMEOUT_SECONDS}"

python3 - "${SYSTEM}" "${RAW_LOG}" "${RAW_CSV}" "${MARKER}" "${RUN_ID}" "${SCRIPT_DIR}" <<'PY'
import sys
from pathlib import Path
system, log, output, marker, run_id, script_dir = sys.argv[1:]
sys.path.insert(0, script_dir)
from parse_lock_results import HEADERS
lines = Path(log).read_text(errors='replace').splitlines()
begin = f'==={marker}_BEGIN:{run_id}==='
end = f'==={marker}_END:{run_id}==='
if lines.count(begin) != 1 or lines.count(end) != 1:
    raise SystemExit('missing or duplicate run markers in screen capture')
first, last = lines.index(begin), lines.index(end)
if last <= first:
    raise SystemExit('out-of-order run markers')
rows = [line for line in lines[first+1:last] if line.startswith(('PASS,', 'FAIL,'))]
Path(output).write_text(HEADERS[system] + '\n' + '\n'.join(rows) + '\n')
PY
python3 "${SCRIPT_DIR}/parse_lock_results.py" "${SYSTEM}" "${RAW_CSV}" \
    "${FIG13_RESULT_DIR}/${SYSTEM}.csv"
echo "validated ${RAW_CSV}"
