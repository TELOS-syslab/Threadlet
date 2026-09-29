#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
AE_DIR="$(cd -- "${SCRIPT_DIR}/../.." && pwd)"
BUILD_DIR="${SCRIPT_DIR}/build"

TOOLCHAIN_NAME="riscv64-lp64d--glibc--stable-2021.11-1"
TOOLCHAIN_SHA256="70fe7d9fc74220b08b2ae0d3527641f4b938a1e4eb6bb305b2ac68fa76f2d6a4"
TOOLCHAIN_ARCHIVE="${AE_DIR}/third_party/toolchains/${TOOLCHAIN_NAME}.tar.bz2"
# Shared with benchmark/mutex-userspace/build.sh's extraction target: this
# toolchain is a large (~120MB compressed) read-only vendored SDK, and both
# benchmarks need the exact same pinned rv64gc/lp64d cross-compiler, so this
# script reuses the sibling's already-extracted+relocated copy when present
# instead of paying for a second multi-hundred-MB extraction. It is fully
# self-sufficient if that copy is not there yet: extraction and relocation
# below are idempotent (guarded by existence checks), and never write
# outside this gitignored build/ cache path.
TOOLCHAIN_ROOT="${AE_DIR}/benchmark/mutex-userspace/build/${TOOLCHAIN_NAME}"
MCS_B_CFLAGS=(-O2 -march=rv64gc -mabi=lp64d)
SOURCE_FILE="${SCRIPT_DIR}/mcs_b_fig13.c"
OUTPUT_FILE="${BUILD_DIR}/mcs_b_fig13"

CROSS_COMPILE="${TOOLCHAIN_ROOT}/bin/riscv64-buildroot-linux-gnu-"
CC="${CROSS_COMPILE}gcc"
READELF="${CROSS_COMPILE}readelf"

for tool in sha256sum tar bzip2 readlink file grep sed python3; do
    if ! command -v "${tool}" >/dev/null 2>&1; then
        echo "missing required build tool: ${tool}" >&2
        exit 1
    fi
done

mkdir -p "${BUILD_DIR}"
if [[ ! -f "${TOOLCHAIN_ARCHIVE}" ]]; then
    echo "missing offline RISC-V toolchain: ${TOOLCHAIN_ARCHIVE}" >&2
    echo "copy third_party/toolchains/${TOOLCHAIN_NAME}.tar.bz2 to the server" >&2
    exit 1
fi
echo "${TOOLCHAIN_SHA256}  ${TOOLCHAIN_ARCHIVE}" | sha256sum --check -

if [[ ! -x "${CC}" ]]; then
    mkdir -p "${AE_DIR}/benchmark/mutex-userspace/build"
    tar --extract --bzip2 --file "${TOOLCHAIN_ARCHIVE}" \
        --directory "${AE_DIR}/benchmark/mutex-userspace/build"
fi
if [[ ! -x "${TOOLCHAIN_ROOT}/relocate-sdk.sh" ]]; then
    echo "toolchain extraction did not produce relocate-sdk.sh" >&2
    exit 1
fi
"${TOOLCHAIN_ROOT}/relocate-sdk.sh"

for tool in "${CC}" "${READELF}"; do
    if [[ ! -x "${tool}" ]]; then
        echo "missing required RISC-V build tool: ${tool}" >&2
        exit 1
    fi
done

target_triple="$("${CC}" -dumpmachine)"
case "${target_triple}" in
    riscv64*-buildroot-linux-gnu*) ;;
    *)
        echo "expected the pinned RISC-V Linux compiler, got: ${target_triple}" >&2
        exit 1
        ;;
esac

if [[ ! -f "${SOURCE_FILE}" ]]; then
    echo "missing benchmark source: ${SOURCE_FILE}" >&2
    exit 1
fi

"${CC}" \
    "${MCS_B_CFLAGS[@]}" \
    -std=gnu11 \
    -static \
    -pthread \
    "${SOURCE_FILE}" \
    -o "${OUTPUT_FILE}"

if ! "${READELF}" -h "${OUTPUT_FILE}" |
    grep -Eq 'Machine:[[:space:]]+RISC-V'; then
    echo "mcs_b_fig13 is not a RISC-V ELF binary" >&2
    exit 1
fi
if "${READELF}" -l "${OUTPUT_FILE}" |
    grep -q 'Requesting program interpreter'; then
    echo "mcs_b_fig13 is dynamically linked" >&2
    exit 1
fi
riscv_attributes="$("${READELF}" -A "${OUTPUT_FILE}")"
if grep -Eq \
    'Tag_RISCV_arch:.*(_v[0-9]|_zve[0-9]|rv64[^"_[:space:]]*v)' \
    <<<"${riscv_attributes}"; then
    echo "mcs_b_fig13 contains an unsupported RISC-V vector ISA" >&2
    exit 1
fi
python3 "${SCRIPT_DIR}/check_riscv_no_vector.py" "${OUTPUT_FILE}"
echo "built ${OUTPUT_FILE}"
