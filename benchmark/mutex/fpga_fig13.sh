#!/usr/bin/env bash
set -euo pipefail
RUN_ID="${1:-manual}"
RESULT_DIR=/root/Threadlet-AE/result/fig13/raw/mutex
RESULT_FILE="${RESULT_DIR}/runs-${RUN_ID}.csv"
mkdir -p "${RESULT_DIR}"
printf '%s\n' 'status,threads_per_cpu,total_workers,run,latency_cycles,throughput_ops_per_second' >"${RESULT_FILE}"
finish() {
    local status=$?
    sed -n '2,$p' "${RESULT_FILE}"
    echo "===MUTEX_FIG13_END:${RUN_ID}==="
    exit "${status}"
}
trap finish EXIT
echo "===MUTEX_FIG13_BEGIN:${RUN_ID}==="
for threads_per_cpu in 1 2 4 8; do
    total_workers=$((threads_per_cpu * 4))
    for run in 1 2 3 4 5; do
        before=$(dmesg | grep -c 'Total lock handover count:' || true)
        insmod "/root/Threadlet-AE/mutex/mutex_${total_workers}.ko"
        sleep 1
        rmmod mutex
        after=$(dmesg | grep -c 'Total lock handover count:' || true)
        if (( after != before + 1 )); then
            echo 'missing or ambiguous new mutex summary' >&2
            exit 1
        fi
        line=$(dmesg | grep 'Total lock handover count:' | tail -n 1)
        metrics=$(printf '%s\n' "${line}" | sed -nE 's/.*Total lock handover count:[[:space:]]*([0-9]+), average handover time:[[:space:]]*([0-9]+)[[:space:]]*cycles,[[:space:]]*throughput:[[:space:]]*([0-9]+).*/\1 \2 \3/p')
        read -r handoffs latency throughput <<<"${metrics}"
        if [[ ! "${handoffs}" =~ ^[0-9]+$ || ! "${latency}" =~ ^[0-9]+$ || ! "${throughput}" =~ ^[0-9]+$ ]] ||
            (( handoffs == 0 || throughput == 0 )); then
            echo "invalid mutex summary: ${line}" >&2
            exit 1
        fi
        printf 'PASS,%d,%d,%d,%d,%d\n' "${threads_per_cpu}" "${total_workers}" "${run}" "${latency}" "${throughput}" >>"${RESULT_FILE}"
    done
done
