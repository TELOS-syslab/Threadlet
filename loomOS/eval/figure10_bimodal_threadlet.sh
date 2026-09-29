#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
RPC_TEST="${SCRIPT_DIR}/fpga-rpc-test-bimodal.sh"
SEND_SLEEP_VALUES=(5000 1500 0)

if [[ ! -x "${RPC_TEST}" ]]; then
  echo "[error] ${RPC_TEST} is not executable." >&2
  exit 1
fi

for idx in "${!SEND_SLEEP_VALUES[@]}"; do
  send_sleep_us="${SEND_SLEEP_VALUES[$idx]}"
  run_idx=$((idx + 1))
  echo "[figure10] run ${run_idx}/${#SEND_SLEEP_VALUES[@]}"
  "${RPC_TEST}" "$@" -s "${send_sleep_us}"

  if [[ "${run_idx}" -lt "${#SEND_SLEEP_VALUES[@]}" ]]; then
    sleep 3
  fi
done


# python3 analysis_rpc.py hwnnor_l37.log --group-size 100