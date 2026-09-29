# Run the following command on your computer:
# ssh -N \
#   -o ServerAliveInterval=30 \
#   -o ServerAliveCountMax=3 \
#   -R 127.0.0.1:1080 \
#   qxh@10.129.164.115

# update from asterinas/threadlet:1.0 to asterinas/threadlet:2.0


set -euo pipefail

WORKDIR=$(pwd)

sudo docker run -it \
  --network=host \
  --privileged \
  --device=/dev/kvm \
  -e ALL_PROXY="socks5h://127.0.0.1:1080" \
  -e all_proxy="socks5h://127.0.0.1:1080" \
  -e HTTP_PROXY="socks5h://127.0.0.1:1080" \
  -e http_proxy="socks5h://127.0.0.1:1080" \
  -e HTTPS_PROXY="socks5h://127.0.0.1:1080" \
  -e https_proxy="socks5h://127.0.0.1:1080" \
  -e NO_PROXY="localhost,127.0.0.1,::1,10.0.0.0/8,172.16.0.0/12,192.168.0.0/16" \
  -e no_proxy="localhost,127.0.0.1,::1,10.0.0.0/8,172.16.0.0/12,192.168.0.0/16" \
  -v "${WORKDIR}:/root/asterinas" \
  asterinas/threadlet:2.0