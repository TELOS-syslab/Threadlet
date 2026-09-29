#!/usr/bin/env python3
"""Analyze Figure 8 EEVDF scheduling rounds from a hardware log."""

from __future__ import annotations

import argparse
import re
from dataclasses import dataclass
from typing import Iterable, Optional


SCHEDULING_STAGE = 999
TAG_ROUND_START = 100
TAG_SCHEDULER_INIT_END = 101
TAG_WORKER_DONE = 200
TAG_SCHEDULER_UPDATE_START = 300
TAG_SCHEDULER_UPDATE_END = 301
TAG_ROUND_END = 9999
ROUND_WORKER_COUNTS = (2, 4, 8)


@dataclass(frozen=True)
class Event:
    cycle: int
    cpu: int
    tag: int


@dataclass(frozen=True)
class RoundMetrics:
    workers: int
    total_cycles: int
    scheduler_cycles: int
    throughput_ktps: float
    scheduling_overhead_percent: float


def parse_event(line: str) -> Optional[Event]:
    match = re.search(
        r"CYCLE:\s*(\d+).*?\[cpu=(\d+)\].*?stage:\s*(\d+),\s*"
        r"addr:\s*0x[0-9a-fA-F]+\s*\(\s*(\d+)\s*\)",
        line,
    )
    if match is None or int(match.group(3)) != SCHEDULING_STAGE:
        return None
    return Event(
        cycle=int(match.group(1)),
        cpu=int(match.group(2)),
        tag=int(match.group(4)),
    )


def collect_rounds(lines: Iterable[str]) -> list[list[Event]]:
    rounds: list[list[Event]] = []
    current: list[Event] = []
    tracked_tags = {
        TAG_ROUND_START,
        TAG_SCHEDULER_INIT_END,
        TAG_WORKER_DONE,
        TAG_SCHEDULER_UPDATE_START,
        TAG_SCHEDULER_UPDATE_END,
    }

    for line in lines:
        event = parse_event(line)
        if event is None:
            continue
        if event.tag == TAG_ROUND_END:
            if current:
                rounds.append(current)
                current = []
            elif len(rounds) < len(ROUND_WORKER_COUNTS):
                raise ValueError("unexpected empty round-end marker")
            continue
        if event.tag in tracked_tags:
            if len(rounds) == len(ROUND_WORKER_COUNTS):
                raise ValueError("unexpected scheduling event after the final round")
            current.append(event)

    if current:
        raise ValueError("the final scheduling round has no round-end marker")
    if len(rounds) != len(ROUND_WORKER_COUNTS):
        raise ValueError(
            f"expected {len(ROUND_WORKER_COUNTS)} scheduling rounds, found {len(rounds)}"
        )
    return rounds


def analyze_round(events: list[Event], workers: int) -> RoundMetrics:
    if len(events) < 4 or [event.tag for event in events[:2]] != [
        TAG_ROUND_START,
        TAG_SCHEDULER_INIT_END,
    ]:
        raise ValueError(
            f"workers={workers} must start with round-start and initialization-end"
        )

    cpus = {event.cpu for event in events}
    if len(cpus) != 1:
        raise ValueError(f"workers={workers} events span multiple CPUs: {sorted(cpus)}")

    cycles = [event.cycle for event in events]
    if cycles != sorted(cycles):
        raise ValueError(f"workers={workers} event cycles are not monotonic")

    round_start = events[0].cycle
    initialization_end = events[1].cycle
    if initialization_end <= round_start:
        raise ValueError(f"workers={workers} has a non-positive initialization interval")

    scheduler_cycles = initialization_end - round_start
    worker_done_count = 0
    update_start: Optional[int] = None
    final_update_end: Optional[int] = None
    update_count = 0

    for event in events[2:]:
        if event.tag == TAG_WORKER_DONE:
            if update_start is not None:
                raise ValueError(f"workers={workers} worker completed inside an update interval")
            worker_done_count += 1
        elif event.tag == TAG_SCHEDULER_UPDATE_START:
            if update_start is not None:
                raise ValueError(f"workers={workers} has nested scheduler update starts")
            update_start = event.cycle
        elif event.tag == TAG_SCHEDULER_UPDATE_END:
            if update_start is None:
                raise ValueError(f"workers={workers} has an unmatched scheduler update end")
            if event.cycle <= update_start:
                raise ValueError(f"workers={workers} has a non-positive update interval")
            scheduler_cycles += event.cycle - update_start
            final_update_end = event.cycle
            update_count += 1
            update_start = None
        else:
            raise ValueError(f"workers={workers} has an unexpected marker {event.tag}")

    if update_start is not None:
        raise ValueError(f"workers={workers} has an unmatched scheduler update start")
    if update_count == 0 or final_update_end is None:
        raise ValueError(f"workers={workers} has no scheduler update interval")
    if events[-1].tag != TAG_SCHEDULER_UPDATE_END:
        raise ValueError(f"workers={workers} does not end with a scheduler update")
    if worker_done_count != workers:
        raise ValueError(
            f"workers={workers} expected {workers} worker completions, "
            f"found {worker_done_count}"
        )

    total_cycles = final_update_end - round_start
    if total_cycles <= 0:
        raise ValueError(f"workers={workers} has a non-positive round cycle count")

    return RoundMetrics(
        workers=workers,
        total_cycles=total_cycles,
        scheduler_cycles=scheduler_cycles,
        throughput_ktps=workers * 1_000_000.0 / total_cycles,
        scheduling_overhead_percent=scheduler_cycles * 100.0 / total_cycles,
    )


def analyze(lines: Iterable[str]) -> list[RoundMetrics]:
    rounds = collect_rounds(lines)
    return [
        analyze_round(events, workers)
        for events, workers in zip(rounds, ROUND_WORKER_COUNTS)
    ]


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "log",
        nargs="?",
        default="figure8_threadlet.log",
        help="hardware log path (default: figure8_threadlet.log)",
    )
    return parser.parse_args()


def main() -> None:
    args = parse_args()
    try:
        with open(args.log, "r", encoding="utf-8", errors="ignore") as log_file:
            metrics = analyze(log_file)
    except (OSError, ValueError) as error:
        raise SystemExit(f"[error] {error}") from error

    print(
        "workers,total_cycles,scheduler_cycles,"
        "worker_throughput_ktps,scheduling_overhead_percent"
    )
    for row in metrics:
        print(
            f"{row.workers},{row.total_cycles},{row.scheduler_cycles},"
            f"{row.throughput_ktps:.2f},{row.scheduling_overhead_percent:.2f}"
        )


if __name__ == "__main__":
    main()
