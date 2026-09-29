#define _GNU_SOURCE

#include <linux/futex.h>
#include <pthread.h>
#include <sched.h>
#include <stdatomic.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <sys/syscall.h>
#include <unistd.h>

#define NUM_CPUS 4
#define MAX_THREADS_PER_CPU 8
#define MAX_WORKERS (NUM_CPUS * MAX_THREADS_PER_CPU)
#define MEASURED_ACQUISITIONS_PER_WORKER 10
#define MAX_ACQUISITIONS \
    (MAX_WORKERS * MEASURED_ACQUISITIONS_PER_WORKER)
#define CRITICAL_CYCLES 10000ULL
#define SHARED_TICK_CYCLES 1000ULL
/*
 * Bounded local spin before parking: keeps a syscall-free fast path for
 * handoffs shorter than the spin window, while longer waits still fall
 * back to futex_wait. Iteration-bounded (not cycle-bounded) so the spin
 * itself never needs an extra rdcycle read.
 */
#define SPIN_LIMIT_ITERS 100
#define FUTEX_OP_WAIT (FUTEX_WAIT | FUTEX_PRIVATE_FLAG)
#define FUTEX_OP_WAKE (FUTEX_WAKE | FUTEX_PRIVATE_FLAG)

struct mcs_node {
    _Atomic(struct mcs_node *) next;
    atomic_uint locked; /* futex word: 1 = waiting, 0 = released */
} __attribute__((aligned(64)));

struct acquisition_record {
    uint64_t enter;
    uint64_t exit;
};

struct worker_arg {
    int worker_id;
    int expected_cpu;
};

static _Atomic(struct mcs_node *) g_tail = NULL;
static struct mcs_node g_nodes[MAX_WORKERS];
static struct worker_arg g_worker_args[MAX_WORKERS];
static pthread_t g_threads[MAX_WORKERS];
static struct acquisition_record g_records[MAX_ACQUISITIONS];
static pthread_barrier_t g_measure_barrier;

static uint64_t g_next_record;
static atomic_uint_fast64_t g_affinity_errors;
static atomic_uint_fast64_t g_record_overflows;
static atomic_uint_fast64_t g_park_events;
static atomic_uint_fast64_t g_wake_syscalls;
static int g_worker_completed[MAX_WORKERS];

static inline uint64_t rdcycle(void)
{
    uint64_t value;

    __asm__ volatile("rdcycle %0" : "=r"(value));
    return value;
}

static inline uint64_t rdtime(void)
{
    uint64_t value;

    __asm__ volatile("rdtime %0" : "=r"(value));
    return value;
}

static void busy_wait_cycles(uint64_t cycles)
{
    uint64_t start = rdcycle();

    while (rdcycle() - start < cycles) {
        __asm__ volatile("nop");
    }
}

static long futex_wait_word(atomic_uint *addr, unsigned expected)
{
    return syscall(SYS_futex, (void *)addr, FUTEX_OP_WAIT, expected,
                   (void *)NULL, (void *)NULL, 0);
}

static long futex_wake_one(atomic_uint *addr)
{
    return syscall(SYS_futex, (void *)addr, FUTEX_OP_WAKE, 1,
                   (void *)NULL, (void *)NULL, 0);
}

/*
 * MCS queue lock. Each thread spins only on its own node (never on a
 * shared cache line), so contention does not create coherence traffic
 * proportional to the waiter count. On top of the textbook algorithm,
 * a waiter spins locally for SPIN_LIMIT_ITERS iterations before parking
 * on its own node's futex word ("spin-then-park"), and its predecessor
 * always pairs a release store with futex_wake so a waiter that parked
 * between the spin and the wait syscall can never miss the wakeup.
 */
static void mcs_lock(struct mcs_node *self)
{
    struct mcs_node *pred;
    int spins;

    atomic_store_explicit(&self->next, NULL, memory_order_relaxed);
    atomic_store_explicit(&self->locked, 1, memory_order_relaxed);

    pred = atomic_exchange_explicit(&g_tail, self, memory_order_acq_rel);
    if (pred == NULL) {
        return;
    }
    atomic_store_explicit(&pred->next, self, memory_order_release);

    for (spins = 0; spins < SPIN_LIMIT_ITERS; spins++) {
        if (atomic_load_explicit(&self->locked, memory_order_acquire) == 0) {
            return;
        }
        __asm__ volatile("nop");
    }

    while (atomic_load_explicit(&self->locked, memory_order_acquire) != 0) {
        atomic_fetch_add_explicit(&g_park_events, 1, memory_order_relaxed);
        /*
         * futex_wait atomically re-checks *addr == expected before
         * sleeping, so a release that already happened between the last
         * load above and this call makes the syscall return immediately
         * (EAGAIN) instead of missing the wakeup.
         */
        (void)futex_wait_word(&self->locked, 1);
    }
}

static void mcs_unlock(struct mcs_node *self)
{
    struct mcs_node *succ =
        atomic_load_explicit(&self->next, memory_order_acquire);

    if (succ == NULL) {
        struct mcs_node *expected = self;

        if (atomic_compare_exchange_strong_explicit(
                &g_tail, &expected, NULL, memory_order_acq_rel,
                memory_order_acquire)) {
            return;
        }
        /*
         * A successor is enqueuing concurrently: it has already won the
         * tail exchange and is one store away from linking self->next,
         * so this bounded spin (not a park) is standard for MCS.
         */
        do {
            succ = atomic_load_explicit(&self->next, memory_order_acquire);
        } while (succ == NULL);
    }

    atomic_store_explicit(&succ->locked, 0, memory_order_release);
    /*
     * Always wake: the unlocker cannot tell whether succ is still in its
     * spin phase or has already parked, and futex_wake on an address
     * with no waiters is a cheap no-op in the kernel.
     */
    atomic_fetch_add_explicit(&g_wake_syscalls, 1, memory_order_relaxed);
    (void)futex_wake_one(&succ->locked);
}

static int parse_bounded_int(const char *text, int minimum, int maximum,
                             int *value)
{
    char *end = NULL;
    long parsed = strtol(text, &end, 10);

    if (text[0] == '\0' || end == NULL || *end != '\0' ||
        parsed < minimum || parsed > maximum) {
        return 0;
    }
    *value = (int)parsed;
    return 1;
}

static int valid_threads_per_cpu(int threads_per_cpu)
{
    return threads_per_cpu == 1 || threads_per_cpu == 2 ||
           threads_per_cpu == 4 || threads_per_cpu == 8;
}

static void *worker_main(void *arg)
{
    const struct worker_arg *worker_arg = arg;
    struct mcs_node *node = &g_nodes[worker_arg->worker_id];
    int i;

    if (sched_getcpu() != worker_arg->expected_cpu) {
        atomic_fetch_add_explicit(&g_affinity_errors, 1,
                                  memory_order_relaxed);
    }

    /* One unmeasured warmup acquisition establishes steady contention. */
    mcs_lock(node);
    busy_wait_cycles(CRITICAL_CYCLES);
    mcs_unlock(node);
    pthread_barrier_wait(&g_measure_barrier);

    for (i = 0; i < MEASURED_ACQUISITIONS_PER_WORKER; i++) {
        uint64_t record_index;
        uint64_t enter_time;

        mcs_lock(node);
        enter_time = rdtime();
        record_index = g_next_record++;
        if (record_index >= MAX_ACQUISITIONS) {
            atomic_fetch_add_explicit(&g_record_overflows, 1,
                                      memory_order_relaxed);
            mcs_unlock(node);
            return NULL;
        }

        g_records[record_index].enter = enter_time;
        busy_wait_cycles(CRITICAL_CYCLES);
        g_records[record_index].exit = rdtime();
        mcs_unlock(node);
    }

    if (sched_getcpu() != worker_arg->expected_cpu) {
        atomic_fetch_add_explicit(&g_affinity_errors, 1,
                                  memory_order_relaxed);
    }
    g_worker_completed[worker_arg->worker_id] = 1;
    return NULL;
}

int main(int argc, char **argv)
{
    uint64_t expected_acquisitions;
    uint64_t records_to_check;
    uint64_t handoffs;
    uint64_t handoff_time_ticks = 0;
    uint64_t elapsed_time_ticks = 0;
    uint64_t latency_cycles = 0;
    uint64_t throughput_per_1m_cycles = 0;
    uint64_t affinity_errors;
    uint64_t order_errors = 0;
    uint64_t record_overflows;
    uint64_t park_events;
    uint64_t wake_syscalls;
    int threads_per_cpu;
    int run;
    int total_workers;
    int completed_workers = 0;
    int i;
    int status_ok = 1;

    if (argc != 3 ||
        !parse_bounded_int(argv[1], 1, MAX_THREADS_PER_CPU,
                           &threads_per_cpu) ||
        !valid_threads_per_cpu(threads_per_cpu) ||
        !parse_bounded_int(argv[2], 1, 5, &run)) {
        fprintf(stderr, "usage: %s <threads-per-cpu:1|2|4|8> <run:1..5>\n",
                argv[0]);
        return EXIT_FAILURE;
    }

    total_workers = NUM_CPUS * threads_per_cpu;
    expected_acquisitions =
        (uint64_t)total_workers * MEASURED_ACQUISITIONS_PER_WORKER;

    if (pthread_barrier_init(&g_measure_barrier, NULL,
                             (unsigned)total_workers) != 0) {
        fprintf(stderr, "pthread_barrier_init failed\n");
        return EXIT_FAILURE;
    }

    for (i = 0; i < total_workers; i++) {
        pthread_attr_t attr;
        cpu_set_t cpuset;
        int assigned_cpu = i % NUM_CPUS;

        g_worker_args[i].worker_id = i;
        g_worker_args[i].expected_cpu = assigned_cpu;

        if (pthread_attr_init(&attr) != 0) {
            fprintf(stderr, "pthread_attr_init failed\n");
            return EXIT_FAILURE;
        }
        CPU_ZERO(&cpuset);
        CPU_SET(assigned_cpu, &cpuset);
        if (pthread_attr_setaffinity_np(&attr, sizeof(cpuset), &cpuset) !=
            0) {
            fprintf(stderr, "pthread_attr_setaffinity_np failed\n");
            return EXIT_FAILURE;
        }
        if (pthread_create(&g_threads[i], &attr, worker_main,
                           &g_worker_args[i]) != 0) {
            fprintf(stderr, "pthread_create failed\n");
            return EXIT_FAILURE;
        }
        pthread_attr_destroy(&attr);
    }

    for (i = 0; i < total_workers; i++) {
        pthread_join(g_threads[i], NULL);
        completed_workers += g_worker_completed[i];
    }
    pthread_barrier_destroy(&g_measure_barrier);

    records_to_check = g_next_record < expected_acquisitions
                           ? g_next_record
                           : expected_acquisitions;
    for (i = 0; i < (int)records_to_check; i++) {
        if (g_records[i].exit <= g_records[i].enter) {
            order_errors++;
        }
        if (i > 0) {
            if (g_records[i].enter < g_records[i - 1].exit) {
                order_errors++;
            } else {
                handoff_time_ticks +=
                    g_records[i].enter - g_records[i - 1].exit;
            }
        }
    }

    handoffs = records_to_check > 0 ? records_to_check - 1 : 0;
    if (records_to_check > 0 &&
        g_records[records_to_check - 1].exit > g_records[0].enter) {
        elapsed_time_ticks =
            g_records[records_to_check - 1].exit - g_records[0].enter;
    } else {
        order_errors++;
    }
    if (handoffs != 0) {
        latency_cycles =
            handoff_time_ticks * SHARED_TICK_CYCLES / handoffs;
    }
    if (elapsed_time_ticks != 0) {
        throughput_per_1m_cycles =
            records_to_check * SHARED_TICK_CYCLES / elapsed_time_ticks;
    }

    affinity_errors =
        atomic_load_explicit(&g_affinity_errors, memory_order_relaxed);
    record_overflows =
        atomic_load_explicit(&g_record_overflows, memory_order_relaxed);
    park_events = atomic_load_explicit(&g_park_events, memory_order_relaxed);
    wake_syscalls =
        atomic_load_explicit(&g_wake_syscalls, memory_order_relaxed);

    if (completed_workers != total_workers ||
        g_next_record != expected_acquisitions ||
        handoffs != expected_acquisitions - 1 ||
        elapsed_time_ticks == 0 ||
        order_errors != 0 ||
        affinity_errors != 0 ||
        record_overflows != 0 ||
        park_events == 0) {
        status_ok = 0;
    }

    printf("status,cpus,threads_per_cpu,total_workers,run,acquisitions,"
           "handoffs,handoff_time_ticks,elapsed_time_ticks,latency_cycles,"
           "throughput_per_1m_cycles,park_events,wake_syscalls,"
           "affinity_errors,order_errors,critical_cycles,spin_limit_iters\n");
    printf("%s,%d,%d,%d,%d,%llu,%llu,%llu,%llu,%llu,%llu,%llu,%llu,%llu,"
           "%llu,%llu,%d\n",
           status_ok ? "PASS" : "FAIL", NUM_CPUS, threads_per_cpu,
           total_workers, run,
           (unsigned long long)g_next_record,
           (unsigned long long)handoffs,
           (unsigned long long)handoff_time_ticks,
           (unsigned long long)elapsed_time_ticks,
           (unsigned long long)latency_cycles,
           (unsigned long long)throughput_per_1m_cycles,
           (unsigned long long)park_events,
           (unsigned long long)wake_syscalls,
           (unsigned long long)affinity_errors,
           (unsigned long long)order_errors,
           (unsigned long long)CRITICAL_CYCLES,
           (int)SPIN_LIMIT_ITERS);

    if (!status_ok) {
        fprintf(stderr,
                "mcs_b_fig13 validation failed: threads_per_cpu=%d run=%d "
                "completed_workers=%d acquisitions=%llu handoffs=%llu "
                "elapsed_ticks=%llu affinity_errors=%llu order_errors=%llu "
                "record_overflows=%llu park_events=%llu\n",
                threads_per_cpu, run, completed_workers,
                (unsigned long long)g_next_record,
                (unsigned long long)handoffs,
                (unsigned long long)elapsed_time_ticks,
                (unsigned long long)affinity_errors,
                (unsigned long long)order_errors,
                (unsigned long long)record_overflows,
                (unsigned long long)park_events);
    }

    return status_ok ? EXIT_SUCCESS : EXIT_FAILURE;
}
