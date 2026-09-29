#!/bin/bash

export FPGA_IMG=/home/qxh/FIRESIM_RUNS_DIR/sim_slot_0/br-base0-br-base.img
export MOUNT_POINT=/home/qxh/FIRESIM_RUNS_DIR/mnt
export AE_HOME=/home/qxh/Threadlet-AE
export LINUX_POLLING=/root/Threadlet-AE/network/thlet_linux.ko
export SHINJUKU=/root/Threadlet-AE/network/thlet_shinjuku.ko
export RPC_BI=$AE_HOME/script/internal-tools/fpga-rpc-test-bi.sh
export RPC_HT=$AE_HOME/script/internal-tools/fpga-rpc-test-ht.sh
export RPC_KV=$AE_HOME/script/internal-tools/fpga-rpc-test-kv.sh
export FPGA_PARSER=/root/Threadlet-AE/tool/fpga_network_parser.sh

# wait for booting the Linux
# sleep 600

# # Log in
# screen -S fsim0 -X stuff "root\n"

# sleep 1

# # Setup and configure the network
# screen -S fsim0 -X stuff "/root/S40network start\n"
# sleep 1
# screen -S fsim0 -X stuff "route add default gw 172.16.0.1 eth0\n echo \"nameserver 8.8.8.8\" >> /etc/resolv.conf\n echo \"nameserver 8.8.4.4\" >> /etc/resolv.conf\n"
# sleep 1

screen -S fsim0 -X stuff "mkdir -p /root/Threadlet-AE/result\n"

# Run Linux 

# Bimodal
screen -S fsim0 -X stuff "echo \"p99slowdown,p99latency,throughput\" > /root/Threadlet-AE/result/linux-bi\n"
# Bimodal set1

screen -S fsim0 -X stuff "insmod $LINUX_POLLING\n"
sleep 3
sudo $RPC_BI -k 1 -s 3000
sleep 3
screen -S fsim0 -X stuff "rmmod $LINUX_POLLING\n"
sleep 3
screen -S fsim0 -X stuff "$FPGA_PARSER >> /root/Threadlet-AE/result/linux-bi\n"
sleep 3

# Bimodal set2

screen -S fsim0 -X stuff "insmod $LINUX_POLLING\n"
sleep 3
sudo $RPC_BI -k 1 -s 2000
sleep 3
screen -S fsim0 -X stuff "rmmod $LINUX_POLLING\n"
sleep 3
screen -S fsim0 -X stuff "$FPGA_PARSER >> /root/Threadlet-AE/result/linux-bi\n"
sleep 3

# Bimodal set3
screen -S fsim0 -X stuff "insmod $LINUX_POLLING\n"
sleep 3
sudo $RPC_BI -k 1 -s 1400
sleep 3
screen -S fsim0 -X stuff "rmmod $LINUX_POLLING\n"
sleep 3
screen -S fsim0 -X stuff "$FPGA_PARSER >> /root/Threadlet-AE/result/linux-bi\n"
sleep 3

# Bimodal set4
screen -S fsim0 -X stuff "insmod $LINUX_POLLING\n"
sleep 3
sudo $RPC_BI -k 1 -s 1000
sleep 3
screen -S fsim0 -X stuff "rmmod $LINUX_POLLING\n"
sleep 3
screen -S fsim0 -X stuff "$FPGA_PARSER >> /root/Threadlet-AE/result/linux-bi\n"
sleep 3

# Heavy-tailed
screen -S fsim0 -X stuff "echo \"p99slowdown,p99latency,throughput\" > /root/Threadlet-AE/result/linux-ht\n"

# Heavy-tailed set1
screen -S fsim0 -X stuff "insmod $LINUX_POLLING\n"
sleep 3
sudo $RPC_HT -k 1 -s 1000
sleep 3
screen -S fsim0 -X stuff "rmmod $LINUX_POLLING\n"
sleep 3
screen -S fsim0 -X stuff "$FPGA_PARSER >> /root/Threadlet-AE/result/linux-ht\n"
sleep 3

# Heavy-tailed set2

screen -S fsim0 -X stuff "insmod $LINUX_POLLING\n"
sleep 3
sudo $RPC_HT -k 1 -s 500
sleep 3
screen -S fsim0 -X stuff "rmmod $LINUX_POLLING\n"
sleep 3
screen -S fsim0 -X stuff "$FPGA_PARSER >> /root/Threadlet-AE/result/linux-ht\n"
sleep 3

# Heavy-tailed set3
screen -S fsim0 -X stuff "insmod $LINUX_POLLING\n"
sleep 3
sudo $RPC_HT -k 2 -s 1000
sleep 3
screen -S fsim0 -X stuff "rmmod $LINUX_POLLING\n"
sleep 3
screen -S fsim0 -X stuff "$FPGA_PARSER >> /root/Threadlet-AE/result/linux-ht\n"
sleep 3


# kv
screen -S fsim0 -X stuff "echo \"p99slowdown,p99latency,throughput\" >> /root/Threadlet-AE/result/linux-ht\n"

# kv set1
screen -S fsim0 -X stuff "insmod $LINUX_POLLING\n"
sleep 3
sudo $RPC_KV -k 1 -s 1000
sleep 3
screen -S fsim0 -X stuff "rmmod $LINUX_POLLING\n"
sleep 3
screen -S fsim0 -X stuff "$FPGA_PARSER >> /root/Threadlet-AE/result/linux-kv\n"
sleep 3

# kv set2

screen -S fsim0 -X stuff "insmod $LINUX_POLLING\n"
sleep 3
sudo $RPC_KV -k 1 -s 500
sleep 3
screen -S fsim0 -X stuff "rmmod $LINUX_POLLING\n"
sleep 3
screen -S fsim0 -X stuff "$FPGA_PARSER >> /root/Threadlet-AE/result/linux-kv\n"
sleep 3

# kv set3
screen -S fsim0 -X stuff "insmod $LINUX_POLLING\n"
sleep 3
sudo $RPC_KV -k 2 -s 0
sleep 3
screen -S fsim0 -X stuff "rmmod $LINUX_POLLING\n"
sleep 3
screen -S fsim0 -X stuff "$FPGA_PARSER >> /root/Threadlet-AE/result/linux-kv\n"
sleep 3


# Run Shinjuku

# Bimodal
screen -S fsim0 -X stuff "echo \"p99slowdown,p99latency,throughput\" >> /root/Threadlet-AE/result/shinjuku-bi\n"
# Bimodal set1

screen -S fsim0 -X stuff "insmod $SHINJUKU\n"
sleep 3
sudo $RPC_BI -k 1 -s 3000
sleep 3
screen -S fsim0 -X stuff "rmmod $SHINJUKU\n"
sleep 3
screen -S fsim0 -X stuff "$FPGA_PARSER >> /root/Threadlet-AE/result/shinjuku-bi\n"
sleep 3

# Bimodal set2

screen -S fsim0 -X stuff "insmod $SHINJUKU\n"
sleep 3
sudo $RPC_BI -k 1 -s 2000
sleep 3
screen -S fsim0 -X stuff "rmmod $SHINJUKU\n"
sleep 3
screen -S fsim0 -X stuff "$FPGA_PARSER >> /root/Threadlet-AE/result/shinjuku-bi\n"
sleep 3

# Bimodal set3
screen -S fsim0 -X stuff "insmod $SHINJUKU\n"
sleep 3
sudo $RPC_BI -k 1 -s 1400
sleep 3
screen -S fsim0 -X stuff "rmmod $SHINJUKU\n"
sleep 3
screen -S fsim0 -X stuff "$FPGA_PARSER >> /root/Threadlet-AE/result/shinjuku-bi\n"
sleep 3

# Heavy-tailed
screen -S fsim0 -X stuff "echo \"p99slowdown,p99latency,throughput\" >> /root/Threadlet-AE/result/shinjuku-ht\n"

# Heavy-tailed set1

screen -S fsim0 -X stuff "insmod $SHINJUKU\n"
sleep 3
sudo $RPC_HT -k 1 -s 3000
sleep 3
screen -S fsim0 -X stuff "rmmod $SHINJUKU\n"
sleep 3
screen -S fsim0 -X stuff "$FPGA_PARSER >> /root/Threadlet-AE/result/shinjuku-ht\n"
sleep 3

# Heavy-tailed set2

screen -S fsim0 -X stuff "insmod $SHINJUKU\n"
sleep 3
sudo $RPC_HT -k 1 -s 0
sleep 3
screen -S fsim0 -X stuff "rmmod $SHINJUKU\n"
sleep 3
screen -S fsim0 -X stuff "$FPGA_PARSER >> /root/Threadlet-AE/result/shinjuku-ht\n"
sleep 3

# Heavy-tailed set3
screen -S fsim0 -X stuff "insmod $SHINJUKU\n"
sleep 3
sudo $RPC_HT -k 2 -s 0
sleep 3
screen -S fsim0 -X stuff "rmmod $SHINJUKU\n"
sleep 3
screen -S fsim0 -X stuff "$FPGA_PARSER >> /root/Threadlet-AE/result/shinjuku-ht\n"
sleep 3

# Heavy-tailed set4
screen -S fsim0 -X stuff "insmod $SHINJUKU\n"
sleep 3
sudo $RPC_HT -k 5 -s 0
sleep 3
screen -S fsim0 -X stuff "rmmod $SHINJUKU\n"
sleep 3
screen -S fsim0 -X stuff "$FPGA_PARSER >> /root/Threadlet-AE/result/shinjuku-ht\n"
sleep 3


# kv

screen -S fsim0 -X stuff "echo \"p99slowdown,p99latency,throughput\" >> /root/Threadlet-AE/result/shinjuku-kv\n"

# kv set1

screen -S fsim0 -X stuff "insmod $SHINJUKU\n"
sleep 3
sudo $RPC_KV -k 1 -s 3000
sleep 3
screen -S fsim0 -X stuff "rmmod $SHINJUKU\n"
sleep 3
screen -S fsim0 -X stuff "$FPGA_PARSER >> /root/Threadlet-AE/result/shinjuku-kv\n"
sleep 3

# kv set2

screen -S fsim0 -X stuff "insmod $SHINJUKU\n"
sleep 3
sudo $RPC_KV -k 1 -s 0
sleep 3
screen -S fsim0 -X stuff "rmmod $SHINJUKU\n"
sleep 3
screen -S fsim0 -X stuff "$FPGA_PARSER >> /root/Threadlet-AE/result/shinjuku-kv\n"
sleep 3

# kv set3
screen -S fsim0 -X stuff "insmod $SHINJUKU\n"
sleep 3
sudo $RPC_KV -k 2 -s 0
sleep 3
screen -S fsim0 -X stuff "rmmod $SHINJUKU\n"
sleep 3
screen -S fsim0 -X stuff "$FPGA_PARSER >> /root/Threadlet-AE/result/shinjuku-kv\n"
sleep 3

# kv set4
screen -S fsim0 -X stuff "insmod $SHINJUKU\n"
sleep 3
sudo $RPC_KV -k 5 -s 0
sleep 3
screen -S fsim0 -X stuff "rmmod $SHINJUKU\n"
sleep 3
screen -S fsim0 -X stuff "$FPGA_PARSER >> /root/Threadlet-AE/result/shinjuku-kv\n"
sleep 3

extract_csv_data() {
  local input_screenshot="$1"
  local output_csv="$2"

  if [[ -z "$input_screenshot" || -z "$output_csv" ]]; then
    echo "Error: Missing arguments!" >&2
    return 1
  fi
  if [[ ! -f "$input_screenshot" ]]; then
    echo "Error: Input file '$input_screenshot' does not exist!" >&2
    return 1
  fi

  local output_dir
  output_dir=$(dirname "$output_csv")
  mkdir -p "$output_dir"

  tac "$input_screenshot" | awk '
    state == 0 && /^[0-9]+,[0-9]+,[0-9]+/ {
      print $0
      state = 1
      next
    }
    
    state == 1 {
      if (/^[0-9]+,[0-9]+,[0-9]+/) {
        print $0
      } 
      else if (/p99slowdown,p99latency,throughput/) {
        print $0
        exit
      }
    }
  ' | tac > "$output_csv"
}

# cp linux
screen -S fsim0 -X stuff "cat /root/Threadlet-AE/result/linux-bi\n"
sleep 1
screen -S fsim0 -X hardcopy "/tmp/threadlet-ae-fig10-linux-bi"
extract_csv_data "/tmp/threadlet-ae-fig10-linux-bi" "$AE_HOME/result/fig10/linux-bi.csv"

screen -S fsim0 -X stuff "cat /root/Threadlet-AE/result/linux-ht\n"
sleep 1
screen -S fsim0 -X hardcopy "/tmp/threadlet-ae-fig10-linux-ht"
extract_csv_data "/tmp/threadlet-ae-fig10-linux-ht" "$AE_HOME/result/fig10/linux-ht.csv"

screen -S fsim0 -X stuff "cat /root/Threadlet-AE/result/linux-kv\n"
sleep 1
screen -S fsim0 -X hardcopy "/tmp/threadlet-ae-fig10-linux-kv"
extract_csv_data "/tmp/threadlet-ae-fig10-linux-kv" "$AE_HOME/result/fig10/linux-kv.csv"

# cp shinjuku
screen -S fsim0 -X stuff "cat /root/Threadlet-AE/result/shinjuku-bi\n"
sleep 1
screen -S fsim0 -X hardcopy "/tmp/threadlet-ae-fig10-shinjuku-bi"
extract_csv_data "/tmp/threadlet-ae-fig10-shinjuku-bi" "$AE_HOME/result/fig10/shinjuku-bi.csv"

screen -S fsim0 -X stuff "cat /root/Threadlet-AE/result/shinjuku-ht\n"
sleep 1
screen -S fsim0 -X hardcopy "/tmp/threadlet-ae-fig10-shinjuku-ht"
extract_csv_data "/tmp/threadlet-ae-fig10-shinjuku-ht" "$AE_HOME/result/fig10/shinjuku-ht.csv"

screen -S fsim0 -X stuff "cat /root/Threadlet-AE/result/shinjuku-kv\n"
sleep 1
screen -S fsim0 -X hardcopy "/tmp/threadlet-ae-fig10-shinjuku-kv"
extract_csv_data "/tmp/threadlet-ae-fig10-shinjuku-kv" "$AE_HOME/result/fig10/shinjuku-kv.csv"

# shut down fpga
# /home/qxh/Threadlet-AE/script/internal-tools/kill_fpga.sh