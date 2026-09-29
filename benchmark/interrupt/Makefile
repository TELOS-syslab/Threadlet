ifneq ($(KERNELRELEASE),)

obj-m += sys_interrupt_helper.o

else

# The default assumes you cloned this as part of firesim-software (FireMarshal)
LINUXSRC=/home/qxh/CHIPYARD_TEST/chipyard/software/firemarshal/boards/default/linux

KMAKE=make LLVM=-15 -C $(LINUXSRC) KBUILD_MODPOST_WARN=1 ARCH=riscv CROSS_COMPILE=riscv64-unknown-linux-gnu- M=$(PWD)

sys_interrupt_helper.ko: sys_interrupt_helper.c
	$(KMAKE)

clean:
	$(KMAKE) clean

endif
