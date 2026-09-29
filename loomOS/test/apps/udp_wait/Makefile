# SPDX-License-Identifier: MPL-2.0

include ../test_common.mk

ifeq ($(ARCH), riscv64)
# On riscv64 inside our image, prefer dynamic linking to avoid missing static libc.
EXTRA_C_FLAGS :=
else
EXTRA_C_FLAGS := -static
endif
