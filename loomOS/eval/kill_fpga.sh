#!/bin/bash
export CHIPYARD_HOME=/home/qxh/CHIPYARD_TEST/chipyard
export FIREMARSHAL_HOME=$CHIPYARD_HOME/software/firemarshal
export FIRESIM_HOME=$CHIPYARD_HOME/sims/firesim

cd /home/qxh/.ssh && ssh-agent -s > AGENT_VARS
cd /home/qxh/.ssh && source AGENT_VARS
cd /home/qxh/.ssh && ssh-add firesim.pem
sudo chmod 666 /dev/xdma*
cd $FIRESIM_HOME && source sourceme-manager.sh --skip-ssh-setup
cd $FIRESIM_HOME && firesim kill
cp /home/qxh/FIRESIM_RUNS_DIR/sim_slot_0/uartlog ./fpga.log