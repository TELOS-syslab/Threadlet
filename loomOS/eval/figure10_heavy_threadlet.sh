#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
RPC_TEST="${SCRIPT_DIR}/fpga-rpc-test-heavy.sh"
SEND_SLEEP_VALUES=(0 0 0)
FANOUT_VALUES=(1 3 6)
MAX_RPC_VALUES=(600 200 200)

if [[ ! -x "${RPC_TEST}" ]]; then
  echo "[error] ${RPC_TEST} is not executable." >&2
  exit 1
fi

for idx in "${!FANOUT_VALUES[@]}"; do
  send_sleep_us="${SEND_SLEEP_VALUES[$idx]}"
  fanout="${FANOUT_VALUES[$idx]}"
  max_rpc="${MAX_RPC_VALUES[$idx]}"
  run_idx=$((idx + 1))
  echo "[figure10] run ${run_idx}/${#FANOUT_VALUES[@]}"
  "${RPC_TEST}" "$@" -n "${max_rpc}" -s "${send_sleep_us}" -k "${fanout}"

  if [[ "${run_idx}" -lt "${#FANOUT_VALUES[@]}" ]]; then
    sleep 3
  fi
done
