#!/usr/bin/env python3
"""
Analyze MCS critical-section cycles from hardware log.

Tracked tags (stage=20):
- MCS_HW_PRINT_CRITICAL_ENTER = 2010
- MCS_HW_PRINT_CRITICAL_EXIT  = 2000

Metrics:
1) throughput = total_critical_sections / (last_exit - first_enter)
   Unit: times / 1M cycles
2) latency = average of (next_enter - current_exit)
"""

from __future__ import annotations

import argparse
import re
from dataclasses import dataclass
from typing import Iterable, List, Optional, Tuple

MCS_STAGE = 20
TAG_CRITICAL_ENTER = 2010
TAG_CRITICAL_EXIT = 2000
CYCLES_PER_MILLION = 1_000_000


@dataclass
class Event:
    cycle: int
    cpu: int
    stage: int
    tag: int


@dataclass
class CriticalSection:
    idx: int
    enter_cycle: int
    exit_cycle: int


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description="Analyze MCS critical-section timing.")
    parser.add_argument(
        "log",
        nargs="?",
        default="hwm48.log",
        help="Path to MCS hardware log (default: hwm48.log)",
    )
    return parser.parse_args()


def parse_event(line: str) -> Optional[Event]:
    m = re.search(
        r"CYCLE:\s*(\d+).*?\[cpu=(\d+)\].*?stage:\s*(\d+),\s*addr:\s*0x[0-9a-fA-F]+\s*\(\s*(\d+)\s*\)",
        line,
    )
    if not m:
        return None
    return Event(
        cycle=int(m.group(1)),
        cpu=int(m.group(2)),
        stage=int(m.group(3)),
        tag=int(m.group(4)),
    )


def mean(vals: List[int]) -> Optional[float]:
    if not vals:
        return None
    return float(sum(vals)) / float(len(vals))


def format_num(v: Optional[float]) -> str:
    if v is None:
        return "NA"
    if isinstance(v, float):
        return f"{v:.2f}"
    return str(v)


def analyze(lines: Iterable[str]) -> Tuple[List[CriticalSection], int, int]:
    enter_cycles: List[int] = []
    exit_cycles: List[int] = []

    for line in lines:
        ev = parse_event(line)
        if ev is None or ev.stage != MCS_STAGE:
            continue
        if ev.tag == TAG_CRITICAL_ENTER:
            enter_cycles.append(ev.cycle)
        elif ev.tag == TAG_CRITICAL_EXIT:
            exit_cycles.append(ev.cycle)

    pair_cnt = min(len(enter_cycles), len(exit_cycles))
    sections = [
        CriticalSection(idx=i, enter_cycle=enter_cycles[i], exit_cycle=exit_cycles[i])
        for i in range(pair_cnt)
        if exit_cycles[i] >= enter_cycles[i]
    ]
    return sections, len(enter_cycles), len(exit_cycles)


def compute_throughput(
    sections: List[CriticalSection],
) -> Tuple[int, Optional[int], Optional[int], Optional[int], Optional[float]]:
    if not sections:
        return 0, None, None, None, None

    total = len(sections)
    first_enter = sections[0].enter_cycle
    last_exit = sections[-1].exit_cycle
    window_cycles = last_exit - first_enter

    if window_cycles <= 0:
        return total, first_enter, last_exit, window_cycles, None

    throughput = (total * CYCLES_PER_MILLION) / float(window_cycles)
    return total, first_enter, last_exit, window_cycles, throughput


def compute_exit_to_next_enter_latency(
    sections: List[CriticalSection],
) -> Tuple[Optional[float], int]:
    if len(sections) < 2:
        return None, 0

    latencies: List[int] = []
    for prev, nxt in zip(sections, sections[1:]):
        if nxt.enter_cycle >= prev.exit_cycle:
            latencies.append(nxt.enter_cycle - prev.exit_cycle)

    return mean(latencies), len(latencies)


def main() -> None:
    args = parse_args()
    with open(args.log, "r", encoding="utf-8", errors="ignore") as f:
        sections, enter_cnt, exit_cnt = analyze(f)

    print(f"log_path: {args.log}")
    print(f"stage_filter: {MCS_STAGE}")
    print(f"tag_filter: enter={TAG_CRITICAL_ENTER}, exit={TAG_CRITICAL_EXIT}")
    print(f"enter_count={enter_cnt}, exit_count={exit_cnt}, paired={len(sections)}")
    print()

    print("==== Critical Section Cycles ====")
    print("idx,enter_cycle,exit_cycle")
    for sec in sections:
        print(f"{sec.idx},{sec.enter_cycle},{sec.exit_cycle}")
    print()

    total, first_enter, last_exit, window, throughput = compute_throughput(sections)
    avg_latency, latency_samples = compute_exit_to_next_enter_latency(sections)

    print("==== Metrics ====")
    print(
        f"throughput(times/1M cycles): {format_num(throughput)}, "
        f"total_critical_sections={total}, "
        f"first_enter_cycle={first_enter if first_enter is not None else 'NA'}, "
        f"last_exit_cycle={last_exit if last_exit is not None else 'NA'}, "
        f"window_cycles={window if window is not None else 'NA'}"
    )
    print(
        f"latency(exit->next enter avg cycles): {format_num(avg_latency)}, "
        f"samples={latency_samples}"
    )


if __name__ == "__main__":
    main()
