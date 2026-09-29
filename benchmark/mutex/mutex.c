#include <linux/module.h>
#include <linux/kernel.h>
#include <linux/kthread.h>
#include <linux/mutex.h>
#include <linux/cpumask.h>
#include <linux/delay.h>
#include <linux/sched.h>
#include <linux/completion.h>

#define THREADS 16
#define THREAD_LOOP 10000
#define THREAD_SPIN 10000

static struct task_struct *mythread[THREADS];
static uint64_t lock_tsc[THREADS][THREAD_LOOP];
static uint64_t unlock_tsc[THREADS][THREAD_LOOP];
static volatile bool all_started = false;
static struct mutex test_mutex;
static volatile uint64_t cnt, unlock, handover;
static volatile uint64_t start_tsc, end_tsc, sum_time;

static inline uint64_t mythread_rdtsc(void) {
  uint64_t val;
  asm volatile ("rdcycle %0" : "=r" (val));
  return val;
}

static inline uint64_t mythread_tsc(void) {
  uint64_t val;
  asm volatile ("rdtime %0" : "=r" (val));
  return val;
}

static int test_thread(void *arg) {
  int id = (int) arg;
  int i, j;
  uint64_t lock, total_time = 0;
  // while (!READ_ONCE(all_started));
  // local_irq_disable();
  for (i = 0; i < 100; i ++) {
    j = 0;
    mutex_lock(&test_mutex);
    lock = mythread_tsc();
    // printk("unlock %d-%d\n", unlock, lock);
    if (lock - unlock < 50) {
      handover += lock - unlock;
      cnt ++;
    }
    uint64_t now = mythread_rdtsc();
    if (start_tsc == 0) {
      start_tsc = mythread_tsc();
    }
    do {
      asm ("nop");
    } while (mythread_rdtsc() - now < THREAD_SPIN);

    unlock = mythread_tsc();
    end_tsc = unlock;
    sum_time += mythread_rdtsc() - now;
    smp_wmb();
    mutex_unlock(&test_mutex);
  }
  // local_irq_enable();
  return 0;
}

static int __init mutex_bench_init(void) {
  mutex_init(&test_mutex);

  handover = 0;
  unlock = 0;
  cnt = 0;
  start_tsc = 0;
  end_tsc = 0;
  smp_wmb();
  for (int i = 0; i < THREADS; i ++) {
    mythread[i] = kthread_create(test_thread, (void *) i, "mutex_bench_b");
    if (mythread[i] == ERR_PTR(-ENOMEM)) {
      printk("Failed to create thread %d\n", i);
      return -ENOMEM;
    }
    kthread_bind(mythread[i], i % 4);
    wake_up_process(mythread[i]);
  }

  udelay(10);
  // all_started = 1;
  smp_wmb();
  return 0;
}

static void __exit mutex_bench_exit(void) {
  
    // TODO: analyze the lock_tsc and unlock_tsc to calculate the average handover time.
  printk("Total lock handover count: %llu, average handover time: %llu cycles, throughput: %llu\n", 
    cnt, cnt ? handover * 1000 / cnt : 0, cnt * 1000000ull / (end_tsc - start_tsc));
  handover = 0;
  unlock = 0;
  cnt = 0;
  smp_wmb();
  
}

module_init(mutex_bench_init);
module_exit(mutex_bench_exit);
MODULE_LICENSE("GPL");