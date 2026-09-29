# SPDX-License-Identifier: MPL-2.0

BINARY_CACHE_DIR := /opt/linux_binary_cache
ifeq ($(ARCH), riscv64)
BUSYBOX_PATH := /opt/riscv_busybox/busybox
else
BUSYBOX_PATH := /bin/busybox
endif

VDSO_DIR := ../target
VDSO_LIB := $(VDSO_DIR)/vdso64.so
MKFILE_PATH := $(abspath $(lastword $(MAKEFILE_LIST)))
CUR_DIR := $(patsubst %/,%,$(dir $(MKFILE_PATH)))
ATOMIC_WGET := $(CUR_DIR)/../tools/atomic_wget.sh
BUILD_DIR := $(CUR_DIR)/build
INITRAMFS := $(BUILD_DIR)/initramfs
INITRAMFS_FILELIST := $(BUILD_DIR)/initramfs.filelist
INITRAMFS_IMAGE := $(BUILD_DIR)/initramfs.cpio.gz
EXT2_IMAGE := $(BUILD_DIR)/ext2.img
EXFAT_IMAGE := $(BUILD_DIR)/exfat.img
BUSYBOX_RISCV64 := $(BUILD_DIR)/busybox.riscv64
INITRAMFS_EMPTY_DIRS := \
	$(INITRAMFS)/root \
	$(INITRAMFS)/tmp \
	$(INITRAMFS)/opt \
	$(INITRAMFS)/proc \
	$(INITRAMFS)/dev \
	$(INITRAMFS)/ext2 \
	$(INITRAMFS)/exfat
INITRAMFS_ALL_DIRS := \
	$(INITRAMFS)/etc \
	$(INITRAMFS)/lib \
	$(INITRAMFS)/lib/riscv64-linux-gnu \
	$(INITRAMFS)/lib64 \
	$(INITRAMFS)/bin \
	$(INITRAMFS)/sbin \
	$(INITRAMFS)/usr/bin \
	$(INITRAMFS)/usr/local \
	$(INITRAMFS)/test \
	$(INITRAMFS)/benchmark \
	$(INITRAMFS_EMPTY_DIRS)
SYSCALL_TEST_DIR := $(INITRAMFS)/opt/syscall_test

.PHONY: all
all: build

# Ensure embedded busybox is generated for riscv64 builds.
ifeq ($(ARCH), riscv64)
$(BUSYBOX_RISCV64):
	@mkdir -p $(BUILD_DIR)
	@cp /opt/riscv_busybox/busybox $@
	@chmod +x $@
endif

$(INITRAMFS)/lib: | $(VDSO_LIB)
	@mkdir -p $@
	@if [ -f /lib/ld-linux-riscv64-lp64d.so.1 ]; then cp -L /lib/ld-linux-riscv64-lp64d.so.1 $@; fi

# @cp -L $(VDSO_LIB) $@	
# modified 
$(INITRAMFS)/lib/riscv64-linux-gnu: | $(VDSO_LIB)
	@mkdir -p $@
	@if [ -f /lib/riscv64-linux-gnu/libc.so.6 ]; then cp -L /lib/riscv64-linux-gnu/libc.so.6 $@; fi
	@if [ -f /lib/riscv64-linux-gnu/libstdc++.so.6 ]; then cp -L /lib/riscv64-linux-gnu/libstdc++.so.6 $@; fi
	@if [ -f /lib/riscv64-linux-gnu/libm.so.6 ]; then cp -L /lib/riscv64-linux-gnu/libm.so.6 $@; fi
	@if [ -f /lib/riscv64-linux-gnu/libgcc_s.so.1 ]; then cp -L /lib/riscv64-linux-gnu/libgcc_s.so.1 $@; fi
	@if [ -f /lib/riscv64-linux-gnu/libpthread.so.0 ]; then cp -L /lib/riscv64-linux-gnu/libpthread.so.0 $@; fi
	@if [ -f /lib/riscv64-linux-gnu/libresolv.so.2 ]; then cp -L /lib/riscv64-linux-gnu/libresolv.so.2 $@; fi
	@if [ -f /usr/lib/riscv64-linux-gnu/libatomic.so.1 ]; then cp -L /usr/lib/riscv64-linux-gnu/libatomic.so.1 $@; fi
	@# required for benchmarks
	@if [ -f /lib/riscv64-linux-gnu/libcrypto.so.3 ]; then cp -L /lib/riscv64-linux-gnu/libcrypto.so.3 $@; fi
	@if [ -f /lib/riscv64-linux-gnu/libcrypt.so.1 ]; then cp -L /lib/riscv64-linux-gnu/libcrypt.so.1 $@; fi
	@if [ -f /lib/riscv64-linux-gnu/libdl.so.2 ]; then cp -L /lib/riscv64-linux-gnu/libdl.so.2 $@; fi
	@if [ -f /lib/riscv64-linux-gnu/libz.so.1 ]; then cp -L /lib/riscv64-linux-gnu/libz.so.1 $@; fi
	@if [ -f /lib/riscv64-linux-gnu/libmvec.so.1 ]; then cp -L /lib/riscv64-linux-gnu/libmvec.so.1 $@; fi
	@# required for VDSO
	@cp -L $(VDSO_LIB) $@

$(VDSO_LIB): | $(VDSO_DIR) $(BINARY_CACHE_DIR)/vdso64.so
	@# TODO: use a custom compiled vdso.so file in the future.
	@cp $(BINARY_CACHE_DIR)/vdso64.so $@
	
$(VDSO_DIR):
	@mkdir -p $@
	
$(BINARY_CACHE_DIR)/vdso64.so:
	@mkdir -p $(BINARY_CACHE_DIR)
	@$(ATOMIC_WGET) $@ "https://raw.githubusercontent.com/asterinas/linux_vdso/2a6d2db/vdso64.so"

$(INITRAMFS)/lib64:
	@mkdir -p $@
#@cp -L /lib64/ld-linux-x86-64.so.2 $@

$(INITRAMFS)/etc:
	@mkdir -p $@
	@cp $(CUR_DIR)/etc/* $@

# Install busybox into /bin and /usr/bin.
$(INITRAMFS)/bin:
	@mkdir -p $@
ifeq ($(ARCH), riscv64)
	@# RISC-V: busybox 改为内嵌到内核镜像，由内核在 rootfs 初始化后写入 /usr/bin/busybox。
	@true
else
	@# x86_64：使用 busybox 的 --install 选项
	@$(BUSYBOX_PATH) --install -s $@
endif

$(INITRAMFS)/sbin:
	@mkdir -p $@
	@# Keep /sbin present but do not include ldconfig binaries to slim initramfs.
	@# ldconfig is not required for our minimal userspace (busybox + provided libs).

$(INITRAMFS)/usr/bin: | $(INITRAMFS)/bin
	@mkdir -p $@
ifeq ($(ARCH), riscv64)
	@# RISC-V: busybox 由内核内嵌，跳过从宿主机复制到 initramfs。
	@true
else
	@cp $(BUSYBOX_PATH) $@
endif

$(INITRAMFS)/usr/local:
	@mkdir -p $@

.PHONY: $(INITRAMFS)/test
$(INITRAMFS)/test:
	@make --no-print-directory -C apps

$(INITRAMFS)/benchmark: | $(INITRAMFS)/benchmark/bin
	@# Directory exists for PATH=/bin:/benchmark, but no files are copied.
	@true

$(INITRAMFS)/benchmark/bin:
	@# Create directory only; skip large benchmark binaries
	@mkdir -p $@

# Make necessary directories.
$(INITRAMFS_EMPTY_DIRS):
	@mkdir -p $@

.PHONY: $(SYSCALL_TEST_DIR)
$(SYSCALL_TEST_DIR):
	@make --no-print-directory -C syscall_test

.PHONY: $(INITRAMFS_IMAGE)
$(INITRAMFS_IMAGE): $(INITRAMFS_FILELIST)
	@if ! cmp -s $(INITRAMFS_FILELIST) $(INITRAMFS_FILELIST).previous ; then \
		echo "Generating the initramfs image..."; \
		cp -f $(INITRAMFS_FILELIST) $(INITRAMFS_FILELIST).previous; \
		( \
			cd $(INITRAMFS); \
			# `$(INITRAMFS_FILELIST)` contains files' last modification \
			# time in the first column, and files' relative path to \
			# `$(INITRAMFS)` in the second column. This prunes the first \
			# column and passes the second column to `cpio`. \
			cut -d " " -f 2- $(INITRAMFS_FILELIST) | \
				cpio -o -H newc | gzip \
		) > $@; \
	fi

.PHONY: $(INITRAMFS_FILELIST)
# If the BUILD_SYSCALL_TEST variable is set, we should depend on the
# sub make output to do incremental building.
ifeq ($(BUILD_SYSCALL_TEST), 1)
$(INITRAMFS_FILELIST): | $(INITRAMFS_ALL_DIRS) $(SYSCALL_TEST_DIR)
else
$(INITRAMFS_FILELIST): | $(INITRAMFS_ALL_DIRS)
endif
	@(cd $(INITRAMFS); find . -printf "%T@ %p\n") > $(INITRAMFS_FILELIST)

$(EXT2_IMAGE):
	@dd if=/dev/zero of=$(EXT2_IMAGE) bs=2G count=1
	@mke2fs $(EXT2_IMAGE)

$(EXFAT_IMAGE):
	@fallocate -l 64M $(EXFAT_IMAGE)
	@mkfs.exfat $(EXFAT_IMAGE)

.PHONY: build
ifeq ($(ARCH), riscv64)
build: $(BUSYBOX_RISCV64) $(INITRAMFS_IMAGE) $(EXT2_IMAGE) $(EXFAT_IMAGE)
else
build: $(INITRAMFS_IMAGE) $(EXT2_IMAGE) $(EXFAT_IMAGE)
endif

.PHONY: format
format:
	@make --no-print-directory -C apps format

.PHONY: check
check:
	@make --no-print-directory -C apps check

.PHONY: clean
clean:
	@rm -rf $(BUILD_DIR)
