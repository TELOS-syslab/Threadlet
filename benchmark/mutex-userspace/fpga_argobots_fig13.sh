#!/usr/bin/env bash
set -euo pipefail
RUN_ID="${1:-manual}"

BENCHMARK="/root/Threadlet-AE/mutex-userspace/argobots_fig13"
RESULT_DIR="/root/Threadlet-AE/result/fig13/raw/argobots"
RESULT_FILE="${RESULT_DIR}/runs.csv"
RESULT_CANDIDATE="${RESULT_FILE}.candidate"
RUN_OUTPUT="${RESULT_DIR}/argobots-run.candidate"
STDERR_CANDIDATE="${RESULT_FILE}.stderr.candidate"
THREADS_PER_CPU=(1 2 4 8)
RUNS=5
HEADER="status,argobots_version,execution_streams,threads_per_cpu,total_workers,run,acquisitions,handoffs,handoff_time_ticks,elapsed_time_ticks,latency_cycles,throughput_per_1m_cycles,tool_enabled,affinity_errors,order_errors,critical_cycles"

mkdir -p "${RESULT_DIR}"
printf '%s\n' "${HEADER}" >"${RESULT_CANDIDATE}"
: >"${STDERR_CANDIDATE}"

echo "===ARGOBOTS_FIG13_BEGIN:${RUN_ID}==="
benchmark_status=0
for threads_per_cpu in "${THREADS_PER_CPU[@]}"; do
    for ((run = 1; run <= RUNS; run++)); do
        "${BENCHMARK}" "${threads_per_cpu}" "${run}" \
            >"${RUN_OUTPUT}" 2>>"${STDERR_CANDIDATE}" ||
            benchmark_status=$?

        if awk -v expected_header="${HEADER}" '
            NR == 1 && $0 != expected_header {
                exit 1
            }
            NR == 2 {
                row = $0
            }
            END {
                if (NR != 2) {
                    exit 1
                }
                print row
            }
        ' "${RUN_OUTPUT}" >>"${RESULT_CANDIDATE}"; then
            :
        else
            echo "malformed argobots_fig13 output for threads_per_cpu=${threads_per_cpu} run=${run}" \
                >>"${STDERR_CANDIDATE}"
            cat "${RUN_OUTPUT}" >>"${STDERR_CANDIDATE}"
            benchmark_status=1
        fi

        if [[ "${benchmark_status}" -ne 0 ]]; then
            break 2
        fi
    done
done

sed -n '2,$p' "${RESULT_CANDIDATE}"
if [[ "${benchmark_status}" -eq 0 ]]; then
    mv "${RESULT_CANDIDATE}" "${RESULT_FILE}"
fi
if [[ -s "${STDERR_CANDIDATE}" ]]; then
    cat "${STDERR_CANDIDATE}" >&2
fi
if [[ "${benchmark_status}" -ne 0 ]]; then
    echo "argobots_fig13 exited with status ${benchmark_status}" >&2
fi
echo "===ARGOBOTS_FIG13_END:${RUN_ID}==="
exit "${benchmark_status}"
