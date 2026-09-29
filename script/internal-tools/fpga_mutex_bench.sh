#!/bin/sh

THREADS_LIST=(4 8 16 32)
LOOP_COUNT=5

mkdir -p /root/Threadlet-AE/result

for i in "${THREADS_LIST[@]}"
do
  sum_time=0
  sum_throughput=0
  valid_count=0
  
  for ((j=1; j<=LOOP_COUNT; j++))
  do
    insmod "/root/Threadlet-AE/mutex/mutex_${i}.ko"
    sleep 1
    rmmod mutex.ko

    line=$(dmesg | tail -n 1 | grep "average handover time:" )

    if [ -n "$line" ]; then
      read -r avg_time throughput <<< "$(echo "$line" | sed -nE 's/.*average handover time:[[:space:]]*([0-9]+)[[:space:]]*cycles,[[:space:]]*throughput:[[:space:]]*([0-9]+).*/\1 \2/p')"

      if [ -n "$avg_time" ] && [ -n "$throughput" ]; then
        sum_time=$((sum_time + avg_time))
        sum_throughput=$((sum_throughput + throughput))
        valid_count=$((valid_count + 1))
      fi
      # echo "***Mutex Bench***: avg_time: $avg_time, throughput: $throughput"
      # echo "$avg_time, $throughput" > "/root/Threadlet-AE/result/mutex_${i}"
    else
      echo "Not found"
    fi
  done
  
  if [ "$valid_count" -gt 0 ]; then
    final_avg_time=$((sum_time / valid_count))
    final_throughput=$((sum_throughput / valid_count))

    echo "***Mutex Bench***: avg_time: $final_avg_time, throughput: $final_throughput"
    echo "$i,$final_avg_time,$final_throughput" > "/root/Threadlet-AE/result/mutex_${i}"
  else
    echo "Not found"
    echo "N/A, N/A" > "/root/Threadlet-AE/result/mutex_${i}"
  fi

done

echo "threads_per_cpu,latency_cycles,throughput_per_1m_cycles" > /root/Threadlet-AE/result/mutex_result
for i in "${THREADS_LIST[@]}"
do
  cat "/root/Threadlet-AE/result/mutex_${i}" >> /root/Threadlet-AE/result/mutex_result
done