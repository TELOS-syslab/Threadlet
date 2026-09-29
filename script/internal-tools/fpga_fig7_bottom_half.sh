#!/bin/sh

# Bottom half
rmmod ipi.ko
t_t=$(dmesg | tail -n 1 | awk '{for(i=1;i<=NF;i++) if($i=="tsc") print $(i-1)}')
echo "t_t_notification,$t_t" >> /root/Threadlet-AE/result/fig7.csv
echo "d_t_notification,1032" >> /root/Threadlet-AE/result/fig7.csv
echo "" >> /root/Threadlet-AE/result/fig7.csv