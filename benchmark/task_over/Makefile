ifneq ($(KERNELRELEASE),)

obj-m += switch.o

else

# The default assumes you cloned this as part of firesim-software (FireMarshal)
LINUXSRC=~/Threadlet-AE/chipyard/software/firemarshal/boards/default/linux

KMAKE=make LLVM=-15 -C $(LINUXSRC) KBUILD_MODPOST_WARN=1 ARCH=riscv CROSS_COMPILE=riscv64-unknown-linux-gnu- M=$(PWD)

switch.ko: switch.c
	$(KMAKE)

clean:
	$(KMAKE) clean

endif
