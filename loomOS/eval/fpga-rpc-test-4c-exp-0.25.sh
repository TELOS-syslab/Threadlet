#!/usr/bin/env bash
set -euo pipefail

# we measure p99.9 tail latency and throughtput
# throughtput * 2
# throughput use requests per 1000000 cycles
# 0.3 mrps
IFACE="tap0"
DEST_MAC="00:12:6D:00:00:02"
DEST_IP="172.16.0.2"
SRC_IP="172.16.0.1"
MAX_RPC="500"
SCAN_FREQ="200"
ENABLE_WAIT="1"
WAIT_RUN_NS="70"
WAIT_LONG_MULTIPLIER="1000"
NUM_WARMUP="0"
SEND_SLEEP_US="10"

while getopts ":i:m:D:S:M:n:f:w:r:l:s:h" opt; do
  case $opt in
    i) IFACE="$OPTARG" ;;
    m) DEST_MAC="$OPTARG" ;;
    D) DEST_IP="$OPTARG" ;;
    S) SRC_IP="$OPTARG" ;;
    M) SRC_MAC="$OPTARG" ;;
    n) MAX_RPC="$OPTARG" ;;
    f) SCAN_FREQ="$OPTARG" ;;
    w) ENABLE_WAIT="$OPTARG" ;;
    r) WAIT_RUN_NS="$OPTARG" ;;
    l) WAIT_LONG_MULTIPLIER="$OPTARG" ;;
    s) SEND_SLEEP_US="$OPTARG" ;;
    h|*)
      echo "Usage: sudo $0 [-i tap0] [-m <dest-mac>] [-D <dest-ip>] [-S <src-ip>] [-M <src-mac>] [-n <max-rpc>] [-f <scan-freq>] [-w <enable-wait:0|1>] [-r <wait-run-ns>] [-l <wait-long-multiplier>] [-s <send-sleep-us>]" >&2
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

if ! [[ "${MAX_RPC}" =~ ^[0-9]+$ ]] || [[ "${MAX_RPC}" -le 0 ]]; then
  echo "-n <max-rpc> must be a positive integer." >&2
  exit 2
fi
if ! [[ "${SCAN_FREQ}" =~ ^[0-9]+$ ]] || [[ "${SCAN_FREQ}" -le 0 ]]; then
  echo "-f <scan-freq> must be a positive integer." >&2
  exit 2
fi
if ! [[ "${ENABLE_WAIT}" =~ ^[01]$ ]]; then
  echo "-w <enable-wait> must be 0 or 1." >&2
  exit 2
fi
if ! [[ "${WAIT_RUN_NS}" =~ ^[0-9]+$ ]]; then
  echo "-r <wait-run-ns> must be a non-negative integer." >&2
  exit 2
fi
if ! [[ "${WAIT_LONG_MULTIPLIER}" =~ ^[0-9]+$ ]] || [[ "${WAIT_LONG_MULTIPLIER}" -le 0 ]]; then
  echo "-l <wait-long-multiplier> must be a positive integer." >&2
  exit 2
fi
if ! [[ "${SEND_SLEEP_US}" =~ ^[0-9]+$ ]]; then
  echo "-s <send-sleep-us> must be a non-negative integer." >&2
  exit 2
fi

BUILD_DIR="$(dirname "$0")"
OUT_BIN="${BUILD_DIR}/fpga_rpc_test"

if ! command -v gcc >/dev/null 2>&1; then
  echo "gcc not found; please install a C compiler." >&2
  exit 1
fi

echo "[build] compiling tools/fpga_rpc_test.c -> ${OUT_BIN}"
gcc -O2 -Wall -o "${OUT_BIN}" "${BUILD_DIR}/fpga_rpc_test.c"

if ! ip link show "${IFACE}" >/dev/null 2>&1; then
  echo "[error] interface '${IFACE}' not found. Please create TAP first (e.g. 'ip tuntap add mode tap dev ${IFACE} user $USER' and bring it up)." >&2
  exit 1
fi

if [[ -n "${SRC_MAC:-}" ]]; then
  echo "[tap] set ${IFACE} mac=${SRC_MAC}"
  ip link set dev "${IFACE}" address "${SRC_MAC}"
fi

ip link set dev "${IFACE}" up
if ! ip -4 addr show dev "${IFACE}" | grep -q "${SRC_IP}/"; then
  echo "[net] add ${SRC_IP}/16 to ${IFACE}"
  ip addr add "${SRC_IP}/16" dev "${IFACE}" || true
fi

echo "[send-rpc] iface=${IFACE} dst-mac=${DEST_MAC} dst-ip=${DEST_IP} src-ip=${SRC_IP} dst-port=5555 max-rpc=${MAX_RPC} scan-freq=${SCAN_FREQ} enable_wait=${ENABLE_WAIT} wait_run_ns=${WAIT_RUN_NS} wait_long_multiplier=${WAIT_LONG_MULTIPLIER} send_sleep_us=${SEND_SLEEP_US}"
"${OUT_BIN}" -i "${IFACE}" -d "${DEST_MAC}" -D "${DEST_IP}" -S "${SRC_IP}" -p 5555 -n "${MAX_RPC}" -f "${SCAN_FREQ}" -w "${ENABLE_WAIT}" -r "${WAIT_RUN_NS}" -l "${WAIT_LONG_MULTIPLIER}" -s "${SEND_SLEEP_US}"
echo "[done] sent RPC UDP frames to port 5555."
