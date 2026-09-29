#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
AE_DIR="$(cd -- "${SCRIPT_DIR}/../.." && pwd)"
BUILD_DIR="${SCRIPT_DIR}/build"

ARGOBOTS_VERSION="1.2"
ARGOBOTS_SHA256="1c056429d9c0a27c041d4734f6318b801fc2ec671854e42c35251c4c7d0d43e1"
ARGOBOTS_ARCHIVE="${AE_DIR}/third_party/argobots/argobots-${ARGOBOTS_VERSION}.tar.gz"
ARGOBOTS_CROSS_PATCH="${AE_DIR}/third_party/argobots/argobots-1.2-cross-compile.patch"
TOOLCHAIN_NAME="riscv64-lp64d--glibc--stable-2021.11-1"
TOOLCHAIN_SHA256="70fe7d9fc74220b08b2ae0d3527641f4b938a1e4eb6bb305b2ac68fa76f2d6a4"
TOOLCHAIN_ARCHIVE="${AE_DIR}/third_party/toolchains/${TOOLCHAIN_NAME}.tar.bz2"
TOOLCHAIN_ROOT="${BUILD_DIR}/${TOOLCHAIN_NAME}"
ARGOBOTS_SOURCE="${BUILD_DIR}/argobots-${ARGOBOTS_VERSION}"
ARGOBOTS_PERF_BUILD="${BUILD_DIR}/argobots-${ARGOBOTS_VERSION}-perf-bootlin-rv64gc-build"
ARGOBOTS_PERF_INSTALL="${BUILD_DIR}/argobots-${ARGOBOTS_VERSION}-perf-bootlin-rv64gc-install"
ARGOBOTS_CROSS_PATCH_MARKER="${ARGOBOTS_SOURCE}/.threadlet-cross-compile-patched"
FIG13_BINARY="${BUILD_DIR}/argobots_fig13"
ARGOBOTS_TARGET_CFLAGS=(-O2 -march=rv64gc -mabi=lp64d)

ARGOBOTS_CROSS_COMPILE="${TOOLCHAIN_ROOT}/bin/riscv64-buildroot-linux-gnu-"
ARGOBOTS_CC="${ARGOBOTS_CROSS_COMPILE}gcc"
ARGOBOTS_AR="${ARGOBOTS_CROSS_COMPILE}ar"
ARGOBOTS_RANLIB="${ARGOBOTS_CROSS_COMPILE}ranlib"
ARGOBOTS_READELF="${ARGOBOTS_CROSS_COMPILE}readelf"
ARGOBOTS_BUILD_JOBS="${ARGOBOTS_BUILD_JOBS:-$(getconf _NPROCESSORS_ONLN)}"

for tool in sha256sum tar bzip2 patch make readlink file grep sed python3; do
    if ! command -v "${tool}" >/dev/null 2>&1; then
        echo "missing required build tool: ${tool}" >&2
        exit 1
    fi
done

mkdir -p "${BUILD_DIR}"
if [[ ! -f "${ARGOBOTS_ARCHIVE}" ]]; then
    echo "missing offline Argobots archive: ${ARGOBOTS_ARCHIVE}" >&2
    echo "copy third_party/argobots/argobots-${ARGOBOTS_VERSION}.tar.gz to the server" >&2
    exit 1
fi
echo "${ARGOBOTS_SHA256}  ${ARGOBOTS_ARCHIVE}" | sha256sum --check -

if [[ ! -f "${TOOLCHAIN_ARCHIVE}" ]]; then
    echo "missing offline RISC-V toolchain: ${TOOLCHAIN_ARCHIVE}" >&2
    echo "copy third_party/toolchains/${TOOLCHAIN_NAME}.tar.bz2 to the server" >&2
    exit 1
fi
echo "${TOOLCHAIN_SHA256}  ${TOOLCHAIN_ARCHIVE}" | sha256sum --check -

if [[ ! -x "${ARGOBOTS_CC}" ]]; then
    tar --extract --bzip2 --file "${TOOLCHAIN_ARCHIVE}" \
        --directory "${BUILD_DIR}"
fi
if [[ ! -x "${TOOLCHAIN_ROOT}/relocate-sdk.sh" ]]; then
    echo "toolchain extraction did not produce relocate-sdk.sh" >&2
    exit 1
fi
"${TOOLCHAIN_ROOT}/relocate-sdk.sh"

for tool in "${ARGOBOTS_CC}" "${ARGOBOTS_AR}" "${ARGOBOTS_RANLIB}" \
    "${ARGOBOTS_READELF}"; do
    if [[ ! -x "${tool}" ]]; then
        echo "missing required RISC-V build tool: ${tool}" >&2
        exit 1
    fi
done

target_triple="$("${ARGOBOTS_CC}" -dumpmachine)"
case "${target_triple}" in
    riscv64*-buildroot-linux-gnu*) ;;
    *)
        echo "expected the pinned RISC-V Linux compiler, got: ${target_triple}" >&2
        exit 1
        ;;
esac

if [[ ! -x "${ARGOBOTS_SOURCE}/configure" ]]; then
    tar --extract --gzip --file "${ARGOBOTS_ARCHIVE}" --directory "${BUILD_DIR}"
fi
if [[ ! -x "${ARGOBOTS_SOURCE}/configure" ]]; then
    echo "Argobots source extraction did not produce configure" >&2
    exit 1
fi
if [[ ! -f "${ARGOBOTS_CROSS_PATCH}" ]]; then
    echo "missing Argobots cross-compile patch: ${ARGOBOTS_CROSS_PATCH}" >&2
    exit 1
fi
if [[ ! -f "${ARGOBOTS_CROSS_PATCH_MARKER}" ]]; then
    (
        cd "${ARGOBOTS_SOURCE}"
        patch --batch --forward -p1 < "${ARGOBOTS_CROSS_PATCH}"
    )
    touch "${ARGOBOTS_CROSS_PATCH_MARKER}"
fi
build_argobots_library() {
    local build_dir="$1"
    local install_dir="$2"
    local tool_flag="$3"
    local build_name="$4"
    local config="${build_dir}/src/include/abt_config.h"
    local build_cflags="${ARGOBOTS_TARGET_CFLAGS[*]}"

    mkdir -p "${build_dir}" "${install_dir}"
    if [[ ! -f "${build_dir}/Makefile" ]]; then
        (
            cd "${build_dir}"
            CC="${ARGOBOTS_CC}" \
            AR="${ARGOBOTS_AR}" \
            RANLIB="${ARGOBOTS_RANLIB}" \
            CFLAGS="${build_cflags}" \
            "${ARGOBOTS_SOURCE}/configure" \
                --host="${target_triple}" \
                --prefix="${install_dir}" \
                --disable-shared \
                --enable-static \
                --enable-affinity \
                "${tool_flag}" \
                --disable-fcontext \
                --disable-simple-mutex
        )
    fi

    make -C "${build_dir}" -j"${ARGOBOTS_BUILD_JOBS}"
    make -C "${build_dir}" install

    if [[ ! -f "${config}" ]]; then
        echo "missing generated Argobots config: ${config}" >&2
        exit 1
    fi
    if grep -Eq \
        '^[[:space:]]*#define[[:space:]]+ABT_CONFIG_USE_SIMPLE_MUTEX' \
        "${config}"; then
        echo "refusing ${build_name} with simple mutex enabled" >&2
        exit 1
    fi
    if ! grep -Eq \
        '^[[:space:]]*#define[[:space:]]+ABT_CONFIG_DISABLE_TOOL_INTERFACE' \
        "${config}"; then
        echo "Argobots tool interface is enabled in ${build_name}" >&2
        exit 1
    fi
    if grep -Eq \
        '^[[:space:]]*#define[[:space:]]+ABT_CONFIG_USE_FCONTEXT' \
        "${config}"; then
        echo "unexpected fcontext backend in ${build_name}" >&2
        exit 1
    fi

}

build_checked_binary() {
    local source_file="$1"
    local install_dir="$2"
    local output_file="$3"
    local binary_name="$4"

    "${ARGOBOTS_CC}" \
        "${ARGOBOTS_TARGET_CFLAGS[@]}" \
        -std=gnu11 \
        -static \
        -I"${install_dir}/include" \
        "${source_file}" \
        "${install_dir}/lib/libabt.a" \
        -pthread \
        -ldl \
        -lrt \
        -o "${output_file}"

    if ! "${ARGOBOTS_READELF}" -h "${output_file}" |
        grep -Eq 'Machine:[[:space:]]+RISC-V'; then
        echo "${binary_name} is not a RISC-V ELF binary" >&2
        exit 1
    fi
    if "${ARGOBOTS_READELF}" -l "${output_file}" |
        grep -q 'Requesting program interpreter'; then
        echo "${binary_name} is dynamically linked" >&2
        exit 1
    fi
    local riscv_attributes
    riscv_attributes="$("${ARGOBOTS_READELF}" -A "${output_file}")"
    if grep -Eq \
        'Tag_RISCV_arch:.*(_v[0-9]|_zve[0-9]|rv64[^"_[:space:]]*v)' \
        <<<"${riscv_attributes}"; then
        echo "${binary_name} contains an unsupported RISC-V vector ISA" >&2
        exit 1
    fi
    python3 "${SCRIPT_DIR}/check_riscv_no_vector.py" "${output_file}"
    echo "built ${output_file}"
}

build_argobots_library \
    "${ARGOBOTS_PERF_BUILD}" "${ARGOBOTS_PERF_INSTALL}" \
    --disable-tool "RISC-V performance build"
build_checked_binary \
    "${SCRIPT_DIR}/argobots_fig13.c" "${ARGOBOTS_PERF_INSTALL}" \
    "${FIG13_BINARY}" "argobots_fig13"
