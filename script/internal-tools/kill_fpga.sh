#!/bin/bash

# chipyard home
export CHIPYARD_HOME=~/Threadlet-AE/chipyard
export FIREMARSHAL_HOME=$CHIPYARD_HOME/software/firemarshal
export FIRESIM_HOME=$CHIPYARD_HOME/sims/firesim

cd $FIRESIM_HOME && source sourceme-manager.sh && firesim kill
