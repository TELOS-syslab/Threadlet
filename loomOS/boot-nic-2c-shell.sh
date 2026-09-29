#!/usr/bin/env bash
set -euo pipefail

# # Build kernel and initramfs
RUSTFLAGS="--cfg netdebug" make build ARCH=riscv64 SCHEME=riscv

# Run QEMU with virtio-net (MMIO) and user networking
qemu-system-riscv64 \
    -kernel ./target/osdk/aster-nix-osdk-bin.qemu_elf \
    -initrd ./test/build/initramfs.cpio.gz \
    -append "SHELL=/bin/sh LOGNAME=root HOME=/ USER=root PATH=/bin:/benchmark init=/usr/bin/busybox ostd.log_level=error -- sh" \
    -cpu rv64,zba=true,zbb=true,svpbmt=true \
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
    -netdev user,id=n0,hostfwd=tcp::10080-:8080,hostfwd=udp::10081-:5555,hostfwd=udp::10082-:5556,hostfwd=udp::10083-:5557 \
    -device virtio-net-device,netdev=n0,mac=52:54:00:12:34:56 \
    -d guest_errors -D qemu.log
    # Uncomment to dump packets for debugging
    # -object filter-dump,id=cap,netdev=n0,file=virtio-net.pcap
