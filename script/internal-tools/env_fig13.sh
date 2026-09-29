#!/usr/bin/env bash
# Shared host paths; callers may override server-specific locations.
AE_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd)"
export AE_DIR
export THREADLET_HOME="${AE_DIR}"
export AE_HOME="${AE_DIR}"
export ASTER_HOME="${AE_DIR}/loomOS"
export CHIPYARD_HOME="${CHIPYARD_HOME:-${AE_DIR}/chipyard}"
export CY_DIR="${CHIPYARD_HOME}"
export FIREMARSHAL_HOME="${FIREMARSHAL_HOME:-${CHIPYARD_HOME}/software/firemarshal}"
export FIRESIM_HOME="${FIRESIM_HOME:-${CHIPYARD_HOME}/sims/firesim}"
export CONFIG_PATH="${CONFIG_PATH:-${FIRESIM_HOME}/deploy}"
export VMLINUX_PATH="${VMLINUX_PATH:-${FIREMARSHAL_HOME}/images/firechip/br-base}"
export FPGA_IMG="${FPGA_IMG:-${VMLINUX_PATH}/br-base.img}"
export MOUNTPOINT="${MOUNTPOINT:-${FIREMARSHAL_HOME}/images/firechip/mnt}"
export FPGA_HOME="${MOUNTPOINT}/root"
export BENCHMARK_HOME="${AE_DIR}/benchmark"
export LINUXSRC="${LINUXSRC:-${FIREMARSHAL_HOME}/boards/default/linux}"
export FIRESIM_RUNS_DIR="${FIRESIM_RUNS_DIR:-/home/qxh/FIRESIM_RUNS_DIR}"
export FIRESIM_SSH_KEY="${FIRESIM_SSH_KEY:-${HOME}/.ssh/firesim.pem}"
export PYTHONDONTWRITEBYTECODE=1
