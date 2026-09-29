# Threadlet

The artifact of the SOSP '26 paper "Computation Is Fast, Use Threadlet!".



## Overview

Threadlet is a hardware-supported thread abstraction implemented in a Rocket-based processor and used by LoomOS, an Asterinas-based operating system.


## Repository Structure

```text
Threadlet/
|-- chipyard/                       Hardware configuration and integration
|   `-- generators/rocket-chip/     Threadlet CPU hardware implementation
|-- loomOS/                         Asterinas-based Threadlet OS
|   |-- kernel/                     LoomOS kernel source
|   |-- ostd/                       Architecture and low-level OS support
|   |-- eval/                       FPGA experiment launchers and log analyzers
|   `-- tools/                      FPGA boot configuration and utilities
|-- benchmark/                      Workload and benchmark source code
|-- script/                         Artifact experiment and plotting entry points
|-- result/                         Parsed CSV results and generated PNG figures
`-- README.md
```


## Evaluation Guide

See [the evaluation guide](./EVALUATION_GUIDE.md) for the instructions.



