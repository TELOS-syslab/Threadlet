#!/usr/bin/env bash
set -eo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
source "${SCRIPT_DIR}/../../script/internal-tools/env_fig13.sh"
source "${CHIPYARD_HOME}/env.sh"
source "${SCRIPT_DIR}/../../script/internal-tools/env_fig13.sh"
RISCV_OBJCOPY="$(command -v "${RISCV_OBJCOPY:-riscv64-unknown-elf-objcopy}")"
cd "${SCRIPT_DIR}"

# Use your own base
export MYBASE=asterinasThreadlet-base.json

echo "---- Stage 1: Build asterinas image"
echo "---- Be sure that you have recomilped the image of asterinas in docker"
sudo env RISCV_OBJCOPY="${RISCV_OBJCOPY}" "${ASTER_HOME}/tools/build_bin_fig13.sh"

# Restore the active configuration only after FireSim has finished collecting logs.
cp "${CONFIG_PATH}/config_runtime.yaml" "${CONFIG_PATH}/config_runtime_fig13.yaml.bak"
restore_config() {
    local status=$?
    trap - EXIT
    if (( status != 0 )); then "${ASTER_HOME}/eval/kill_fpga_fig13.sh" || true; fi
    cp "${CONFIG_PATH}/config_runtime_fig13.yaml.bak" "${CONFIG_PATH}/config_runtime.yaml"
    exit "${status}"
}
trap restore_config EXIT
trap 'exit 130' INT
trap 'exit 143' TERM
python3 - "${SCRIPT_DIR}/tmp.yaml" "${CONFIG_PATH}/config_runtime.yaml" "${FIRESIM_RUNS_DIR}" <<'PY_CONFIG'
import re, sys
from pathlib import Path
text = Path(sys.argv[1]).read_text()
text, count = re.subn(r'(?m)^(\s*default_simulation_dir:)\s*.*$', lambda m: m[1] + ' ' + sys.argv[3], text)
if count != 1: raise SystemExit('expected one default_simulation_dir')
Path(sys.argv[2]).write_text(text)
PY_CONFIG

# Modify your tmp.json
cp "${SCRIPT_DIR}/tmp_fig13.json" "${FIREMARSHAL_HOME}/boards/firechip/base-workloads/${MYBASE}"


echo "---- Stage 2: Combile asterinas image with OpenSBI"
cd "${FIREMARSHAL_HOME}"
./marshal build "${MYBASE}"
./marshal install "${MYBASE}"



echo "---- Stage 3: Deliver the image and bitstream to FPGA"
if [[ -z "${SSH_AUTH_SOCK:-}" ]]; then eval "$(ssh-agent -s)"; fi
ssh-add "${FIRESIM_SSH_KEY}"
sudo chmod 666 /dev/xdma*
cd "${FIRESIM_HOME}"
source sourceme-manager.sh --skip-ssh-setup
cd "${FIRESIM_HOME}"
firesim infrasetup -a "${CY_DIR}/sims/firesim-staging/sample_config_hwdb.yaml" -r "${CY_DIR}/sims/firesim-staging/sample_config_build_recipes.yaml"


echo "---- Setting up tap0 network interface"
if ! ip link show tap0 >/dev/null 2>&1; then
    sudo ip tuntap add mode tap dev tap0 user "$USER"
fi
sudo ip link set tap0 up
sudo ip addr replace 172.16.0.1/16 dev tap0
sudo ifconfig tap0 hw ether 8e:6b:35:04:00:00
sudo sysctl -w net.ipv6.conf.tap0.disable_ipv6=1

sudo sysctl -w net.ipv4.ip_forward=1
sudo iptables -A FORWARD -i eno1 -o tap0 -m state --state RELATED,ESTABLISHED -j ACCEPT
sudo iptables -A FORWARD -i tap0 -o eno1 -j ACCEPT
sudo iptables -t nat -A POSTROUTING -o eno1 -j MASQUERADE

TOOLS_DIR=$FIRESIM_HOME/tools
INFRASETUP_DIR=$FIRESIM_HOME/target-design/switch/$(ls $FIRESIM_HOME/target-design/switch/ -t | head -n 1)

cp "${ASTER_HOME}/tools/switchconfig.h" "${INFRASETUP_DIR}/switchconfig.h"
cp "${ASTER_HOME}/tools/switch.cc" "${INFRASETUP_DIR}/switch.cc"
# cp $ASTER_HOME/tools/sshport.h $INFRASETUP_DIR/sshport.h

cd "${INFRASETUP_DIR}"
make
cp switch switch0
scp "${INFRASETUP_DIR}/switch0" "localhost:${FIRESIM_RUNS_DIR}/switch_slot_0/switch0"


echo "---- Open another terminal and enter \"screen -r fsim0\""
cd "${FIRESIM_HOME}"
firesim runworkload -a "${CY_DIR}/sims/firesim-staging/sample_config_hwdb.yaml" -r "${CY_DIR}/sims/firesim-staging/sample_config_build_recipes.yaml"
