#!/usr/bin/env python3
"""Validate five runs per configuration and emit Figure 13 CSV data."""
import argparse
import csv
import io
import re
from pathlib import Path

COLUMNS = ['threads_per_cpu', 'latency_cycles', 'throughput_per_1m_cycles']
HEADERS = {
    'mutex': 'status,threads_per_cpu,total_workers,run,latency_cycles,throughput_ops_per_second',
    'argobots': 'status,argobots_version,execution_streams,threads_per_cpu,total_workers,run,acquisitions,handoffs,handoff_time_ticks,elapsed_time_ticks,latency_cycles,throughput_per_1m_cycles,tool_enabled,affinity_errors,order_errors,critical_cycles',
    'mcs-b': 'status,cpus,threads_per_cpu,total_workers,run,acquisitions,handoffs,handoff_time_ticks,elapsed_time_ticks,latency_cycles,throughput_per_1m_cycles,park_events,wake_syscalls,affinity_errors,order_errors,critical_cycles,spin_limit_iters',
}


def parse_rows(system, text):
    reader = csv.DictReader(io.StringIO(text))
    if reader.fieldnames != HEADERS[system].split(','):
        raise ValueError(f'{system}: incorrect raw CSV header')
    raw = list(reader)
    if len(raw) != 20:
        raise ValueError(f'{system}: expected 20 runs, got {len(raw)}')
    output = []
    for index, row in enumerate(raw):
        threads = (1, 2, 4, 8)[index // 5]
        run = index % 5 + 1
        if None in row or any(value is None for value in row.values()) or row['status'] != 'PASS':
            raise ValueError(f'{system}: failed/malformed run {index + 1}')
        numeric = {}
        for key, value in row.items():
            if key in ('status', 'argobots_version'):
                continue
            if not re.fullmatch(r'[0-9]+', value):
                raise ValueError(f'{system}: invalid {key}: {value!r}')
            numeric[key] = int(value)
        expected = {'threads_per_cpu': threads, 'total_workers': threads * 4, 'run': run}
        if system != 'mutex':
            acquisitions = threads * 4 * 10
            expected.update(acquisitions=acquisitions, handoffs=acquisitions - 1,
                            affinity_errors=0, order_errors=0, critical_cycles=10000)
            expected['execution_streams' if system == 'argobots' else 'cpus'] = 4
            if numeric['elapsed_time_ticks'] <= 0:
                raise ValueError(f'{system}: zero elapsed time')
            expected['latency_cycles'] = numeric['handoff_time_ticks'] * 1000 // (acquisitions - 1)
            expected['throughput_per_1m_cycles'] = acquisitions * 1000 // numeric['elapsed_time_ticks']
            if system == 'argobots':
                if row['argobots_version'] != '1.2rc1':
                    raise ValueError('unexpected Argobots version')
                expected['tool_enabled'] = 0
            else:
                expected['spin_limit_iters'] = 100
                if numeric['park_events'] <= 0:
                    raise ValueError('mcs-b: no blocking park events')
        if any(numeric[key] != value for key, value in expected.items()):
            raise ValueError(f'{system}: inconsistent configuration/metrics at run {index + 1}')
        throughput = (numeric['throughput_ops_per_second'] / 1000.0 if system == 'mutex'
                      else numeric['throughput_per_1m_cycles'])
        if throughput <= 0:
            raise ValueError(f'{system}: nonpositive throughput')
        if run == 1:
            latency_sum = throughput_sum = 0.0
        latency_sum += numeric['latency_cycles']
        throughput_sum += throughput
        if run == 5:
            output.append(dict(zip(COLUMNS, (str(threads), f'{latency_sum / 5:.2f}', f'{throughput_sum / 5:.2f}'))))
    return output


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('system', choices=HEADERS)
    parser.add_argument('raw_csv', type=Path)
    parser.add_argument('output_csv', type=Path)
    args = parser.parse_args()
    try:
        rows = parse_rows(args.system, args.raw_csv.read_text())
        args.output_csv.parent.mkdir(parents=True, exist_ok=True)
        with args.output_csv.open('w', newline='') as output:
            writer = csv.DictWriter(output, fieldnames=COLUMNS)
            writer.writeheader()
            writer.writerows(rows)
    except (OSError, ValueError) as error:
        raise SystemExit(f'[error] {error}')


if __name__ == '__main__':
    main()
