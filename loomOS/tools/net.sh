#!/bin/bash
export CHIPYARD_HOME=/home/qxh/CHIPYARD_TEST/chipyard
export FIREMARSHAL_HOME=$CHIPYARD_HOME/software/firemarshal
export FIRESIM_HOME=$CHIPYARD_HOME/sims/firesim

# cd /home/qxh/.ssh && ssh-agent -s > AGENT_VARS
# cd /home/qxh/.ssh && source AGENT_VARS
# cd /home/qxh/.ssh && ssh-add firesim.pem
# sudo chmod 666 /dev/xdma*
# cd $FIRESIM_HOME && source sourceme-manager.sh --skip-ssh-setup
# cd $FIRESIM_HOME && firesim infrasetup -a ${CY_DIR}/sims/firesim-staging/sample_config_hwdb.yaml -r ${CY_DIR}/sims/firesim-staging/sample_config_build_recipes.yaml

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

# cd $FIRESIM_HOME && firesim runworkload -a ${CY_DIR}/sims/firesim-staging/sample_config_hwdb.yaml -r ${CY_DIR}/sims/firesim-staging/sample_config_build_recipes.yaml
