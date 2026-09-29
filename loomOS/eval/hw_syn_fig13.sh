#!/usr/bin/env bash
set -euo pipefail
SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
source "${SCRIPT_DIR}/../../script/internal-tools/env_fig13.sh"
sed '/Core1/d' "${FIRESIM_RUNS_DIR}/sim_slot_0/synthesized-prints.out0"
