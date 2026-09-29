#include <stdio.h>
#include <unistd.h>
#include <sys/syscall.h>
#include <stdint.h>
#include <sys/types.h>
#include <sys/stat.h>
#include <stdlib.h>
#include <fcntl.h>
#include <sys/ioctl.h>
#include "interrupt.h"
#include <sys/mman.h>

#define __NR_thlet 453
#define __aligned(x) __attribute__((aligned(x)))
#ifndef PAGE_SIZE
#define PAGE_SIZE 4096
#endif

static inline uint64_t static_rdtsc(void) {
    uint64_t val;
    asm volatile ("rdcycle %0" : "=r" (val));
    return val;
}

struct thlet_intr_stats {
	uint64_t exception_entry;
	uint64_t do_trap_entry;
	uint64_t enter_from_user_entry;
	uint64_t enter_from_user_exit;
	uint64_t do_intr_entry;
	uint64_t do_intr_exit;
	uint64_t exit_to_user_entry;
	uint64_t exit_to_user_exit;
} __aligned(64);

struct thlet_intr_stats *kstats;

int main() {
    int helper_fd = open("/dev/thlet_intr", O_RDWR);
    if (helper_fd < 0) {
        perror("Failed to open thlet_switch_helper");
        exit(1);
    }

    kstats = mmap(NULL, PAGE_SIZE, PROT_READ | PROT_WRITE, MAP_SHARED, helper_fd, 0);
    if (!kstats) {
        perror("mmap failed");
        close(helper_fd);
        exit(1);
    }

    uint64_t exception = 0, reg = 0, enter_user = 0, to_do = 0, exit_do = 0, exit_user = 0, exit_to = 0;
    uint64_t cnt = 0;

    for (int i = 0; i < 100; i ++) {
        uint64_t s = static_rdtsc();
        long ret = syscall(__NR_thlet);
        uint64_t e = static_rdtsc();
        ioctl(helper_fd, 0, 0);

        exception += kstats->exception_entry - s;
        reg += kstats->do_trap_entry - kstats->exception_entry;
        enter_user += kstats->enter_from_user_exit - kstats->enter_from_user_entry;
        to_do += kstats->do_intr_entry - kstats->enter_from_user_exit;
        exit_do += kstats->exit_to_user_entry - kstats->do_intr_exit;
        exit_user += kstats->exit_to_user_exit - kstats->exit_to_user_entry;
        exit_to += e - kstats->exit_to_user_exit;

        cnt ++;
    }
    if (cnt) {
        exception /= cnt;
        reg /= cnt;
        enter_user /= cnt;
        exit_user /= cnt;
        to_do /= cnt;
        exit_do /= cnt;
        exit_user /= cnt;
        exit_to /= cnt;
    }

    printf("exception: %llu, reg: %llu, enter_user: %llu, exit_user: %llu, to_do: %llu, exit_do: %llu, exit_user: %llu, exit_to: %llu\n",
        exception, reg, enter_user, exit_user, to_do, exit_do, exit_user, exit_to);

    close(helper_fd);
    return 0;
}