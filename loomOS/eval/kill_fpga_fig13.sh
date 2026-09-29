#!/usr/bin/env bash
set -eo pipefail
SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
source "${SCRIPT_DIR}/../../script/internal-tools/env_fig13.sh"
"${AE_DIR}/script/internal-tools/kill_fpga_fig13.sh"
cp "${FIRESIM_RUNS_DIR}/sim_slot_0/uartlog" "${SCRIPT_DIR}/fpga.log"
