#!/bin/bash


# *** This script is for running Linux ***
# *** If you want to run LoomOS, refer to the $Threadlet-HOME/loomOS/eval ***

set -eo pipefail
SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
source "${SCRIPT_DIR}/env_fig13.sh"
if [[ "${FIG13_BASELINES:-0}" == 1 ]]; then
    for binary in "${BENCHMARK_HOME}/mutex-userspace/build/argobots_fig13" "${BENCHMARK_HOME}/mcs-b/build/mcs_b_fig13"; do
        [[ -x "${binary}" ]] || { echo "missing baseline: ${binary}" >&2; exit 1; }
    done
    for workers in 4 8 16 32; do
        [[ -f "${BENCHMARK_HOME}/mutex/build/mutex_${workers}.ko" ]] || exit 1
    done
fi
[[ -f "${FPGA_IMG}" ]] || { echo "missing image: ${FPGA_IMG}" >&2; exit 1; }
if [[ -z "${SSH_AUTH_SOCK:-}" ]]; then
    eval "$(ssh-agent -s)"
fi
ssh-add "${FIRESIM_SSH_KEY}"
sudo chmod 666 /dev/xdma*
cd "${FIRESIM_HOME}"
source sourceme-manager.sh --skip-ssh-setup
source "${SCRIPT_DIR}/env_fig13.sh"
MOUNTED=0
CONFIG_SAVED=0
cleanup() {
    local status=$?
    trap - EXIT
    if (( MOUNTED )); then sudo umount "${MOUNTPOINT}" || true; fi
    if (( CONFIG_SAVED )); then
        if (( status != 0 )); then "${SCRIPT_DIR}/kill_fpga_fig13.sh" || true; fi
        cp "${SCRIPT_DIR}/config_runtime_fig13.yaml.cp" "${CONFIG_PATH}/config_runtime.yaml"
    fi
    exit "${status}"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

# Linux configuration
cp "${CONFIG_PATH}/config_runtime.yaml" "${SCRIPT_DIR}/config_runtime_fig13.yaml.cp"
CONFIG_SAVED=1
python3 - "${SCRIPT_DIR}/linux.yaml" "${CONFIG_PATH}/config_runtime.yaml" "${FIRESIM_RUNS_DIR}" <<'PY_CONFIG'
import re, sys
from pathlib import Path
text = Path(sys.argv[1]).read_text()
text, count = re.subn(r'(?m)^(\s*default_simulation_dir:)\s*.*$', lambda m: m[1] + ' ' + sys.argv[3], text)
if count != 1: raise SystemExit('expected one default_simulation_dir')
Path(sys.argv[2]).write_text(text)
PY_CONFIG

# Stage the guest runners and freshly built baselines.
mkdir -p "${MOUNTPOINT}"
sudo mount "${FPGA_IMG}" "${MOUNTPOINT}"
MOUNTED=1
sudo mkdir -p "${FPGA_HOME}/Threadlet-AE/tool"
sudo cp "${BENCHMARK_HOME}/mutex/fpga_fig13.sh" \
    "${BENCHMARK_HOME}/mutex-userspace/fpga_argobots_fig13.sh" \
    "${BENCHMARK_HOME}/mcs-b/fpga_mcs_b.sh" \
    "${FPGA_HOME}/Threadlet-AE/tool/"
for benchmark in mutex mutex-userspace mcs-b; do
    case "${benchmark}" in
        mutex) files=("${BENCHMARK_HOME}/mutex/build/"*.ko) ;;
        mutex-userspace) files=("${BENCHMARK_HOME}/mutex-userspace/build/argobots_fig13") ;;
        mcs-b) files=("${BENCHMARK_HOME}/mcs-b/build/mcs_b_fig13") ;;
    esac
    for file in "${files[@]}"; do
        if [[ -f "${file}" ]]; then
            sudo mkdir -p "${FPGA_HOME}/Threadlet-AE/${benchmark}"
            sudo cp "${file}" "${FPGA_HOME}/Threadlet-AE/${benchmark}/"
        fi
    done
done
sudo umount "${MOUNTPOINT}"
MOUNTED=0

# sync with fpga
cd "${FIRESIM_HOME}"
firesim infrasetup -a "${CY_DIR}/sims/firesim-staging/sample_config_hwdb.yaml" -r "${CY_DIR}/sims/firesim-staging/sample_config_build_recipes.yaml"

# Setup the network for fpga
TAP0_STATUS=$(ifconfig tap0 2>/dev/null || true)
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

cp "${TOOLS_DIR}/switchconfig.h" "${INFRASETUP_DIR}/switchconfig.h"
cd "${INFRASETUP_DIR}"
make
cp switch switch0
scp "${INFRASETUP_DIR}/switch0" "localhost:${FIRESIM_RUNS_DIR}/switch_slot_0/switch0"

# This step is for fpga setup
cd "${FIRESIM_HOME}"
firesim runworkload -a "${CY_DIR}/sims/firesim-staging/sample_config_hwdb.yaml" -r "${CY_DIR}/sims/firesim-staging/sample_config_build_recipes.yaml"
