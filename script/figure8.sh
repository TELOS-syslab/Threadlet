#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
AE_DIR="$(cd -- "${SCRIPT_DIR}/.." && pwd)"
EVAL_DIR="${AE_DIR}/loomOS/eval"
RESULT_DIR="${AE_DIR}/result/fig8"

mkdir -p "${RESULT_DIR}"


cd ${SCRIPT_DIR}

# Build the Linux baseline with the current throughput formula.
(
    cd "${AE_DIR}/benchmark/task_over"
    ./build.sh
)

# # Run Threadlet tests.
"${EVAL_DIR}/figure8_threadlet.sh" \
    "${EVAL_DIR}/figure8_threadlet.log" \
    "${RESULT_DIR}/threadlet.csv"

# Run baseline tests.
"${AE_DIR}/script/internal-tools/setup_fpga.sh" & \
    "${AE_DIR}/script/internal-tools/baseline_fig8.sh"



# Plot all available results.
PYTHONDONTWRITEBYTECODE=1 python3 "${SCRIPT_DIR}/plot_figures.py" 8 "${RESULT_DIR}"
