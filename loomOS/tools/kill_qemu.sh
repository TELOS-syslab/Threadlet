#!/usr/bin/env bash
set -euo pipefail

target="qemu-system-riscv64"
pids=()

for d in /proc/[0-9]*; do
  cmdline="$d/cmdline"
  [[ -r "$cmdline" ]] || continue

  first=$(tr '\0' '\n' < "$cmdline" | head -n1)
  [[ -n "$first" ]] || continue

  if [[ "$(basename -- "$first")" == "$target" ]]; then
    pids+=("${d##*/}")
  fi
done

if ((${#pids[@]} == 0)); then
  echo "No qemu-system-riscv64 processes found."
  exit 0
fi

echo "Sending SIGTERM to: ${pids[*]}"
sudo kill -TERM "${pids[@]}"

deadline=$((SECONDS + 5))
while (( SECONDS < deadline )) && ((${#pids[@]} > 0)); do
  sleep 1
  alive=()
  for pid in "${pids[@]}"; do
    sudo kill -0 "$pid" 2>/dev/null && alive+=("$pid")
  done
  pids=("${alive[@]}")
done

if ((${#pids[@]} > 0)); then
  echo "Forcing SIGKILL to: ${pids[*]}"
  kill -KILL "${pids[@]}"
else
  echo "All processes exited."
fi
