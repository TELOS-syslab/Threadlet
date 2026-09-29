#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
AE_DIR="$(cd -- "${SCRIPT_DIR}/.." && pwd)"
EVAL_DIR="${AE_DIR}/loomOS/eval"
RESULT_DIR="${AE_DIR}/result/fig7"

mkdir -p "${RESULT_DIR}"

# Run Threadlet tests.
"${EVAL_DIR}/figure7_threadlet.sh" "${RESULT_DIR}/threadlet.csv"

# Run baseline tests.
"${AE_DIR}/script/internal-tools/setup_fpga.sh" & \
    "${AE_DIR}/script/internal-tools/baseline_fig7.sh"

# Plot all available results.
PYTHONDONTWRITEBYTECODE=1 python3 "${SCRIPT_DIR}/plot_figures.py" 7 "${RESULT_DIR}"
