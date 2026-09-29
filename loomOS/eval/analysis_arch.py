#!/usr/bin/env python3
"""Analyze ctx-saver high-priority threadlet wakeup-to-start latency."""

# python3 analysis_arch.py hw_debug_2.9g.log --expected-start-samples 390 --expected-wakeups 390

from __future__ import annotations

import argparse
import math
import re
from collections import defaultdict
from dataclasses import dataclass
from typing import Dict, Iterable, List, Optional, Tuple


CTX_SAVER_TEST_STAGE = 700
WAKEUP_PRINT_BASE = 700_000
START_PRINT_BASE = 710_000
MARKER_SPAN = 10_000
EXPECTED_START_SAMPLES = 10
EXPECTED_WAKEUPS = 151


@dataclass(frozen=True)
class Event:
    cycle: int
    cpu: int
    stage: int
    data: int


@dataclass(frozen=True)
class WakeupSample:
    cpu: int
    hart: int
    wakeup_cycle: int
    start_cycle: int

    @property
    def latency(self) -> int:
        return self.start_cycle - self.wakeup_cycle


@dataclass
class Analysis:
    samples: List[WakeupSample]
    wakeups: Dict[Tuple[int, int], List[int]]
    starts: Dict[Tuple[int, int], List[int]]
    unused_wakeups: Dict[Tuple[int, int], List[int]]
    unmatched_starts: List[Event]


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Analyze high-priority threadlet wakeup-to-start latency."
    )
    parser.add_argument(
        "log",
        nargs="?",
        default="hw_debug_2.9f.log",
        help="Path to hardware log (default: hw_debug_2.9f.log)",
    )
    parser.add_argument(
        "--details",
        action="store_true",
        help="Print every paired wakeup/start sample.",
    )
    parser.add_argument(
        "--expected-samples",
        type=int,
        default=None,
        help="Compatibility alias for --expected-start-samples.",
    )
    parser.add_argument(
        "--expected-start-samples",
        type=int,
        default=EXPECTED_START_SAMPLES,
        help=f"Expected first-start latency samples (default: {EXPECTED_START_SAMPLES}).",
    )
    parser.add_argument(
        "--expected-wakeups",
        type=int,
        default=EXPECTED_WAKEUPS,
        help=f"Expected low-priority wakeup markers (default: {EXPECTED_WAKEUPS}).",
    )
    return parser.parse_args()


def parse_event(line: str) -> Optional[Event]:
    match = re.search(
        r"CYCLE:\s*(\d+).*?\[cpu=(\d+)\].*?"
        r"stage:\s*(\d+),\s*addr:\s*0x[0-9a-fA-F]+\s*\(\s*(\d+)\s*\)",
        line,
    )
    if match is None:
        return None
    return Event(
        cycle=int(match.group(1)),
        cpu=int(match.group(2)),
        stage=int(match.group(3)),
        data=int(match.group(4)),
    )


def analyze(lines: Iterable[str]) -> Analysis:
    wakeups: Dict[Tuple[int, int], List[int]] = defaultdict(list)
    starts: Dict[Tuple[int, int], List[int]] = defaultdict(list)
    samples: List[WakeupSample] = []
    unmatched_starts: List[Event] = []
    used_wakeups: Dict[Tuple[int, int], set[int]] = defaultdict(set)

    for line in lines:
        event = parse_event(line)
        if event is None or event.stage != CTX_SAVER_TEST_STAGE:
            continue

        if WAKEUP_PRINT_BASE <= event.data < WAKEUP_PRINT_BASE + MARKER_SPAN:
            hart = event.data - WAKEUP_PRINT_BASE
            wakeups[(event.cpu, hart)].append(event.cycle)
            continue

        if START_PRINT_BASE <= event.data < START_PRINT_BASE + MARKER_SPAN:
            hart = event.data - START_PRINT_BASE
            key = (event.cpu, hart)
            starts[key].append(event.cycle)

            # The current run_ctx_saver_test prints a start marker only on a
            # high-priority threadlet's first entry. Pair that start with the
            # closest preceding wakeup for the same hart; later repeated
            # wakeups are expected markers, not missed starts.
            candidates = [
                index
                for index, cycle in enumerate(wakeups[key])
                if cycle <= event.cycle and index not in used_wakeups[key]
            ]
            if not candidates:
                unmatched_starts.append(event)
                continue
            wakeup_index = candidates[-1]
            wakeup_cycle = wakeups[key][wakeup_index]
            used_wakeups[key].add(wakeup_index)
            samples.append(
                WakeupSample(
                    cpu=event.cpu,
                    hart=hart,
                    wakeup_cycle=wakeup_cycle,
                    start_cycle=event.cycle,
                )
            )

    unused_wakeups = {
        key: [cycle for index, cycle in enumerate(cycles) if index not in used_wakeups[key]]
        for key, cycles in wakeups.items()
        if any(index not in used_wakeups[key] for index in range(len(cycles)))
    }
    return Analysis(samples, wakeups, starts, unused_wakeups, unmatched_starts)


def mean(values: List[int]) -> Optional[float]:
    if not values:
        return None
    return sum(values) / float(len(values))


def percentile(values: List[int], ratio: float) -> Optional[int]:
    if not values:
        return None
    ordered = sorted(values)
    index = max(0, math.ceil(len(ordered) * ratio) - 1)
    return ordered[index]


def format_float(value: Optional[float]) -> str:
    return "NA" if value is None else f"{value:.2f}"


def print_stats(name: str, samples: List[WakeupSample]) -> None:
    latencies = [sample.latency for sample in samples]
    print(
        f"{name}: count={len(latencies)}, avg_cycles={format_float(mean(latencies))}, "
        f"p99_cycles={percentile(latencies, 0.99) if latencies else 'NA'}, "
        f"min_cycles={min(latencies) if latencies else 'NA'}, "
        f"max_cycles={max(latencies) if latencies else 'NA'}"
    )


def main() -> int:
    args = parse_args()
    expected_start_samples = (
        args.expected_samples
        if args.expected_samples is not None
        else args.expected_start_samples
    )
    with open(args.log, "r", encoding="utf-8", errors="ignore") as log_file:
        result = analyze(log_file)

    wakeup_count = sum(len(cycles) for cycles in result.wakeups.values())
    start_count = sum(len(cycles) for cycles in result.starts.values())
    unused_wakeup_count = sum(
        len(cycles) for cycles in result.unused_wakeups.values()
    )

    print(f"log_path: {args.log}")
    print(
        f"markers: stage={CTX_SAVER_TEST_STAGE}, "
        f"wakeup={WAKEUP_PRINT_BASE}+hart, start={START_PRINT_BASE}+hart"
    )
    print(
        f"marker_counts: wakeups={wakeup_count}, starts={start_count}, "
        f"high_harts={len(result.starts)}"
    )
    print_stats("overall", result.samples)

    by_thread: Dict[Tuple[int, int], List[WakeupSample]] = defaultdict(list)
    for sample in result.samples:
        by_thread[(sample.cpu, sample.hart)].append(sample)
    for (cpu, hart), samples in sorted(by_thread.items()):
        print_stats(f"cpu={cpu} hart={hart}", samples)

    print(
        f"completeness: expected_start_samples={expected_start_samples}, "
        f"paired={len(result.samples)}, "
        f"unmatched_starts={len(result.unmatched_starts)}, "
        f"expected_wakeups={args.expected_wakeups}, "
        f"wakeups={wakeup_count}, "
        f"unused_repeated_wakeups={unused_wakeup_count}"
    )
    status_ok = (
        len(result.samples) == expected_start_samples
        and wakeup_count == args.expected_wakeups
        and not result.unmatched_starts
    )
    print(
        "status: "
        + ("OK" if status_ok else "MISMATCH")
    )

    if args.details:
        print("idx,cpu,hart,wakeup_cycle,start_cycle,latency_cycles")
        for index, sample in enumerate(result.samples):
            print(
                f"{index},{sample.cpu},{sample.hart},{sample.wakeup_cycle},"
                f"{sample.start_cycle},{sample.latency}"
            )

    return 0


if __name__ == "__main__":
    raise SystemExit(main())
