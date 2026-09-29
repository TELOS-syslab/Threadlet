#!/bin/bash

export FPGA_IMG=/home/qxh/FIRESIM_RUNS_DIR/sim_slot_0/br-base0-br-base.img
export MOUNT_POINT=/home/qxh/FIRESIM_RUNS_DIR/mnt
export AE_HOME=/home/qxh/Threadlet-AE

# wait for booting the Linux
sleep 600

# Log in
screen -S fsim0 -X stuff "root\n"

sleep 5

# Running the benchmark on fpga-linux
screen -S fsim0 -X stuff "/root/Threadlet-AE/tool/fpga_fig7.sh\n"

# Waiting for the experiment
sleep 60

# Get the result in fpga
screen -S fsim0 -X stuff "/root/Threadlet-AE/tool/fpga_fig7_bottom_half.sh\n"
sleep 5

# get the screenshot and parse the result
screen -S fsim0 -X stuff "cat /root/Threadlet-AE/result/fig7.csv\n"
sleep 5
screen -S fsim0 -X hardcopy "/tmp/threadlet-ae-fig7"
tac /tmp/threadlet-ae-fig7 | awk '
  $0 ~ /metric,time/ {
    if (state == 1) {
      print "metric,time"
      exit
    }
  }
  state == 0 && match($0, /d_t_notification,[0-9a-zA-Z_.-]+/) {
    print substr($0, RSTART, RLENGTH)
    state = 1
    next
  }
  state == 1 {
    if (match($0, /[0-9a-zA-Z_.-]+,[0-9a-zA-Z_.-]+/)) {
      print substr($0, RSTART, RLENGTH)
    }
  }
' | tac > $AE_HOME/result/fig7/baseline.csv

# shut down fpga
/home/qxh/Threadlet-AE/script/internal-tools/kill_fpga.sh