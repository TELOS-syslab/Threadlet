#!/usr/bin/env bash
set -euo pipefail
SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
source "${SCRIPT_DIR}/internal-tools/env_fig13.sh"
EVAL_DIR="${AE_DIR}/loomOS/eval"
RESULT_DIR="${AE_DIR}/result/fig13"
SETUP_PID=""
mkdir -p "${RESULT_DIR}/raw"

if [[ "$(uname -s)" != Linux ]]; then
    echo 'Figure 13 must run on the Linux FPGA/FireSim host.' >&2
    exit 1
fi
[[ -f "${CHIPYARD_HOME}/env.sh" ]] || { echo "missing Chipyard environment: ${CHIPYARD_HOME}/env.sh" >&2; exit 1; }
# Load the compiler and Python environment before dependency checks.
set +eu
source "${CHIPYARD_HOME}/env.sh"
ENV_STATUS=$?
set -euo pipefail
(( ENV_STATUS == 0 )) || { echo 'failed to load Chipyard environment' >&2; exit 1; }
source "${SCRIPT_DIR}/internal-tools/env_fig13.sh"
for tool in bash python3 make dtc screen sudo ssh-agent ssh-add setsid; do
    command -v "${tool}" >/dev/null || { echo "missing tool: ${tool}" >&2; exit 1; }
done
python3 -c 'import matplotlib'
export RISCV_OBJCOPY="$(command -v "${RISCV_OBJCOPY:-riscv64-unknown-elf-objcopy}")"
[[ -x "${RISCV_OBJCOPY}" ]] || { echo 'set RISCV_OBJCOPY to a RISC-V objcopy executable' >&2; exit 1; }
for path in "${FIRESIM_SSH_KEY}" "${FIRESIM_HOME}/sourceme-manager.sh" "${FPGA_IMG}" "${LINUXSRC}/Makefile" "${ASTER_HOME}/target/osdk/aster-nix-osdk-bin.qemu_elf"; do
    [[ -f "${path}" ]] || { echo "missing prerequisite: ${path}" >&2; exit 1; }
done

if [[ -z "${SSH_AUTH_SOCK:-}" ]]; then eval "$(ssh-agent -s)"; fi
ssh-add "${FIRESIM_SSH_KEY}"

# Build every baseline before mounting the Linux image.
(cd "${AE_DIR}/benchmark/mutex" && ./build_fig13.sh)
"${AE_DIR}/benchmark/mutex-userspace/build.sh"
"${AE_DIR}/benchmark/mcs-b/build.sh"

# Run Threadlet tests.
"${EVAL_DIR}/figure13_threadlet.sh" \
    "${EVAL_DIR}/figure13_threadlet.log" "${RESULT_DIR}/threadlet.csv"

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

# Run baseline tests.
export FIG13_SHARED_SESSION=1
export FIG13_SHELL_READY=0
FIG13_BASELINES=1 setsid "${SCRIPT_DIR}/internal-tools/setup_fpga_fig13.sh" >"${RESULT_DIR}/raw/linux-setup.log" 2>&1 &
SETUP_PID=$!
export FIG13_SETUP_PID="${SETUP_PID}"
"${AE_DIR}/benchmark/fig13/collect.sh" mutex
export FIG13_SHELL_READY=1
"${AE_DIR}/benchmark/fig13/collect.sh" argobots
"${AE_DIR}/benchmark/fig13/collect.sh" mcs-b
"${SCRIPT_DIR}/internal-tools/kill_fpga_fig13.sh"
wait "${SETUP_PID}" || true
SETUP_PID=""

# Plot all four validated results.
python3 "${SCRIPT_DIR}/plot_figures.py" 13 "${RESULT_DIR}"
