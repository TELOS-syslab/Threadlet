#!/bin/sh

THREADS_LIST=(2 4 8)

mkdir -p /root/Threadlet-AE/result

for i in "${THREADS_LIST[@]}"
do
  insmod "/root/Threadlet-AE/task_over/switch_${i}.ko"
  sleep 0.5
  rmmod switch.ko

  block=$(dmesg | grep -E "sched :|cs :|throughput:|break down" | tail -n 5)

  if [ -n "$block" ]; then
    parsed_values=$(echo "$block" | sed 's/^.*\]//' | sed 's/[^0-9\n]//g' | tr '\n' ' ')

    read -r sched cs throughput bd_sched bd_cs <<< "$parsed_values"

    echo "***Task_over Bench***: sched: ${sched}%, cs: ${cs}%, throughput: $throughput, bd_sched: $bd_sched, bd_cs: $bd_cs"
    echo "$sched, $cs, $throughput, $bd_sched, $bd_cs" > "/root/Threadlet-AE/result/switch_${i}"

  else
    echo "Not found"
  fi

done