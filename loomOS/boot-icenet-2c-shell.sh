#!/usr/bin/env bash
set -euo pipefail

# Build kernel and initramfs with verbose net debug
# Enable Zicbo* target features so DmaStream::sync emits CBO when toolchain supports it
RUSTFLAGS="--cfg netdebug -C target-feature=+zicbom" make build ARCH=riscv64 SCHEME=riscv

# Run QEMU with icenet-mmio (MMIO NIC) and user networking
qemu-system-riscv64 \
    -kernel ./target/osdk/aster-nix-osdk-bin.qemu_elf \
    -initrd ./test/build/initramfs.cpio.gz \
    -append "SHELL=/bin/sh LOGNAME=root HOME=/ USER=root PATH=/bin:/benchmark init=/usr/bin/busybox ostd.log_level=error -- sh" \
    -cpu rv64,svpbmt=true,zba=true,zbb=true,zicbom=true,zicboz=true,zicbop=true,cbom_blocksize=64,cboz_blocksize=64 \
    -machine virt \
    -m 8G \
    -smp 2 \
    --no-reboot \
    -nographic \
    -display none \
    -serial chardev:mux \
    -monitor chardev:mux \
    -chardev stdio,id=mux,mux=on,signal=off,logfile=qemu.log \
    -drive if=none,format=raw,id=x0,file=./test/build/ext2.img \
    -drive if=none,format=raw,id=x1,file=./test/build/exfat.img \
    -device virtio-blk-device,drive=x0 \
    -device virtio-keyboard-device \
    -device virtio-serial-device \
    -device virtconsole,chardev=mux \
    -netdev user,id=n0,hostfwd=tcp::11180-:8080,hostfwd=udp::11181-:5555,hostfwd=udp::11182-:5556,hostfwd=udp::11183-:5557 \
    -net nic,model=icenet-mmio,netdev=n0,macaddr=52:54:00:12:34:56 \
    -d guest_errors -D qemu.log
    # Packets dump example:
    # -object filter-dump,id=cap,netdev=n0,file=icenet.pcap
