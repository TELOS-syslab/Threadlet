#!/usr/bin/env bash
# sudo docker build . -t asterinas/threadlet:1.0
set -euo pipefail

# If you want to expose more ports from the guest (via QEMU hostfwd),
# adjust these mappings or set env vars before running this script.
HOSTFWD_PORT1=${HOSTFWD_PORT1:-12280}  # mapped to guest :8080 in QEMU
HOSTFWD_PORT2=${HOSTFWD_PORT2:-9299}   # optional for guest→host tests (TCP)
HOSTFWD_PORT_UDP_BASE=${HOSTFWD_PORT_UDP_BASE:-12281} # Base UDP port to expose (range BASE..BASE+9)

# Use current repo as workspace inside container
WORKDIR=$(pwd)

# Run the container with explicit port mappings instead of --network=host,
# so that services in the guest (forwarded by QEMU to 127.0.0.1:$HOSTFWD_PORTx)
# are reachable from outside the container.
# Expose 10 UDP ports (BASE..BASE+9) so host can send to QEMU hostfwd inside container
UDP_PORT_FLAGS=()
for p in $(seq ${HOSTFWD_PORT_UDP_BASE} $((HOSTFWD_PORT_UDP_BASE+9))); do
  UDP_PORT_FLAGS+=( -p ${p}:${p}/udp )
done

sudo docker run -it \
  --privileged \
  --device=/dev/kvm \
  -p ${HOSTFWD_PORT1}:${HOSTFWD_PORT1} \
  -p ${HOSTFWD_PORT2}:${HOSTFWD_PORT2} \
  "${UDP_PORT_FLAGS[@]}" \
  -v "${WORKDIR}:/root/asterinas" \
  asterinas/threadlet:2.0
