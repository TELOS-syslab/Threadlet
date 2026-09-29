cd qemu
./configure --target-list=riscv64-softmmu --prefix=/usr/local/qemu --enable-slirp
make -j
make install