#!/bin/bash

export ASTER_HOME=/home/qxh/asterinas_threadlet
export CHIPYARD_HOME=/home/qxh/CHIPYARD_TEST/chipyard
export FIREMARSHAL_HOME=$CHIPYARD_HOME/software/firemarshal
export FIRESIM_HOME=$CHIPYARD_HOME/sims/firesim

# Use your own base
export MYBASE=asterinasThreadlet-base.json


# Modify your tmp.yaml
if [ -d "/home/qxh/CHIPYARD_TEST/chipyard/sims/firesim/deploy/config_runtime.yaml.bak" ]; then
    cp /home/qxh/CHIPYARD_TEST/chipyard/sims/firesim/deploy/config_runtime.yaml.bak /home/qxh/CHIPYARD_TEST/chipyard/sims/firesim/deploy/config_runtime.yaml
fi
mv /home/qxh/CHIPYARD_TEST/chipyard/sims/firesim/deploy/config_runtime.yaml /home/qxh/CHIPYARD_TEST/chipyard/sims/firesim/deploy/config_runtime.yaml.bak
cp ./tmp.yaml /home/qxh/CHIPYARD_TEST/chipyard/sims/firesim/deploy/config_runtime.yaml

# Modify your tmp.json
cp tmp.json /home/qxh/CHIPYARD_TEST/chipyard/software/firemarshal/boards/firechip/base-workloads/$MYBASE

source $CHIPYARD_HOME/env.sh
cd $FIRESIM_HOME && source sourceme-manager.sh --skip-ssh-setup

echo "---- Setting up tap0 network interface"
sudo ip link delete dev tap0
sudo ip tuntap add mode tap dev tap0 user $USER
sudo ip link set tap0 up
sudo ip addr add 172.16.0.1/16 dev tap0
sudo ifconfig tap0 hw ether 8e:6b:35:04:00:00
sudo sysctl -w net.ipv6.conf.tap0.disable_ipv6=1

sudo sysctl -w net.ipv4.ip_forward=1
sudo iptables -A FORWARD -i eno1 -o tap0 -m state --state RELATED,ESTABLISHED -j ACCEPT
sudo iptables -A FORWARD -i tap0 -o eno1 -j ACCEPT
sudo iptables -t nat -A POSTROUTING -o eno1 -j MASQUERADE

TOOLS_DIR=$FIRESIM_HOME/tools
INFRASETUP_DIR=$FIRESIM_HOME/target-design/switch/$(ls $FIRESIM_HOME/target-design/switch/ -t | head -n 1)

cp $ASTER_HOME/tools/switchconfig.h $INFRASETUP_DIR/switchconfig.h
cp $ASTER_HOME/tools/switch.cc $INFRASETUP_DIR/switch.cc
# cp $ASTER_HOME/tools/sshport.h $INFRASETUP_DIR/sshport.h

cd $INFRASETUP_DIR && make && cp switch switch0
scp $INFRASETUP_DIR/switch0 localhost:/home/qxh/FIRESIM_RUNS_DIR/switch_slot_0/switch0


echo "---- Open another terminal and enter \"screen -r fsim0\""
cd $FIRESIM_HOME && firesim runworkload -a ${CY_DIR}/sims/firesim-staging/sample_config_hwdb.yaml -r ${CY_DIR}/sims/firesim-staging/sample_config_build_recipes.yaml


cp /home/qxh/CHIPYARD_TEST/chipyard/sims/firesim/deploy/config_runtime.yaml.bak /home/qxh/CHIPYARD_TEST/chipyard/sims/firesim/deploy/config_runtime.yaml
