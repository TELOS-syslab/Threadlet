#!/usr/bin/env bash
set -euo pipefail

IFACE="tap0"
DEST_MAC="00:12:6D:00:00:02"
DEST_IP="172.16.0.2"
SRC_IP="172.16.0.1"

while getopts ":i:m:D:S:M:h" opt; do
  case $opt in
    i) IFACE="$OPTARG" ;;
    m) DEST_MAC="$OPTARG" ;;
    D) DEST_IP="$OPTARG" ;;
    S) SRC_IP="$OPTARG" ;;
    M) SRC_MAC="$OPTARG" ;;
    h|*)
      echo "Usage: sudo $0 [-i tap0] [-m <dest-mac>] [-D <dest-ip>] [-S <src-ip>] [-M <src-mac>]" >&2
      exit 2
      ;;
  esac
done

if [[ $EUID -ne 0 ]]; then
  echo "Please run as root" >&2
  exit 1
fi

if [[ -z "${IFACE}" ]]; then
  echo "-i <iface> is required (host NIC connected to FPGA)." >&2
  exit 2
fi

# Build sender tool
BUILD_DIR="$(dirname "$0")"
OUT_BIN="${BUILD_DIR}/fpga_icenet_send"

if ! command -v gcc >/dev/null 2>&1; then
  echo "gcc not found; please install a C compiler." >&2
  exit 1
fi

echo "[build] compiling tools/fpga_icenet_send.c -> ${OUT_BIN}"
gcc -O2 -Wall -o "${OUT_BIN}" "${BUILD_DIR}/fpga_icenet_send.c"

# Ensure TAP exists (do not auto-create)
if ! ip link show "${IFACE}" >/dev/null 2>&1; then
  echo "[error] interface '${IFACE}' not found. Please create TAP first (e.g. 'ip tuntap add mode tap dev ${IFACE} user $USER' and bring it up)." >&2
  exit 1
fi

# Optionally set a specific source MAC if provided
if [[ -n "${SRC_MAC:-}" ]]; then
  echo "[tap] set ${IFACE} mac=${SRC_MAC}"
  ip link set dev "${IFACE}" address "${SRC_MAC}"
fi

# Bring up interface and configure IP if not present
ip link set dev "${IFACE}" up
if ! ip -4 addr show dev "${IFACE}" | grep -q "${SRC_IP}/"; then
  # Respect the 172.16.0.0/16 topology used by the host TAP
  echo "[net] add ${SRC_IP}/16 to ${IFACE}"
  ip addr add "${SRC_IP}/16" dev "${IFACE}" || true
fi


for i in {1..10}; do
  "${OUT_BIN}" -i "${IFACE}" -d "${DEST_MAC}" -D "${DEST_IP}" -S "${SRC_IP}" -p 5556 -s "okok${i}"
done

