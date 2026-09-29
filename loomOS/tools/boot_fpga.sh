#!/bin/bash
export ASTER_HOME=../
export CHIPYARD_HOME=/home/qxh/CHIPYARD_TEST/chipyard
export FIREMARSHAL_HOME=$CHIPYARD_HOME/software/firemarshal
export FIRESIM_HOME=$CHIPYARD_HOME/sims/firesim

# Use your own base
export MYBASE=asterinasAnother-base.json

echo "---- Stage 1: Build asterinas image"
echo "---- Be sure that you have recomilped the image of asterinas in docker"
sudo $ASTER_HOME/tools/build_bin.sh

# Modify your tmp.yaml
if [ -d "/home/qxh/CHIPYARD_TEST/chipyard/sims/firesim/deploy/config_runtime.yaml.bak" ]; then
    cp /home/qxh/CHIPYARD_TEST/chipyard/sims/firesim/deploy/config_runtime.yaml.bak /home/qxh/CHIPYARD_TEST/chipyard/sims/firesim/deploy/config_runtime.yaml
fi
mv /home/qxh/CHIPYARD_TEST/chipyard/sims/firesim/deploy/config_runtime.yaml /home/qxh/CHIPYARD_TEST/chipyard/sims/firesim/deploy/config_runtime.yaml.bak
cp ./tmp.yaml /home/qxh/CHIPYARD_TEST/chipyard/sims/firesim/deploy/config_runtime.yaml


# Modify your tmp.json
cp tmp.json /home/qxh/CHIPYARD_TEST/chipyard/software/firemarshal/boards/firechip/base-workloads/$MYBASE

echo "---- Stage 2: Combile asterinas image with OpenSBI"
source $CHIPYARD_HOME/env.sh
cd $FIREMARSHAL_HOME && ./marshal build $MYBASE && ./marshal install $MYBASE



echo "---- Stage 3: Deliver the image and bitstream to FPGA"
cd /home/qxh/.ssh && ssh-agent -s > AGENT_VARS
cd /home/qxh/.ssh && source AGENT_VARS
cd /home/qxh/.ssh && ssh-add firesim.pem
sudo chmod 666 /dev/xdma*
cd $FIRESIM_HOME && source sourceme-manager.sh --skip-ssh-setup
# cd $FIRESIM_HOME && firesim kill
cd $FIRESIM_HOME && firesim infrasetup -a ${CY_DIR}/sims/firesim-staging/sample_config_hwdb.yaml -r ${CY_DIR}/sims/firesim-staging/sample_config_build_recipes.yaml
echo "---- Open another terminal and enter \"screen -r fsim0\""





cd $FIRESIM_HOME && firesim runworkload -a ${CY_DIR}/sims/firesim-staging/sample_config_hwdb.yaml -r ${CY_DIR}/sims/firesim-staging/sample_config_build_recipes.yaml


cp /home/qxh/CHIPYARD_TEST/chipyard/sims/firesim/deploy/config_runtime.yaml.bak /home/qxh/CHIPYARD_TEST/chipyard/sims/firesim/deploy/config_runtime.yaml
