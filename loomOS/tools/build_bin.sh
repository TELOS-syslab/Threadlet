export ASTER_TARGER_PATH=../target/osdk
export ASTER_TARGER_RISCV_PATH=../target/riscv64gc-unknown-none-elf/release-lto
export RISCV_TOOl=/home/qxh/opt/riscv/riscv64/bin
$RISCV_TOOl/riscv64-unknown-elf-objcopy -O binary $ASTER_TARGER_PATH/aster-nix-osdk-bin.qemu_elf $ASTER_TARGER_PATH/aster-nix-osdk-bin.bin
# $RISCV_TOOl/riscv64-unknown-elf-objcopy -O binary $ASTER_TARGER_RISCV_PATH/aster-nix-osdk-bin $ASTER_TARGER_RISCV_PATH/aster-nix-osdk-bin.bin