# SPDX-License-Identifier: MPL-2.0

#= Install packages for Docker building ====================================

# sudo docker build . -t asterinas/asterinas:riscv

FROM asterinas/asterinas:0.15.1-20250603



WORKDIR /opt/linux_binary_cache
RUN wget https://raw.githubusercontent.com/asterinas/linux_vdso/2a6d2db/vdso64.so -O vdso64.so
RUN wget https://raw.githubusercontent.com/asterinas/linux_binary_cache/14598b6/vmlinuz-5.15.0-105 \
        -O vmlinuz

WORKDIR /root

RUN apt update && apt-get install -y --no-install-recommends \ 
    libgcrypt-dev   `# optional build dependency` \ 
    libglib2.0-dev  `# build dependency` \ 
    libpixman-1-dev `# build dependency` \ 
    libusb-dev      `# optional build dependency` \ 
    meson \ 
    ninja-build

RUN wget -O qemu.tar.xz https://download.qemu.org/qemu-9.1.0.tar.xz \ 
    && mkdir /root/qemu \ 
    && tar xf qemu.tar.xz --strip-components=1 -C /root/qemu \ 
    && rm qemu.tar.xz
WORKDIR /root/qemu
RUN ./configure --target-list=riscv64-softmmu --prefix=/usr/local/qemu --enable-slirp \ 
    && make -j \ 
    && make install
WORKDIR /root
RUN rm -r --force /root/qemu



# WORKDIR /root
# RUN wget -O busybox.tar.bz2 https://busybox.net/downloads/busybox-1.35.0.tar.bz2 \ 
#     && mkdir /root/busybox \ 
#     && tar xf busybox.tar.bz2 --strip-components=1 -C /root/busybox \ 
#     && rm busybox.tar.bz2
# WORKDIR /root/busybox
# RUN make defconfig \ 
#     && sed -i "s/# CONFIG_STATIC is not set/CONFIG_STATIC=y/g" .config \ 
#     && sed -i "s/# CONFIG_FEATURE_SH_STANDALONE is not set/CONFIG_FEATURE_SH_STANDALONE=y/g" .config \ 
#     && make -j

# RUN cp -r /root/busybox/busybox /bin/busybox
# 安装 RISC-V 交叉编译工具链
RUN apt-get update && apt-get install -y --no-install-recommends \
    gcc-riscv64-linux-gnu \
    binutils-riscv64-linux-gnu \
    libc6-dev-riscv64-cross

# 编译 RISC-V 版本的 busybox
WORKDIR /root
RUN wget -O busybox-riscv.tar.bz2 https://busybox.net/downloads/busybox-1.35.0.tar.bz2 \
    && mkdir /root/busybox-riscv \
    && tar xf busybox-riscv.tar.bz2 --strip-components=1 -C /root/busybox-riscv \
    && rm busybox-riscv.tar.bz2

WORKDIR /root/busybox-riscv



RUN make clean && make defconfig

RUN sed -i "s/# CONFIG_STATIC is not set/CONFIG_STATIC=y/g" .config \
    && sed -i "s/# CONFIG_FEATURE_SH_STANDALONE is not set/CONFIG_FEATURE_SH_STANDALONE=y/g" .config \
    && sed -i "s/# CONFIG_CROSS_COMPILER_PREFIX is not set/CONFIG_CROSS_COMPILER_PREFIX=\"riscv64-linux-gnu-\"/g" .config \
    && grep "CONFIG_CROSS_COMPILER_PREFIX" .config  # 验证配置已正确设置


RUN CROSS_COMPILE=riscv64-linux-gnu- make -j


RUN file busybox | grep "RISC-V"

RUN apt-get update && \
    apt-get install -y netcat && \
    rm -rf /var/lib/apt/lists/*

RUN apt-get update && \
    apt-get install -y --no-install-recommends device-tree-compiler && \
    rm -rf /var/lib/apt/lists/*


RUN mkdir -p /opt/riscv_busybox \
    && cp -r busybox /opt/riscv_busybox/busybox \
    && file /opt/riscv_busybox/busybox  # 再次验证

WORKDIR /root