#!/usr/bin/env bash
set -euo pipefail

if [[ "$#" -gt 1 ]]; then
    echo "Usage: $0 [result.csv]" >&2
    exit 2
fi

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
RESULT_FILE="${1:-}"

ARGS=(
    "${SCRIPT_DIR}/figure8_threadlet.log"
    "${SCRIPT_DIR}/figure13_threadlet.log"
    "${SCRIPT_DIR}/figure10_threadlet.log"
)

if [[ -n "${RESULT_FILE}" ]]; then
    mkdir -p "$(dirname -- "${RESULT_FILE}")"
    PYTHONDONTWRITEBYTECODE=1 python3 \
        "${SCRIPT_DIR}/analysis_inefficiency.py" "${ARGS[@]}" > "${RESULT_FILE}"
else
    PYTHONDONTWRITEBYTECODE=1 python3 \
        "${SCRIPT_DIR}/analysis_inefficiency.py" "${ARGS[@]}"
fi
