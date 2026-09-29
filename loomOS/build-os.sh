# Build the RISC-V kernel with extra compile-time cfgs.
#
# - `--cfg netdebug`: enables extra network debug prints.
# - `--cfg mcs_hw_log`: enables MCS(threadlet) HW logs.
# - `--cfg irqdebug`: threadlet irq debug logs.
# - `--cfg pulldebug`: fath path debug
# Example:
# `RUSTFLAGS="--cfg netdebug --cfg mcs_hw_log -C target-feature=-c" ./build-os.sh`
#
set -euo pipefail

DEFAULT_RUSTFLAGS="--cfg mcs_hw_log -C target-feature=-c"
RUSTFLAGS="${RUSTFLAGS:-$DEFAULT_RUSTFLAGS}" \
    make build RELEASE_LTO=1 ARCH=riscv64 SCHEME=riscv


for dts in tools/*.dts; do
    [ -e "$dts" ] || break
    dtb="${dts%.dts}.dtb"
    dtc -I dts -O dtb -o "$dtb" "$dts"
done
