#include <linux/module.h>
#include <linux/kernel.h>
#include <linux/kthread.h>
#include <linux/sched.h>
#include <linux/percpu.h>
#include <linux/delay.h>

#define THREADS 64
#define THREAD_LOOP 1400
#define EXPECTED_TASK_CYCLES 10000

typedef struct {
  uint64_t start;
  uint64_t end;
  uint64_t pick;
  uint64_t pick_update;
  uint64_t sched_update;
  uint64_t sched;
  uint64_t before_rcu;
  uint64_t after_rcu;
  uint64_t cs_mm;
  uint64_t cs_reg;
  uint64_t cs;
  uint64_t loop;
  uint64_t cnt;
} __aligned(64) SwitchStats;

static inline uint64_t static_rdtsc(void) {
  uint64_t val;
  asm volatile ("rdcycle %0" : "=r" (val));
  return val;
}

struct task_struct *mythread[THREADS];
static volatile bool all_finished = false, all_started = false;
SwitchStats stats_record[THREADS];

static int mythread_loop(void *arg) {
  uint64_t sched = 0, cs = 0, cs_mm = 0, cs_reg = 0, pick = 0, pick_update = 0, sched_update = 0;
  uint64_t before_rcu = 0, after_rcu = 0;
  uint64_t loop = 0, s, e;
  uint64_t cnt = 0, id = (uint64_t) arg;
  struct thlet_switch_stats* stats = &per_cpu(cpu_thlet_switch_stats, smp_processor_id());

  // while (!READ_ONCE(all_started));

  local_irq_disable();
  uint64_t start = static_rdtsc();

  while (cnt < 100) {
    s = static_rdtsc();
    uint64_t i = 0;
    do {
      asm ("nop");
    } while (i ++ < THREAD_LOOP);
    e = static_rdtsc();

    // usleep_range(5 - (e % 5), 5 + (e % 5));
    this_cpu_write(cpu_thlet_switch_start, true);
    // usleep_range(0, 1);
    yield();
    schedule();
    this_cpu_write(cpu_thlet_switch_start, false);
    sched += stats->sched_exit - stats->sched_entry;
    before_rcu += stats->rcu_entry - stats->sched_entry;
    after_rcu += stats->pick_entry - stats->rcu_entry;
    sched_update += stats->pick_entry - stats->sched_entry;
    if (stats->eevdf_entry > stats->sched_entry && stats->eevdf_entry < stats->sched_exit) {
      pick += stats->eevdf_exit - stats->eevdf_entry;
      pick_update += stats->pick_exit - stats->pick_entry - (stats->eevdf_exit - stats->eevdf_entry);
    }

    if (stats->cs_entry < stats->sched_exit && stats->cs_entry > stats->sched_entry) {
      cs += stats->cs_exit - stats->cs_entry;
      cs_reg += stats->cs_exit - stats->cs_reg;
      cs_mm += stats->cs_reg - stats->cs_mm;
    }
    loop += e - s;

    cnt ++;
  }

  uint64_t end = static_rdtsc();
  local_irq_enable();

  stats_record[id] = (SwitchStats) {
    .start = start,
    .end = end,
    .sched = sched,
    .cs = cs,
    .loop = loop,
    .cnt = cnt,
    .cs_mm = cs_mm,
    .cs_reg = cs_reg,
    .pick = pick,
    .pick_update = pick_update,
    .sched_update = sched_update,
    .before_rcu = before_rcu,
    .after_rcu = after_rcu,
  };

  return 0;
}

static int mythread_switch_init(void) {
  // sched_set_fifo(current);
  for (int i = 0; i < THREADS; i ++) {
    mythread[i] = kthread_create(mythread_loop, (void *)i, "mythread-loop");
    kthread_bind(mythread[i], 3);
    wake_up_process(mythread[i]);
  }

  // udelay(100);
  all_started = true;
  smp_wmb();

  return 0;
}

static void mythread_switch_exit(void) {
  SwitchStats s = (SwitchStats) {
    .start = -1ull,
    .end = 0,
    .cnt = 0,
    .sched = 0,
    .cs = 0,
    .loop = 0,
    .pick_update = 0,
    .sched_update = 0,
    .pick = 0,
    .cs_mm = 0,
    .cs_reg = 0,
    .before_rcu = 0,
    .after_rcu = 0,
  };

  uint64_t loop_cycles = 0, total = 0, throughput = 0;

  for (int i = 0; i < THREADS; i ++) {
    if (!stats_record[i].cnt) continue;
    s.cnt += stats_record[i].cnt;
    s.start = min(s.start, stats_record[i].start);
    s.end = max(s.end, stats_record[i].end);
    s.loop += stats_record[i].loop;
    s.sched += stats_record[i].sched;
    s.cs += stats_record[i].cs;
    s.cs_mm += stats_record[i].cs_mm;
    s.cs_reg += stats_record[i].cs_reg;
    s.pick += stats_record[i].pick;
    s.pick_update += stats_record[i].pick_update;
    s.sched_update += stats_record[i].sched_update;
    s.before_rcu += stats_record[i].before_rcu;
    s.after_rcu += stats_record[i].after_rcu;

    // printk("mythread-%d start %llu, end %llu\n", i, stats_record[i].start, stats_record[i].end);
  }
  
  loop_cycles = s.cnt * EXPECTED_TASK_CYCLES;
  total = loop_cycles + s.sched;

  // printk("mythread %d record: %llu\n", THREADS, s.cnt);
  // Use the expected 10 us work budget plus measured scheduling cycles.
  throughput = s.cnt * 1000000 / total;
  printk("figure8: completed_tasks=%llu total_cycles=%llu scheduler_cycles=%llu throughput_ktps=%llu\n",
      s.cnt, total, s.sched, throughput);
  printk("sched : %llu%%\n", (s.sched - s.cs) * 100 / total);
  printk("cs : %llu%%\n", s.cs * 100 / total);
  printk("throughput: %llu\n", throughput);
  printk("break down sched : %llu\n", s.sched / s.cnt);
  printk("break down cs : %llu\n", s.cs / s.cnt);
}

module_init(mythread_switch_init);
module_exit(mythread_switch_exit);
MODULE_LICENSE("GPL");
