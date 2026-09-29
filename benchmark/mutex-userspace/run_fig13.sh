#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
source "${SCRIPT_DIR}/../../script/internal-tools/env_fig13.sh"
set +eu
source "${CHIPYARD_HOME}/env.sh"
ENV_STATUS=$?
set -euo pipefail
(( ENV_STATUS == 0 )) || exit "${ENV_STATUS}"
source "${SCRIPT_DIR}/../../script/internal-tools/env_fig13.sh"
SETUP_PID=""

cleanup() {
    local status=$?
    trap - EXIT
    if [[ -n "${SETUP_PID}" ]]; then
        kill -TERM -- "-${SETUP_PID}" 2>/dev/null || true
        wait "${SETUP_PID}" || true
    fi
    exit "${status}"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

"${SCRIPT_DIR}/build.sh"
if [[ ! -x "${AE_DIR}/benchmark/mutex-userspace/build/argobots_fig13" ]]; then
    echo "missing argobots_fig13 after build" >&2
    exit 1
fi

setsid "${SCRIPT_DIR}/../../script/internal-tools/setup_fpga_fig13.sh" &
SETUP_PID=$!
export FIG13_SETUP_PID="${SETUP_PID}"
export FIG13_SHARED_SESSION=0 FIG13_SHELL_READY=0
"${SCRIPT_DIR}/../fig13/collect.sh" argobots
wait "${SETUP_PID}" || true
SETUP_PID=""

echo "Argobots raw runs: ${AE_DIR}/result/fig13/raw/argobots/"
echo "Argobots result: ${AE_DIR}/result/fig13/argobots.csv"
