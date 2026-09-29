#!/bin/sh

mkdir -p /root/Threadlet-AE/result

echo "metric,time" > /root/Threadlet-AE/result/fig7.csv

# Get context switch & Sched result
insmod "/root/Threadlet-AE/task_over/switch_8.ko"
sleep 0.5
rmmod switch.ko
block=$(dmesg | grep -E "sched :|cs :|throughput:|break down" | tail -n 5)
if [ -n "$block" ]; then
  parsed_values=$(echo "$block" | sed 's/^.*\]//' | sed 's/[^0-9\n]//g' | tr '\n' ' ')

  read -r sched cs throughput bd_sched bd_cs <<< "$parsed_values"

  # echo "***Task_over Bench***: sched: ${sched}%, cs: ${cs}%, throughput: $throughput, bd_sched: $bd_sched, bd_cs: $bd_cs"
  # echo "$sched, $cs, $throughput, $bd_sched, $bd_cs" > "/root/Threadlet-AE/result/switch_${i}"
  # echo "$i,$sched,$cs,$throuput" >> /root/Threadlet-AE/result/fig7.csv
  echo "context_switch,$bd_cs" >> /root/Threadlet-AE/result/fig7.csv
  echo "scheduling,$bd_sched" >> /root/Threadlet-AE/result/fig7.csv
else
  echo "Not found"
fi

# Get T-T and D-T notification
insmod "/root/Threadlet-AE/ipi/ipi.ko"

# We can not sleep here due to avoid interference 
# So we wait in the host to perform rmmod

# Bottom half
# sleep 5
# rmmod ipi.ko
# t_t=$(dmesg | tail -n 1 | awk '{for(i=1;i<=NF;i++) if($i=="tsc") print $(i-1)}')
# echo "t_t_notification,$t_t" >> /root/Threadlet-AE/result/fig7.csv
# echo "d_t_notification,1032" >> /root/Threadlet-AE/result/fig7.csv
# echo "" >> /root/Threadlet-AE/result/fig7.csv