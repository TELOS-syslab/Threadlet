#define _GNU_SOURCE

#include <abt.h>
#include <sched.h>
#include <stdatomic.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#define NUM_XSTREAMS 4
#define MAX_THREADS_PER_XSTREAM 8
#define MAX_WORKERS (NUM_XSTREAMS * MAX_THREADS_PER_XSTREAM)
#define MEASURED_ACQUISITIONS_PER_WORKER 10
#define MAX_ACQUISITIONS \
    (MAX_WORKERS * MEASURED_ACQUISITIONS_PER_WORKER)
#define CRITICAL_CYCLES 10000ULL
#define SHARED_TICK_CYCLES 1000ULL

struct worker_arg {
    int worker_id;
    int expected_cpu;
};

struct acquisition_record {
    uint64_t enter;
    uint64_t exit;
};

static ABT_xstream g_xstreams[NUM_XSTREAMS];
static ABT_pool g_pools[NUM_XSTREAMS];
static ABT_thread g_threads[MAX_WORKERS];
static struct worker_arg g_worker_args[MAX_WORKERS];
static struct acquisition_record g_records[MAX_ACQUISITIONS];
static ABT_mutex g_mutex = ABT_MUTEX_NULL;
static ABT_barrier g_measure_barrier = ABT_BARRIER_NULL;

static uint64_t g_next_record;
static atomic_uint_fast64_t g_affinity_errors;
static atomic_uint_fast64_t g_record_overflows;
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

static void check_abt(int rc, const char *operation)
{
    if (rc != ABT_SUCCESS) {
        fprintf(stderr, "%s failed: Argobots error %d\n", operation, rc);
        exit(EXIT_FAILURE);
    }
}

static void busy_wait_cycles(uint64_t cycles)
{
    uint64_t start = rdcycle();

    while (rdcycle() - start < cycles) {
        __asm__ volatile("nop");
    }
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

static void worker_main(void *arg)
{
    const struct worker_arg *worker_arg = arg;
    int i;

    if (sched_getcpu() != worker_arg->expected_cpu) {
        atomic_fetch_add_explicit(&g_affinity_errors, 1,
                                  memory_order_relaxed);
    }

    /* One unmeasured warmup acquisition establishes steady contention. */
    check_abt(ABT_mutex_lock(g_mutex), "ABT_mutex_lock(warmup)");
    busy_wait_cycles(CRITICAL_CYCLES);
    check_abt(ABT_mutex_unlock(g_mutex), "ABT_mutex_unlock(warmup)");
    check_abt(ABT_barrier_wait(g_measure_barrier),
              "ABT_barrier_wait(measure)");

    for (i = 0; i < MEASURED_ACQUISITIONS_PER_WORKER; i++) {
        uint64_t record_index;
        uint64_t enter_time;

        check_abt(ABT_mutex_lock(g_mutex), "ABT_mutex_lock");
        enter_time = rdtime();
        record_index = g_next_record++;
        if (record_index >= MAX_ACQUISITIONS) {
            atomic_fetch_add_explicit(&g_record_overflows, 1,
                                      memory_order_relaxed);
            check_abt(ABT_mutex_unlock(g_mutex), "ABT_mutex_unlock");
            return;
        }

        g_records[record_index].enter = enter_time;
        busy_wait_cycles(CRITICAL_CYCLES);
        g_records[record_index].exit = rdtime();
        check_abt(ABT_mutex_unlock(g_mutex), "ABT_mutex_unlock");
    }

    if (sched_getcpu() != worker_arg->expected_cpu) {
        atomic_fetch_add_explicit(&g_affinity_errors, 1,
                                  memory_order_relaxed);
    }
    g_worker_completed[worker_arg->worker_id] = 1;
}

int main(int argc, char **argv)
{
    ABT_bool tool_enabled = ABT_TRUE;
    ABT_bool affinity_enabled = ABT_FALSE;
    ABT_bool uses_fcontext = ABT_TRUE;
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
    int threads_per_cpu;
    int run;
    int total_workers;
    int completed_workers = 0;
    int i;
    int status_ok = 1;

    if (argc != 3 ||
        !parse_bounded_int(argv[1], 1, MAX_THREADS_PER_XSTREAM,
                           &threads_per_cpu) ||
        !valid_threads_per_cpu(threads_per_cpu) ||
        !parse_bounded_int(argv[2], 1, 5, &run)) {
        fprintf(stderr, "usage: %s <threads-per-cpu:1|2|4|8> <run:1..5>\n",
                argv[0]);
        return EXIT_FAILURE;
    }

    total_workers = NUM_XSTREAMS * threads_per_cpu;
    expected_acquisitions =
        (uint64_t)total_workers * MEASURED_ACQUISITIONS_PER_WORKER;

    check_abt(ABT_init(argc, argv), "ABT_init");
    check_abt(ABT_info_query_config(ABT_INFO_QUERY_KIND_ENABLED_TOOL,
                                    &tool_enabled),
              "ABT_info_query_config(tool)");
    check_abt(ABT_info_query_config(ABT_INFO_QUERY_KIND_ENABLED_AFFINITY,
                                    &affinity_enabled),
              "ABT_info_query_config(affinity)");
    check_abt(ABT_info_query_config(ABT_INFO_QUERY_KIND_FCONTEXT,
                                    &uses_fcontext),
              "ABT_info_query_config(fcontext)");

    check_abt(ABT_xstream_self(&g_xstreams[0]), "ABT_xstream_self");
    for (i = 1; i < NUM_XSTREAMS; i++) {
        check_abt(ABT_xstream_create(ABT_SCHED_NULL, &g_xstreams[i]),
                  "ABT_xstream_create");
    }
    for (i = 0; i < NUM_XSTREAMS; i++) {
        int bound_cpu = -1;

        check_abt(ABT_xstream_set_cpubind(g_xstreams[i], i),
                  "ABT_xstream_set_cpubind");
        check_abt(ABT_xstream_get_cpubind(g_xstreams[i], &bound_cpu),
                  "ABT_xstream_get_cpubind");
        if (bound_cpu != i) {
            atomic_fetch_add_explicit(&g_affinity_errors, 1,
                                      memory_order_relaxed);
        }
        check_abt(ABT_xstream_get_main_pools(g_xstreams[i], 1,
                                             &g_pools[i]),
                  "ABT_xstream_get_main_pools");
    }

    check_abt(ABT_mutex_create(&g_mutex), "ABT_mutex_create");
    check_abt(ABT_barrier_create(total_workers + 1, &g_measure_barrier),
              "ABT_barrier_create");

    for (i = 0; i < total_workers; i++) {
        int pool_index = i % NUM_XSTREAMS;

        g_worker_args[i].worker_id = i;
        g_worker_args[i].expected_cpu = pool_index;
        check_abt(ABT_thread_create(g_pools[pool_index], worker_main,
                                    &g_worker_args[i],
                                    ABT_THREAD_ATTR_NULL, &g_threads[i]),
                  "ABT_thread_create");
    }

    check_abt(ABT_barrier_wait(g_measure_barrier),
              "ABT_barrier_wait(main)");
    for (i = 0; i < total_workers; i++) {
        check_abt(ABT_thread_free(&g_threads[i]), "ABT_thread_free");
        completed_workers += g_worker_completed[i];
    }
    for (i = 1; i < NUM_XSTREAMS; i++) {
        check_abt(ABT_xstream_join(g_xstreams[i]), "ABT_xstream_join");
        check_abt(ABT_xstream_free(&g_xstreams[i]), "ABT_xstream_free");
    }

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

    if (strcmp(ABT_VERSION, "1.2rc1") != 0 ||
        tool_enabled != ABT_FALSE ||
        affinity_enabled != ABT_TRUE ||
        uses_fcontext != ABT_FALSE ||
        completed_workers != total_workers ||
        g_next_record != expected_acquisitions ||
        handoffs != expected_acquisitions - 1 ||
        elapsed_time_ticks == 0 ||
        order_errors != 0 ||
        affinity_errors != 0 ||
        record_overflows != 0) {
        status_ok = 0;
    }

    printf("status,argobots_version,execution_streams,threads_per_cpu,"
           "total_workers,run,acquisitions,handoffs,handoff_time_ticks,"
           "elapsed_time_ticks,latency_cycles,throughput_per_1m_cycles,"
           "tool_enabled,affinity_errors,order_errors,critical_cycles\n");
    printf("%s,%s,%d,%d,%d,%d,%llu,%llu,%llu,%llu,%llu,%llu,%d,%llu,"
           "%llu,%llu\n",
           status_ok ? "PASS" : "FAIL", ABT_VERSION, NUM_XSTREAMS,
           threads_per_cpu, total_workers, run,
           (unsigned long long)g_next_record,
           (unsigned long long)handoffs,
           (unsigned long long)handoff_time_ticks,
           (unsigned long long)elapsed_time_ticks,
           (unsigned long long)latency_cycles,
           (unsigned long long)throughput_per_1m_cycles,
           (int)tool_enabled, (unsigned long long)affinity_errors,
           (unsigned long long)order_errors,
           (unsigned long long)CRITICAL_CYCLES);

    if (!status_ok) {
        fprintf(stderr,
                "argobots_fig13 validation failed: version=%s "
                "tool_enabled=%d affinity_enabled=%d uses_fcontext=%d "
                "threads_per_cpu=%d run=%d completed_workers=%d "
                "acquisitions=%llu handoffs=%llu handoff_ticks=%llu "
                "elapsed_ticks=%llu affinity_errors=%llu "
                "order_errors=%llu record_overflows=%llu\n",
                ABT_VERSION, (int)tool_enabled, (int)affinity_enabled,
                (int)uses_fcontext, threads_per_cpu, run,
                completed_workers, (unsigned long long)g_next_record,
                (unsigned long long)handoffs,
                (unsigned long long)handoff_time_ticks,
                (unsigned long long)elapsed_time_ticks,
                (unsigned long long)affinity_errors,
                (unsigned long long)order_errors,
                (unsigned long long)record_overflows);
    }

    check_abt(ABT_barrier_free(&g_measure_barrier), "ABT_barrier_free");
    check_abt(ABT_mutex_free(&g_mutex), "ABT_mutex_free");
    check_abt(ABT_finalize(), "ABT_finalize");
    return status_ok ? EXIT_SUCCESS : EXIT_FAILURE;
}
