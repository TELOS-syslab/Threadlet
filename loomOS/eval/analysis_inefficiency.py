#!/usr/bin/env python3
"""Analyze Figure 7 latency metrics from existing hardware logs."""

from __future__ import annotations

import argparse
import re
from dataclasses import dataclass
from typing import Iterable, Optional

import analysis_scheduling


SCHEDULING_STAGE = 999
MCS_STAGE = 20

TAG_ROUND_START = 100
TAG_WORKER_START = 199
TAG_WORKER_DONE = 200
TAG_ROUND_END = 9999

TAG_UNLOCK_STORE = 2002
TAG_WAIT_WAKE = 2006
TAG_WAIT_DONE = 2007

SCHEDULER_TICK_CYCLES = 32
D_T_SAMPLE_LIMIT = 100

SYN_EVENT_RE = re.compile(
    r"CYCLE:\s*(\d+).*?\[cpu=(\d+)\].*?stage:\s*(\d+),\s*"
    r"addr:\s*0x[0-9a-fA-F]+\s*\(\s*(\d+)\s*\)"
)
RX_FINISHED_RE = re.compile(
    r"CYCLE:\s*(\d+).*?\[State #3\s*\]\s*RX packages finished"
)


@dataclass(frozen=True)
class SynEvent:
    cycle: int
    cpu: int
    stage: int
    tag: int


@dataclass(frozen=True)
class LatencyMetric:
    metric: str
    component: str
    latency_cycles: float
    samples: int


def parse_syn_event(line: str) -> Optional[SynEvent]:
    match = SYN_EVENT_RE.search(line)
    if match is None:
        return None
    return SynEvent(
        cycle=int(match.group(1)),
        cpu=int(match.group(2)),
        stage=int(match.group(3)),
        tag=int(match.group(4)),
    )


def average(values: list[int], description: str) -> float:
    if not values:
        raise ValueError(f"no valid {description} samples")
    return sum(values) / len(values)


def collect_scheduling_rounds(lines: Iterable[str]) -> list[list[SynEvent]]:
    rounds: list[list[SynEvent]] = []
    current: Optional[list[SynEvent]] = None

    for line in lines:
        event = parse_syn_event(line)
        if event is None or event.stage != SCHEDULING_STAGE:
            continue
        if event.tag == TAG_ROUND_START:
            if current is not None:
                raise ValueError("a scheduling round starts before the previous round ends")
            current = [event]
        elif current is not None:
            if event.tag == TAG_ROUND_END:
                rounds.append(current)
                current = None
            else:
                current.append(event)

    if current is not None:
        raise ValueError("the final scheduling round has no round-end marker")
    return rounds


def analyze_scheduling(
    lines: Iterable[str],
) -> tuple[LatencyMetric, LatencyMetric, LatencyMetric]:
    cached_lines = list(lines)
    rounds = collect_scheduling_rounds(cached_lines)
    eight_worker_rounds = [
        events
        for events in rounds
        if sum(event.tag == TAG_WORKER_DONE for event in events) == 8
    ]
    if len(eight_worker_rounds) != 1:
        raise ValueError(
            f"expected one 8-worker scheduling round, found {len(eight_worker_rounds)}"
        )

    worker_events = [
        event
        for event in eight_worker_rounds[0]
        if event.tag in (TAG_WORKER_START, TAG_WORKER_DONE)
    ]
    expected_tags = [
        tag
        for _ in range(8)
        for tag in (TAG_WORKER_START, TAG_WORKER_DONE)
    ]
    if [event.tag for event in worker_events] != expected_tags:
        raise ValueError("the 8-worker start/done markers do not alternate correctly")

    context_switch_samples = [
        next_event.cycle - event.cycle
        for event, next_event in zip(worker_events, worker_events[1:])
        if event.tag == TAG_WORKER_DONE and next_event.tag == TAG_WORKER_START
    ]
    if any(latency < 0 for latency in context_switch_samples):
        raise ValueError("the 8-worker scheduling event cycles are not monotonic")

    scheduling_rows = analysis_scheduling.analyze(cached_lines)
    scheduling_8 = next((row for row in scheduling_rows if row.workers == 8), None)
    if scheduling_8 is None:
        raise ValueError("the scheduling analysis has no 8-worker result")

    context_switch = LatencyMetric(
        metric="context_switch",
        component="threadlet",
        latency_cycles=average(context_switch_samples, "context-switch latency"),
        samples=len(context_switch_samples),
    )
    hardware_scheduling = LatencyMetric(
        metric="scheduling",
        component="hardware",
        latency_cycles=SCHEDULER_TICK_CYCLES / 2,
        samples=1,
    )
    software_scheduling = LatencyMetric(
        metric="scheduling",
        component="software",
        latency_cycles=scheduling_8.scheduler_cycles / scheduling_8.workers,
        samples=scheduling_8.workers,
    )
    return context_switch, hardware_scheduling, software_scheduling


def analyze_thread_to_thread(lines: Iterable[str]) -> LatencyMetric:
    pending_unlock: Optional[int] = None
    wake_seen = False
    latencies: list[int] = []

    for line in lines:
        event = parse_syn_event(line)
        if event is None or event.stage != MCS_STAGE:
            continue
        if event.tag == TAG_ROUND_END:
            break
        if event.tag == TAG_UNLOCK_STORE:
            pending_unlock = event.cycle
            wake_seen = False
        elif event.tag == TAG_WAIT_WAKE and pending_unlock is not None:
            wake_seen = event.cycle >= pending_unlock
        elif event.tag == TAG_WAIT_DONE and pending_unlock is not None and wake_seen:
            if event.cycle >= pending_unlock:
                latencies.append(event.cycle - pending_unlock)
            pending_unlock = None
            wake_seen = False

    return LatencyMetric(
        metric="t_t_notification",
        component="threadlet",
        latency_cycles=average(latencies, "T-T notification latency"),
        samples=len(latencies),
    )


def analyze_device_to_thread(
    lines: Iterable[str], sample_limit: int = D_T_SAMPLE_LIMIT
) -> LatencyMetric:
    if sample_limit <= 0:
        raise ValueError("D-T sample limit must be positive")

    pending_rx: Optional[int] = None
    latencies: list[int] = []

    for line in lines:
        rx_match = RX_FINISHED_RE.search(line)
        if rx_match is not None:
            pending_rx = int(rx_match.group(1))
            continue

        event = parse_syn_event(line)
        if (
            pending_rx is None
            or event is None
            or event.stage != 7
            or event.tag != 3
        ):
            continue
        if event.cycle >= pending_rx:
            latencies.append(event.cycle - pending_rx)
        pending_rx = None
        if len(latencies) == sample_limit:
            break

    if len(latencies) != sample_limit:
        raise ValueError(
            f"expected {sample_limit} D-T notification samples, found {len(latencies)}"
        )
    return LatencyMetric(
        metric="d_t_notification",
        component="threadlet",
        latency_cycles=average(latencies, "D-T notification latency"),
        samples=len(latencies),
    )


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("figure8_log", help="Figure 8 hardware log path")
    parser.add_argument("figure13_log", help="Figure 13 hardware log path")
    parser.add_argument("figure10_log", help="Figure 10 hardware log path")
    return parser.parse_args()


def read_lines(path: str) -> list[str]:
    with open(path, "r", encoding="utf-8", errors="ignore") as log_file:
        return list(log_file)


def main() -> None:
    args = parse_args()
    try:
        metrics = [
            *analyze_scheduling(read_lines(args.figure8_log)),
            analyze_thread_to_thread(read_lines(args.figure13_log)),
            analyze_device_to_thread(read_lines(args.figure10_log)),
        ]
    except (OSError, ValueError) as error:
        raise SystemExit(f"[error] {error}") from error

    print("metric,component,latency_cycles,samples")
    for metric in metrics:
        print(
            f"{metric.metric},{metric.component},"
            f"{metric.latency_cycles:.2f},{metric.samples}"
        )


if __name__ == "__main__":
    main()
