#include <linux/sched.h>

#define DEV_MAJOR            451
#define DEV_MINOR            0
#define DEV_NAME             "thlet_intr"
#define thlet_intr_ITER      10
#define thlet_intr_THREADS   1024
#define THLET_RUNNING_CORE   19
#define THLET_STATS_CORE     ((THLET_RUNNING_CORE) > 16 ? THLET_RUNNING_CORE - 16 : 16 - THLET_RUNNING_CORE)
#define THLET_SLEEP_NS       10000
#define THLET_LOOPS          1000

#define THLET_IOCTL_START _IO(DEV_MAJOR, 0)
#define THLET_IOCTL_FINISH _IO(DEV_MAJOR, 1)
#define THLET_IOCTL_CLEAR _IO(DEV_MAJOR, 2)
#define THLET_IOCTL_PRINT _IO(DEV_MAJOR, 3)