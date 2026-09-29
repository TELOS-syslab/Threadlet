#!/usr/bin/env bash
set -euo pipefail

# Build kernel and initramfs
# Only enable Zicbom (hardware-supported); do NOT enable svpbmt.
RUSTFLAGS="--cfg netdebug -C target-feature=+zicbom" make build ARCH=riscv64 SCHEME=riscv

# QEMU launch: Rocket-like memory size and icenet MMIO address
# Use real DTB now; OS ignores DT 'interrupts' for icenet and assumes 12/13/14.

qemu-system-riscv64 \
    -kernel ./target/osdk/aster-nix-osdk-bin.qemu_elf \
    -initrd ./test/build/initramfs.cpio.gz \
    -dtb tools/WithIceNICRocketConfig.dtb \
    -append "SHELL=/bin/sh LOGNAME=root HOME=/ USER=root PATH=/bin:/benchmark init=/usr/bin/busybox ostd.log_level=error -- sh" \
    -cpu rv64,svpbmt=false,zba=true,zbb=true,zicbom=true,cbom_blocksize=64 \
    -machine virt \
    -m 256M \
    -smp 2 \
    --no-reboot \
    -nographic \
    -display none \
    -serial chardev:mux \
    -monitor chardev:mux \
    -chardev stdio,id=mux,mux=on,signal=off,logfile=qemu.log \
    -netdev user,id=n0,hostfwd=tcp::11180-:8080,hostfwd=udp::11181-:5555,hostfwd=udp::11182-:5556,hostfwd=udp::11183-:5557 \
    -net nic,model=icenet-mmio,netdev=n0,macaddr=52:54:00:12:34:56 \
    -d guest_errors -D qemu.log
    # To capture packets:
    # -object filter-dump,id=cap,netdev=n0,file=icenet.pcap
