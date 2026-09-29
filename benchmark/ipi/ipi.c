#include <linux/module.h>
#include <linux/kernel.h>
#include <linux/kthread.h>
#include <linux/mutex.h>
#include <linux/cpumask.h>
#include <linux/delay.h>
#include <linux/sched.h>
#include <linux/completion.h>
#include <linux/thlet_rpc.h>
#include <linux/thlet_hash.h>
#include <linux/thlet_lock_stats.h>

#define THLET_LOOP_TIME 5000
#define THLET_ITER 1000

static struct mutex test_mutex;
static struct task_struct *task_a, *task_b, *task_c;
static int a = 0, b = 0;
static uint64_t time_b, time_c;
static volatile uint64_t t = 0;
volatile bool all_started;

#define between_lock(stat, num) ((num) < (stat).mutex_lock_exit_ts && (num) > (stat).mutex_lock_entry_ts)
#define between_unlock(stat, num) ((num) < (stat).mutex_unlock_exit_ts && (num) > (stat).mutex_unlock_entry_ts)
ThletLockStats gs[4];
uint64_t send_tsc[4][THLET_ITER];
uint64_t recv_tsc[4][THLET_ITER];

static inline uint64_t thlet_rdtsc(void) {
  uint64_t val;
  asm volatile ("rdcycle %0" : "=r" (val));
  return val;
}

void thlet_lock_stats_report(void) {
    int cpu;
    ThletLockStats *s;

    for_each_online_cpu(cpu) {
        s = &per_cpu(g_lock_stats, cpu);
        
        printk("---------- Thlet Lock Stats [CPU %d] ----------\n", cpu);

        /* 1. Mutex Unlock / Wakeup Path (唤醒端) */
        printk("[Unlock Path]\n");
        printk("  generic_unlock_ts:     %llu\n", (unsigned long long)s->unlock_ts);
        printk("  generic_unlock_tsc:     %llu\n", (unsigned long long)s->unlock_tsc);
        printk("  mutex_unlock_entry:    %llu\n", (unsigned long long)s->mutex_unlock_entry_ts);
        printk("  mutex_wkq_entry:       %llu\n", (unsigned long long)s->mutex_wkq_entry_ts);
        printk("  mutex_wkp_entry:       %llu\n", (unsigned long long)s->mutex_wkp_entry_ts);
        printk("  mutex_sendipi_entry:   %llu\n", (unsigned long long)s->mutex_sendipi_entry_ts);
        printk("  mutex_sendipi_entry_tsc:   %llu\n", (unsigned long long)s->mutex_sendipi_entry_tsc);
        printk("  mutex_sendipi_exit:    %llu\n", (unsigned long long)s->mutex_sendipi_exit_ts);
        printk("  mutex_wkp_exit:        %llu\n", (unsigned long long)s->mutex_wkp_exit_ts);
        printk("  mutex_wkq_exit:        %llu\n", (unsigned long long)s->mutex_wkq_exit_ts);
        printk("  mutex_unlock_exit:     %llu\n", (unsigned long long)s->mutex_unlock_exit_ts);
        
        printk("  wake_up_process_entry: %llu\n", (unsigned long long)s->wake_up_process_entry_ts);
        printk("  send_ipi_cycle:        %llu\n", (unsigned long long)s->send_ipi_cycle);
        printk("  send_ipi_tsc:          %llu\n", (unsigned long long)s->send_ipi_tsc);
        printk("  wake_up_process_exit:  %llu\n", (unsigned long long)s->wake_up_process_exit_ts);

        /* 2. IPI Handling & Scheduling (被唤醒端核心路径) */
        printk("[IPI & Sched Path]\n");
        printk("  handle_ipi_entry_cycle:%llu\n", (unsigned long long)s->handle_ipi_entry_cycle);
        printk("  handle_ipi_entry_tsc:  %llu\n", (unsigned long long)s->handle_ipi_entry_tsc);
        printk("  handle_ipi_exit:       %llu\n", (unsigned long long)s->handle_ipi_exit_ts);
        printk("  schedule_entry:        %llu\n", (unsigned long long)s->schedule_entry_ts);
        printk("  schedule_exit:         %llu\n", (unsigned long long)s->schedule_exit_ts);

        /* 3. Mutex Lock Path (等待/获取端) */
        printk("[Lock Path]\n");
        printk("  mutex_lock_entry:      %llu\n", (unsigned long long)s->mutex_lock_entry_ts);
        printk("  mutex_lock_loop_entry: %llu\n", (unsigned long long)s->mutex_lock_loop_entry_ts);
        printk("  mutex_lock_op_loop_entry: %llu\n", (unsigned long long)s->mutex_loop_op_loop_entry_ts);
        printk("  mutex_lock_op_loop_exit: %llu\n", (unsigned long long)s->mutex_loop_op_loop_exit_ts);
        printk("  mutex_lock_sched_entry:%llu\n", (unsigned long long)s->mutex_lock_sched_entry_ts);
        printk("  mutex_lock_sched_exit: %llu\n", (unsigned long long)s->mutex_lock_sched_exit_ts);
        printk("  mutex_lock_loop_exit:  %llu\n", (unsigned long long)s->mutex_lock_loop_exit_ts);
        printk("  mutex_lock_exit:       %llu\n", (unsigned long long)s->mutex_lock_exit_ts);
        printk("  acquire_lock_ts:       %llu\n", (unsigned long long)s->acquire_lock_ts);
        printk("  acquire_lock_tsc:       %llu\n", (unsigned long long)s->acquire_lock_tsc);
        
        printk("-----------------------------------------------\n\n");
    }
}

void thlet_lock_stats_report_data(ThletLockStats *s) {
        /* 1. Mutex Unlock / Wakeup Path (唤醒端) */
        printk("[Unlock Path]\n");
        printk("  generic_unlock_ts:     %llu\n", (unsigned long long)s->unlock_ts);
        printk("  generic_unlock_tsc:     %llu\n", (unsigned long long)s->unlock_tsc);
        printk("  mutex_unlock_entry:    %llu\n", (unsigned long long)s->mutex_unlock_entry_ts);
        printk("  mutex_wkq_entry:       %llu\n", (unsigned long long)s->mutex_wkq_entry_ts);
        printk("  mutex_wkp_entry:       %llu\n", (unsigned long long)s->mutex_wkp_entry_ts);
        printk("  mutex_sendipi_entry:   %llu\n", (unsigned long long)s->mutex_sendipi_entry_ts);
        printk("  mutex_sendipi_entry_tsc:   %llu\n", (unsigned long long)s->mutex_sendipi_entry_tsc);
        printk("  mutex_sendipi_exit:    %llu\n", (unsigned long long)s->mutex_sendipi_exit_ts);
        printk("  mutex_wkp_exit:        %llu\n", (unsigned long long)s->mutex_wkp_exit_ts);
        printk("  mutex_wkq_exit:        %llu\n", (unsigned long long)s->mutex_wkq_exit_ts);
        printk("  mutex_unlock_exit:     %llu\n", (unsigned long long)s->mutex_unlock_exit_ts);
        
        printk("  wake_up_process_entry: %llu\n", (unsigned long long)s->wake_up_process_entry_ts);
        printk("  send_ipi_cycle:        %llu\n", (unsigned long long)s->send_ipi_cycle);
        printk("  send_ipi_tsc:          %llu\n", (unsigned long long)s->send_ipi_tsc);
        printk("  wake_up_process_exit:  %llu\n", (unsigned long long)s->wake_up_process_exit_ts);

        /* 2. IPI Handling & Scheduling (被唤醒端核心路径) */
        printk("[IPI & Sched Path]\n");
        printk("  handle_ipi_entry_cycle:%llu\n", (unsigned long long)s->handle_ipi_entry_cycle);
        printk("  handle_ipi_entry_tsc:  %llu\n", (unsigned long long)s->handle_ipi_entry_tsc);
        printk("  handle_ipi_exit:       %llu\n", (unsigned long long)s->handle_ipi_exit_ts);
        printk("  schedule_entry:        %llu\n", (unsigned long long)s->schedule_entry_ts);
        printk("  schedule_exit:         %llu\n", (unsigned long long)s->schedule_exit_ts);

        /* 3. Mutex Lock Path (等待/获取端) */
        printk("[Lock Path]\n");
        printk("  mutex_lock_entry:      %llu\n", (unsigned long long)s->mutex_lock_entry_ts);
        printk("  mutex_lock_slow_entry: %llu\n", (unsigned long long)s->mutex_lock_slow_entry_ts);
        printk("  mutex_lock_pre_entry: %llu\n", (unsigned long long)s->mutex_lock_pre_entry_ts);
        printk("  mutex_lock_loop_entry: %llu\n", (unsigned long long)s->mutex_lock_loop_entry_ts);
        printk("  mutex_lock_op_loop_entry: %llu\n", (unsigned long long)s->mutex_loop_op_loop_entry_ts);
        printk("  mutex_lock_op_loop_exit: %llu\n", (unsigned long long)s->mutex_loop_op_loop_exit_ts);
        printk("  mutex_lock_sched_entry:%llu\n", (unsigned long long)s->mutex_lock_sched_entry_ts);
        printk("  mutex_lock_sched_exit: %llu\n", (unsigned long long)s->mutex_lock_sched_exit_ts);
        printk("  mutex_lock_loop_exit:  %llu\n", (unsigned long long)s->mutex_lock_loop_exit_ts);
        printk("  mutex_lock_slow_exit: %llu\n", (unsigned long long)s->mutex_lock_slow_exit_ts);
        printk("  mutex_lock_pre_exit: %llu\n", (unsigned long long)s->mutex_lock_pre_exit_ts);
        printk("  mutex_lock_exit:       %llu\n", (unsigned long long)s->mutex_lock_exit_ts);
        printk("  acquire_lock_ts:       %llu\n", (unsigned long long)s->acquire_lock_ts);
        printk("  acquire_lock_tsc:       %llu\n", (unsigned long long)s->acquire_lock_tsc);
        
        printk("-----------------------------------------------\n\n");
}

static int thread_c_fn(void *data) {
  uint64_t i = 0;
  a = 1;
  smp_wmb();
  uint64_t now = thlet_rdtsc();
  time_c = now;
  thlet_lock_stats_record_enable();
  mutex_lock(&test_mutex);

  if (!t) t = 2;
  thlet_lock_stats_record_acquire_lock();
  thlet_lock_stats_record_acquire_lock_tsc();
  // printk("----C---\n");

  do {
    asm("nop");
  } while (thlet_rdtsc() - now <= THLET_LOOP_TIME);
  
  // thlet_lock_stats_report();
  mutex_unlock(&test_mutex);
  thlet_lock_stats_record_disable();
  gs[smp_processor_id()] = per_cpu(g_lock_stats, smp_processor_id());

  // thlet_lock_stats_report();
  return 0;
}

static int thread_b_fn(void *data) {
  uint64_t i = 0;
  a = 1;
  smp_wmb();
  uint64_t now = thlet_rdtsc();
  time_b = now;
  thlet_lock_stats_record_enable();
  mutex_lock(&test_mutex);

  do {
    asm("nop");
  } while (thlet_rdtsc() - now <= THLET_LOOP_TIME);

  if (!t) t = 1;
  gs[smp_processor_id()] = per_cpu(g_lock_stats, smp_processor_id());
  thlet_lock_stats_record_acquire_lock();
  thlet_lock_stats_record_acquire_lock_tsc();
  // printk("----B---\n");
  // thlet_lock_stats_report();
  
  mutex_unlock(&test_mutex);
  thlet_lock_stats_record_disable();
  return 0;
}

static int thread_a_fn(void *data) {
  thlet_lock_stats_record_enable();
  mutex_lock(&test_mutex);
  
  // udelay(1);
  while (!a);
  thlet_lock_stats_record_unlock_tsc();
  thlet_lock_stats_record_unlock();
  smp_wmb();
  mutex_unlock(&test_mutex);
  thlet_lock_stats_record_disable();

  return 0;
}

static int thread_idle(void *data) {
  while (!kthread_should_stop());

  return 0;
}

static int test_thread(void *data) {
    int64_t runs = 0;
    int cpu = smp_processor_id();
    ThletLockStats stats;
    
    // 将大数组改为指针
    uint64_t *pre_lock, *bot_lock, *unlock, *pre_unlock, *bot_unlock, *sched, *ipi, *op_loop, *ipi2sched, *ipi2lock, *un2ipi;
    
    // 计数器和汇总变量
    uint64_t pre_lock_num = 0, bot_lock_num = 0, unlock_num = 0, pre_unlock_num = 0, 
      bot_unlock_num = 0, sched_num = 0, ipi_num = 0, op_loop_num = 0, i2s_num = 0, i2l_num = 0, u2i_num = 0;
    uint64_t plock = 0, block = 0, ulock = 0, sd = 0, ip = 0, pulock = 0, bulock = 0, opl = 0, i2s = 0, i2l = 0, u2i = 0;

    // 1. 在堆上分配内存 (THLET_ITER * sizeof(uint64_t))
    size_t alloc_size = THLET_ITER * sizeof(uint64_t);
    pre_lock = vzalloc(alloc_size);
    bot_lock = vzalloc(alloc_size);
    unlock = vzalloc(alloc_size);
    pre_unlock = vzalloc(alloc_size);
    bot_unlock = vzalloc(alloc_size);
    sched = vzalloc(alloc_size);
    ipi = vzalloc(alloc_size);
    op_loop = vzalloc(alloc_size);
    ipi2sched = vzalloc(alloc_size);
    ipi2lock = vzalloc(alloc_size);
    un2ipi = vzalloc(alloc_size);

    // 检查分配是否成功
    if (!pre_lock || !bot_lock || !unlock || !pre_unlock || !bot_unlock || !sched || !ipi || 
        !op_loop || !ipi2sched || !ipi2lock || !un2ipi) {
        printk(KERN_ERR "tester-%d: vzalloc failed\n", cpu);
        goto out_free;
    }

    while (!READ_ONCE(all_started)) ;

    // 修改判断条件，增加溢出保护
    while (runs < THLET_ITER) {
        thlet_lock_stats_record_enable();
        thlet_lock_stats_record_acquire_lock();
        
        mutex_lock(&test_mutex);
        
        thlet_lock_stats_record_acquire_lock_tsc();

        uint64_t now = thlet_rdtsc();
        do {
          asm ("nop");
        } while (thlet_rdtsc() - now < THLET_LOOP_TIME);
        // usleep_range(10, 20);
        thlet_lock_stats_record_unlock_tsc();
        thlet_lock_stats_record_unlock();
        
        mutex_unlock(&test_mutex);

        stats = per_cpu(g_lock_stats, cpu);
        thlet_lock_stats_record_disable();

        // 统计逻辑 (增加对计数器的边界检查)
        if (pre_lock_num < THLET_ITER && between_lock(stats, stats.mutex_lock_loop_entry_ts)) {
            pre_lock[pre_lock_num++] = stats.mutex_lock_loop_entry_ts - stats.mutex_lock_entry_ts;
            bot_lock[bot_lock_num++] = stats.mutex_lock_exit_ts - stats.mutex_lock_loop_exit_ts;
        }
        if (sched_num < THLET_ITER && between_lock(stats, stats.schedule_entry_ts)) {
            sched[sched_num++] = stats.schedule_exit_ts - stats.schedule_entry_ts;
        }
        if (ipi_num < THLET_ITER && between_lock(stats, stats.handle_ipi_entry_cycle)) {
            ipi[ipi_num] = stats.handle_ipi_exit_ts - stats.handle_ipi_entry_cycle;
            recv_tsc[cpu][ipi_num++] = stats.handle_ipi_entry_tsc;
            ipi2lock[i2l_num ++] = stats.mutex_lock_exit_ts - stats.handle_ipi_exit_ts;
            if (i2s_num < THLET_ITER)
                ipi2sched[i2s_num++] = stats.schedule_entry_ts - stats.handle_ipi_exit_ts;
        }
        if (op_loop_num < THLET_ITER && between_lock(stats, stats.mutex_loop_op_loop_entry_ts)) {
            op_loop[op_loop_num++] = stats.mutex_loop_op_loop_exit_ts - stats.mutex_loop_op_loop_entry_ts;
        }
        if (pre_unlock_num < THLET_ITER && between_unlock(stats, stats.mutex_wkq_entry_ts)) {
            pre_unlock[pre_unlock_num++] = stats.mutex_wkq_entry_ts - stats.mutex_unlock_entry_ts;
            bot_unlock[bot_unlock_num++] = stats.mutex_unlock_exit_ts - stats.mutex_wkq_entry_ts;
        }
        if (unlock_num < THLET_ITER) {
            unlock[unlock_num++] = stats.mutex_unlock_exit_ts - stats.mutex_unlock_entry_ts;
        }
        if (u2i_num < THLET_ITER && between_unlock(stats, stats.mutex_sendipi_entry_ts)) {
          un2ipi[u2i_num] = stats.mutex_sendipi_entry_ts - stats.mutex_unlock_entry_ts;
          send_tsc[cpu][u2i_num] = stats.mutex_sendipi_entry_tsc;
          u2i_num ++;
        }

        runs++;
    }

    // 2. 计算平均值 (逻辑不变)
    for (int i = 0; i < pre_lock_num; i++) plock += pre_lock[i];
    if (pre_lock_num) plock /= pre_lock_num;

    for (int i = 0; i < bot_lock_num; i++) block += bot_lock[i];
    if (bot_lock_num) block /= bot_lock_num;

    for (int i = 0; i < unlock_num; i++) ulock += unlock[i];
    if (unlock_num) ulock /= unlock_num;

    for (int i = 0; i < sched_num; i++) sd += sched[i];
    if (sched_num) sd /= sched_num;

    for (int i = 0; i < ipi_num; i++) ip += ipi[i];
    if (ipi_num) ip /= ipi_num;

    for (int i = 0; i < pre_unlock_num; i++) pulock += pre_unlock[i];
    if (pre_unlock_num) pulock /= pre_unlock_num; // 修复了你代码里的 pulock 被赋值给 plock 的笔误

    for (int i = 0; i < bot_unlock_num; i++) bulock += bot_unlock[i];
    if (bot_unlock_num) bulock /= bot_unlock_num;

    for (int i = 0; i < op_loop_num; i++) opl += op_loop[i];
    if (op_loop_num) opl /= op_loop_num;

    for (int i = 0; i < i2s_num; i++) i2s += ipi2sched[i];
    if (i2s_num) i2s /= i2s_num;

    for (int i = 0; i < i2l_num; i++) i2l += ipi2lock[i];
    if (i2l_num) i2l /= i2l_num;

    for (int i = 0; i < u2i_num; i++) u2i += un2ipi[i];
    if (u2i_num) u2i /= u2i_num;

    // 3. 打印结果
    printk("tester-%d p %llu, opl: %llu, b %llu, u %llu, sd %llu, ipi %llu, i2s %llu, i2l %llu, u2i %llu, pulock %llu, bulock %llu\n", 
           cpu, plock, opl, block, ulock, sd, ip, i2s, i2l, u2i, pulock, bulock);

    gs[cpu] = stats;

out_free:
    // 4. 释放内存
    vfree(pre_lock); vfree(bot_lock); vfree(unlock);
    vfree(pre_unlock); vfree(bot_unlock); vfree(sched);
    vfree(ipi); vfree(op_loop); vfree(ipi2sched); vfree(ipi2lock);
    
    return 0;
}

static int __init mutex_bench_init(void) {
  mutex_init(&test_mutex);
  a = 0;
  b = 0;
  task_a = kthread_create(test_thread, (void *) 0, "mutex_bench_a");
  kthread_bind(task_a, 0);
  
  task_b = kthread_create(test_thread, (void *) 1, "mutex_bench_b");
  kthread_bind(task_b, 1);

  task_c = kthread_create(test_thread, (void *) 2, "mutex_bench_b");
  kthread_bind(task_c, 2);

  wake_up_process(task_a);
  wake_up_process(task_b);
  wake_up_process(task_c);

  udelay(10);
  all_started = 1;
  smp_wmb();
  return 0;
}

static void __exit mutex_bench_exit(void) {
  // if (task_a) {
  //   kthread_stop(task_a);
  // }
  // if (task_b) {
  //   kthread_stop(task_b);
  // }
  printk("timeb %llu\n", time_b);
  printk("timec %llu\n", time_c);
  if (t == 1) {
    printk("B->C\n");
  } else {
    printk("C->B\n");
  }

  // printk("=========!!!!!!!!===========\n");
  // for (int i = 0; i < 4; i ++)
  //   thlet_lock_stats_report_data(&gs[i]);

  int i, j, k;
    uint64_t total_ipi_hw_lat = 0;
    uint64_t matched_count = 0;

    // 遍历每一个可能的发送者 (CPU i)
    for (i = 0; i < 3; i++) {
        // 遍历发送者的每一个发送记录
        for (j = 0; j < THLET_ITER; j++) {
            uint64_t s_ts = send_tsc[i][j];
            if (s_ts == 0) continue;

            uint64_t min_diff = (uint64_t)-1;
            int best_target_cpu = -1;

            // 在其他 CPU (接收者 k) 中寻找时间戳最接近且大于 s_ts 的接收记录
            for (k = 0; k < 3; k++) {
                if (i == k) continue; 
                
                for (int m = 0; m < THLET_ITER; m++) {
                    uint64_t r_ts = recv_tsc[k][m];
                    if (r_ts > s_ts) {
                        uint64_t diff = r_ts - s_ts;
                        if (diff < min_diff && diff < 10) { // 阈值过滤，防止匹配到错误的轮次
                            min_diff = diff;
                            best_target_cpu = k;
                        }
                    }
                }
            }

            if (best_target_cpu != -1) {
                total_ipi_hw_lat += min_diff;
                matched_count++;
            }
        }
    }

  if (matched_count) {
    printk("IPI HW Latency (Avg): %llu cycles (over %llu matches)\n", 
            total_ipi_hw_lat * 1000 / matched_count, matched_count);
  }
}

module_init(mutex_bench_init);
module_exit(mutex_bench_exit);
MODULE_LICENSE("GPL");