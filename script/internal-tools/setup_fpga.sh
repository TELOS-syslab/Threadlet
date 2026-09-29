#!/bin/bash


# *** This script is for running Linux ***
# *** If you want to run LoomOS, refer to the $Threadlet-HOME/loomOS/eval ***

# chipyard home
export CHIPYARD_HOME=~/Threadlet-AE/chipyard
export FIREMARSHAL_HOME=$CHIPYARD_HOME/software/firemarshal
export FIRESIM_HOME=$CHIPYARD_HOME/sims/firesim
export CONFIG_PATH=/home/qxh/CHIPYARD_TEST/chipyard/sims/firesim/deploy/

# linux img home
export VMLINUX_PATH=$CHIPYARD_HOME/software/firemarshal/images/firechip/br-base
export MOUNTPOINT=$VMLINUX_PATH/../mnt
export FPGA_HOME=$VMLINUX_PATH/../mnt/root/

export THREADLET_HOME=~/Threadlet-AE
export BENCHMARK_HOME=~/Threadlet-AE/benchmark

cd /home/qxh/.ssh && ssh-agent -s > AGENT_VARS
cd /home/qxh/.ssh && source AGENT_VARS
cd /home/qxh/.ssh && ssh-add firesim.pem
sudo chmod 666 /dev/xdma*
cd $FIRESIM_HOME && source sourceme-manager.sh --skip-ssh-setup

# Linux configuration
cp $CONFIG_PATH/config_runtime.yaml $THREADLET_HOME/script/internal-tools/config_runtime.yaml.cp
cp $THREADLET_HOME/script/internal-tools/linux.yaml $CONFIG_PATH/config_runtime.yaml

# Preparing the img running on fpga-os
sudo mount $VMLINUX_PATH/br-base.img $MOUNTPOINT
mkdir -p $FPGA_HOME/Threadlet-AE
mkdir -p $FPGA_HOME/Threadlet-AE/tool

# Copy tool scripts to fpga
export FPGA_BENCH_SCRIPT=$THREADLET_HOME/script/internal-tools
cp $FPGA_BENCH_SCRIPT/fpga* $FPGA_HOME/Threadlet-AE/tool

# Copy all the benchmarks to fpga
mkdir -p $FPGA_HOME/Threadlet-AE/mutex
export MUTEX_BENCHMARK=$BENCHMARK_HOME/mutex/build
cp $MUTEX_BENCHMARK/* $FPGA_HOME/Threadlet-AE/mutex

mkdir -p $FPGA_HOME/Threadlet-AE/task_over
export TASK_BENCHMARK=$BENCHMARK_HOME/task_over/build
cp $TASK_BENCHMARK/* $FPGA_HOME/Threadlet-AE/task_over

mkdir -p $FPGA_HOME/Threadlet-AE/ipi
export IPI_BENCHMARK=$BENCHMARK_HOME/ipi/ipi.ko
cp $IPI_BENCHMARK $FPGA_HOME/Threadlet-AE/ipi

mkdir -p $FPGA_HOME/Threadlet-AE/interrupt
export INTR_BENCHMARK=$BENCHMARK_HOME/interrupt/build
cp $INTR_BENCHMARK/* $FPGA_HOME/Threadlet-AE/interrupt

mkdir -p $FPGA_HOME/Threadlet-AE/network
export INTR_BENCHMARK=$BENCHMARK_HOME/network/
cp $INTR_BENCHMARK/*.ko $FPGA_HOME/Threadlet-AE/network

sudo umount $VMLINUX_PATH/../mnt

# sync with fpga
cd $FIRESIM_HOME && firesim infrasetup -a ${CY_DIR}/sims/firesim-staging/sample_config_hwdb.yaml -r ${CY_DIR}/sims/firesim-staging/sample_config_build_recipes.yaml

# Setup the network for fpga
TAP0_STATUS=$(ifconfig tap0 2>/dev/null)
if [ -z "$TAP0_STATUS" ]; then
    sudo ip tuntap add mode tap dev tap0 user $USER
    sudo ip link set tap0 up
    sudo ip addr add 172.16.0.1/16 dev tap0
    sudo ifconfig tap0 hw ether 8e:6b:35:04:00:00
    sudo sysctl -w net.ipv6.conf.tap0.disable_ipv6=1

    sudo sysctl -w net.ipv4.ip_forward=1
    sudo iptables -A FORWARD -i eno1 -o tap0 -m state --state RELATED,ESTABLISHED -j ACCEPT
    sudo iptables -A FORWARD -i tap0 -o eno1 -j ACCEPT
    sudo iptables -t nat -A POSTROUTING -o eno1 -j MASQUERADE
else
    echo "tap0 is built; continue"
fi

TOOLS_DIR=$FIRESIM_HOME/tools
INFRASETUP_DIR=$FIRESIM_HOME/target-design/switch/$(ls $FIRESIM_HOME/target-design/switch/ -t | head -n 1)

cp $TOOLS_DIR/switchconfig.h $INFRASETUP_DIR/switchconfig.h
cd $INFRASETUP_DIR && make && cp switch switch0
scp $INFRASETUP_DIR/switch0 localhost:/home/qxh/FIRESIM_RUNS_DIR/switch_slot_0/switch0

# This step is for fpga setup
cd $FIRESIM_HOME && firesim runworkload -a ${CY_DIR}/sims/firesim-staging/sample_config_hwdb.yaml -r ${CY_DIR}/sims/firesim-staging/sample_config_build_recipes.yaml
