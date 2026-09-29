#!/usr/bin/env bash
set -euo pipefail

project_root="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "${project_root}"

dtc -I dts -O dtb -o tools/qemu_2c_icenet.dtb tools/qemu_2c_icenet.dts

kernel_elf="${project_root}/target/osdk/aster-nix-osdk-bin.qemu_elf"
kernel_bin="${project_root}/target/osdk/aster-nix-osdk.bin"
initrd_path="${project_root}/test/build/initramfs.cpio.gz"
dtb_path="${project_root}/tools/qemu_2c_icenet.dtb"
opensbi_dir="${project_root}/opensbi_qemu"
opensbi_fw_bin="${opensbi_dir}/build/platform/generic/firmware/fw_jump.bin"
opensbi_fw_payload="${opensbi_dir}/build/platform/generic/firmware/fw_payload.bin"

command_exists() {
    command -v "$1" >/dev/null 2>&1
}

find_objcopy() {
    if [[ -n "${OBJCOPY:-}" ]] && command_exists "${OBJCOPY}"; then
        echo "${OBJCOPY}"
        return
    fi

    if [[ -n "${CROSS_COMPILE:-}" ]] && command_exists "${CROSS_COMPILE}objcopy"; then
        echo "${CROSS_COMPILE}objcopy"
        return
    fi

    local candidates=(
        rust-objcopy
        llvm-objcopy
        riscv64-unknown-elf-objcopy
        riscv64-unknown-linux-gnu-objcopy
        riscv64-linux-gnu-objcopy
        objcopy
    )

    for tool in "${candidates[@]}"; do
        if command_exists "${tool}"; then
            echo "${tool}"
            return
        fi
    done

    echo "Could not find a suitable objcopy; please install riscv64-unknown-elf-binutils or llvm-objcopy." >&2
    exit 1
}

find_cross_prefix() {
    if [[ -n "${CROSS_COMPILE:-}" ]] && command_exists "${CROSS_COMPILE}gcc"; then
        echo "${CROSS_COMPILE}"
        return
    fi

    local prefixes=(
        riscv64-unknown-elf-
        riscv64-unknown-linux-gnu-
        riscv64-linux-gnu-
    )

    for prefix in "${prefixes[@]}"; do
        if command_exists "${prefix}gcc"; then
            echo "${prefix}"
            return
        fi
    done

    echo ""
}

echo "[1/4] Building Asterinas kernel and initramfs..."
# Force unwind tables and frame pointers to improve backtrace quality on real hardware.
# Note: keep flags here instead of embedding them into build system sources.
# Threadlet use target-feature=-c to disable compressed instructions.
RUSTFLAGS="--cfg netdebug -C force-unwind-tables=yes -C force-frame-pointers=yes -C target-feature=-c" \
    make build RELEASE=1 ARCH=riscv64 SCHEME=riscv

if [[ ! -f "${kernel_elf}" ]]; then
    echo "Kernel ELF ${kernel_elf} not found after build." >&2
    exit 1
fi

objcopy_bin="$(find_objcopy)"
echo "[2/4] Converting ${kernel_elf} to raw binary with ${objcopy_bin}..."
"${objcopy_bin}" --binary-architecture=riscv64 -O binary "${kernel_elf}" "${kernel_bin}"

cross_prefix="$(find_cross_prefix)"
if [[ -z "${cross_prefix}" ]]; then
    echo "No RISC-V cross compiler detected. Set CROSS_COMPILE or install riscv64-unknown-elf-gcc." >&2
    exit 1
fi

echo "[3/4] Building OpenSBI fw_jump (PLATFORM=generic, FW_JUMP_ADDR=0x80200000)..."
make -C "${opensbi_dir}" \
    ARCH=riscv \
    PLATFORM=generic \
    CROSS_COMPILE="${cross_prefix}" \
    FW_PAYLOAD_PATH="${kernel_bin}" \
    FW_PAYLOAD_OFFSET=0x200000
# make -C "${opensbi_dir}" ARCH=riscv PLATFORM=generic FW_JUMP_ADDR=0x80200000 CROSS_COMPILE="${cross_prefix}"

if [[ ! -f "${opensbi_fw_payload}" ]]; then
    echo "OpenSBI firmware ${opensbi_fw_payload} not found after build." >&2
    exit 1
fi

if [[ ! -f "${initrd_path}" ]]; then
    echo "Initramfs ${initrd_path} is missing; ensure test assets are built." >&2
    exit 1
fi

if [[ ! -f "${dtb_path}" ]]; then
    echo "Device tree ${dtb_path} not found." >&2
    exit 1
fi


echo "[4/4] Launching QEMU..."
qemu-system-riscv64 \
    -bios "${opensbi_fw_payload}" \
    -machine virt \
    -m 256M \
    -smp 2 \
    -cpu rv64,svpbmt=false,zba=true,zbb=true \
    -dtb "${dtb_path}" \
    -nographic \
    -display none \
    --no-reboot \
    -serial chardev:mux \
    -monitor chardev:mux \
    -chardev stdio,id=mux,mux=on,signal=off,logfile=qemu.log \
    -netdev user,id=n0,hostfwd=tcp::11180-:8080,hostfwd=udp::11181-:5555,hostfwd=udp::11182-:5556,hostfwd=udp::11183-:5557 \
    -net nic,model=icenet-mmio,netdev=n0,macaddr=52:54:00:12:34:56 \
    -d guest_errors -D qemu.log
