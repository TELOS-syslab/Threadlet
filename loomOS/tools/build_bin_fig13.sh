#!/usr/bin/env bash
set -euo pipefail
SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
TARGET_DIR="${SCRIPT_DIR}/../target/osdk"
OBJCOPY="$(command -v "${RISCV_OBJCOPY:-riscv64-unknown-elf-objcopy}")"
[[ -x "${OBJCOPY}" ]] || { echo 'RISC-V objcopy not found; set RISCV_OBJCOPY' >&2; exit 1; }
"${OBJCOPY}" -O binary "${TARGET_DIR}/aster-nix-osdk-bin.qemu_elf" "${TARGET_DIR}/aster-nix-osdk-bin.bin"
