#!/bin/bash

export AE_HOME=~/Threadlet-AE
export CHIPYRAD_HOME=$AE_HOME/chipyard

source $CHIPYRAD_HOME/env.sh && riscv64-unknown-linux-gnu-gcc ./interrupt.c -o interrupt

make
cp interrupt build/ && cp sys_interrupt_helper.ko build/