#!/bin/bash

export FPGA_IMG=/home/qxh/FIRESIM_RUNS_DIR/sim_slot_0/br-base0-br-base.img
export MOUNT_POINT=/home/qxh/FIRESIM_RUNS_DIR/mnt
export AE_HOME=/home/qxh/Threadlet-AE
RAW_LOG=/tmp/threadlet-ae-fig8

extract_figure8_csv() {
  local input_log="$1"
  local output_csv="$2"
  local candidate="${output_csv}.candidate.$$"

  if ! awk '
    {
      if (match($0, /figure8: completed_tasks=[0-9]+ total_cycles=[0-9]+ scheduler_cycles=[0-9]+ throughput_ktps=[0-9]+/)) {
        summary = substr($0, RSTART, RLENGTH)
        sub(/^figure8: completed_tasks=/, "", summary)
        sub(/ total_cycles=/, ",", summary)
        sub(/ scheduler_cycles=/, ",", summary)
        sub(/ throughput_ktps=/, ",", summary)
        field_count = split(summary, fields, ",")
        tasks = fields[1] + 0
        threads = tasks / 100
        total_cycles = fields[2] + 0
        scheduler_cycles = fields[3] + 0
        throughput = fields[4] + 0
        if (field_count == 4 && tasks == threads * 100 &&
            (threads == 2 || threads == 4 || threads == 8) &&
            total_cycles > 0 && scheduler_cycles <= total_cycles) {
          overhead = scheduler_cycles * 100.0 / total_cycles
          rows[threads] = sprintf("%d,%d,%d,%d,%d,%.2f",
                                  threads, tasks, total_cycles,
                                  scheduler_cycles, throughput, overhead)
        }
      }
    }
    END {
      if (!(2 in rows) || !(4 in rows) || !(8 in rows)) {
        print "missing one or more Figure 8 summaries for threads 2, 4, and 8" > "/dev/stderr"
        exit 1
      }
      print "threads,tasks,total_cycles,scheduler_cycles,throughput_ktps,overhead_percent"
      print rows[2]
      print rows[4]
      print rows[8]
    }
  ' "$input_log" > "$candidate"; then
    return 1
  fi
  mv "$candidate" "$output_csv"
}

if [[ "${1:-}" == "--extract" ]]; then
  if [[ "$#" -ne 3 ]]; then
    echo "Usage: $0 --extract SCREEN_LOG OUTPUT_CSV" >&2
    exit 2
  fi
  extract_figure8_csv "$2" "$3"
  exit $?
fi

# wait for booting the Linux
sleep 600

# Log in
screen -S fsim0 -X stuff "root\n"

sleep 2

# Running the benchmark on fpga-linux
screen -S fsim0 -X stuff "/root/Threadlet-AE/tool/fpga_fig8.sh\n"

# Waiting for the result
sleep 120

# get the screenshot and parse the result
screen -S fsim0 -X width 300
screen -S fsim0 -X stuff "cat /root/Threadlet-AE/result/fig8.csv\n"
sleep 2
screen -S fsim0 -X hardcopy "$RAW_LOG"
if ! extract_figure8_csv "$RAW_LOG" "$AE_HOME/result/fig8/baseline.csv"; then
  echo "Failed to extract Figure 8 baseline CSV from $RAW_LOG" >&2
  tail -n 30 "$RAW_LOG" >&2
  /home/qxh/Threadlet-AE/script/internal-tools/kill_fpga.sh
  exit 1
fi

# shut down fpga
/home/qxh/Threadlet-AE/script/internal-tools/kill_fpga.sh
