#!/usr/bin/env python3
"""
Analyze RPC latency from dispatcher send to worker finish.

Per-packet path:
1) send:   threadlet_syn_print(RPC_SEND_STAGE, request_id)
2) finish: threadlet_syn_print(RPC_FINISH_STAGE, request_id)
"""

from __future__ import annotations

import argparse
import math
import re
from collections import defaultdict
from dataclasses import dataclass
from pathlib import Path
from typing import Dict, Iterable, List, Optional, Tuple


CYCLES_PER_MS = 1_000_000
DEFAULT_GROUP_GAP_CYCLES = 10_000_000


@dataclass(frozen=True)
class RpcEvent:
    cycle: int
    cpu: int
    request_id: int


@dataclass(frozen=True)
class CntSample:
    cycle: int
    value: int


@dataclass
class PacketRecord:
    request_id: int
    send_cycle: Optional[int]
    finish_cycle: Optional[int]

    @property
    def send_to_finish(self) -> Optional[int]:
        if self.send_cycle is None or self.finish_cycle is None:
            return None
        return self.finish_cycle - self.send_cycle

    @property
    def status(self) -> str:
        if self.send_cycle is None and self.finish_cycle is None:
            return "missing_send,missing_finish"
        if self.send_cycle is None:
            return "missing_send"
        if self.finish_cycle is None:
            return "missing_finish"
        return "ok"


@dataclass
class PacketRecordGroup:
    start_send_index: int
    end_send_index: int
    records: List[PacketRecord]


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Analyze RPC latency from send stage to finish stage."
    )
    parser.add_argument(
        "log",
        nargs="?",
        default="hw.log",
        help="Path to hardware log (default: hw.log)",
    )
    parser.add_argument(
        "--send-stage",
        type=int,
        default=87,
        help="Stage number for RPC send print (default: 87)",
    )
    parser.add_argument(
        "--finish-stage",
        type=int,
        default=9,
        help="Stage number for worker finish print (default: 9)",
    )
    parser.add_argument(
        "--cnt-stage",
        type=int,
        default=21,
        help="Stage number for per-worker-core cnt print (default: 21)",
    )
    parser.add_argument(
        "--max-print",
        type=int,
        default=None,
        help="Print at most N packet rows (default: all).",
    )
    parser.add_argument(
        "--group-size",
        "--packets-per-group",
        dest="packets_per_group",
        type=int,
        default=None,
        help=(
            "Number of packet send events per report group. If omitted, split "
            "groups by --group-gap-cycles."
        ),
    )
    parser.add_argument(
        "--group-gap-cycles",
        type=int,
        default=DEFAULT_GROUP_GAP_CYCLES,
        help=(
            "Start a new report group when adjacent send events differ by more "
            f"than this many cycles (default: {DEFAULT_GROUP_GAP_CYCLES})."
        ),
    )
    parser.add_argument(
        "--rpc-stage",
        type=int,
        default=None,
        help="Deprecated and ignored (kept for compatibility).",
    )
    parser.add_argument(
        "--dispatch-tag",
        type=int,
        default=None,
        help="Deprecated and ignored (kept for compatibility).",
    )
    parser.add_argument(
        "--dequeue-tag",
        type=int,
        default=None,
        help="Deprecated and ignored (kept for compatibility).",
    )
    parser.add_argument(
        "--start-id",
        type=int,
        default=None,
        help="Deprecated and ignored (kept for compatibility).",
    )
    args = parser.parse_args()
    if args.packets_per_group is not None and args.packets_per_group <= 0:
        parser.error("--group-size must be greater than 0")
    if args.group_gap_cycles <= 0:
        parser.error("--group-gap-cycles must be greater than 0")
    return args


def pctl(values: List[int], pct: float) -> Optional[int]:
    if not values:
        return None
    s = sorted(values)
    rank = max(0, math.ceil(len(s) * pct) - 1)
    return s[rank]


def mean(values: List[int]) -> Optional[float]:
    if not values:
        return None
    return float(sum(values)) / float(len(values))


def format_num(v: Optional[float]) -> str:
    if v is None:
        return "NA"
    if isinstance(v, float):
        return f"{v:.2f}"
    return str(v)


def format_latency_stats(name: str, pairs: List[Tuple[int, int]]) -> str:
    values = [v for _, v in pairs]
    base = (
        f"{name}"
        f"count={len(values)}, "
        f"avg={format_num(mean(values))}, "
        f"p90={format_num(pctl(values, 0.90))}, "
        f"p99={format_num(pctl(values, 0.99))}"
    )
    if len(values) == 1:
        pkt_id, lat = pairs[0]
        return base + f", packet_id={pkt_id}, latency={lat}"
    return base


def compute_throughput(
    records: List[PacketRecord],
) -> Tuple[int, Optional[int], Optional[int], Optional[int], Optional[float]]:
    sent = [r for r in records if r.send_cycle is not None]
    matched = [r for r in records if r.send_cycle is not None and r.finish_cycle is not None]
    if not sent or not matched:
        return 0, None, None, None, None

    first_send = min(r.send_cycle for r in sent if r.send_cycle is not None)
    last_finish = max(r.finish_cycle for r in matched if r.finish_cycle is not None)
    window_cycles = last_finish - first_send
    packet_count = len(matched)
    if window_cycles <= 0:
        return packet_count, first_send, last_finish, window_cycles, None
    throughput = (packet_count * CYCLES_PER_MS) / float(window_cycles)
    return packet_count, first_send, last_finish, window_cycles, throughput


def parse_log(
    lines: Iterable[str],
    send_stage: int,
    finish_stage: int,
    cnt_stage: int,
) -> Tuple[Dict[int, List[int]], Dict[int, List[int]], Dict[int, List[int]]]:
    send_events, finish_events, cnt_samples_by_cpu = parse_log_events(
        lines,
        send_stage,
        finish_stage,
        cnt_stage,
    )

    send_cycles_by_id: Dict[int, List[int]] = defaultdict(list)
    finish_cycles_by_id: Dict[int, List[int]] = defaultdict(list)
    cnt_values_by_cpu: Dict[int, List[int]] = defaultdict(list)

    for event in send_events:
        send_cycles_by_id[event.request_id].append(event.cycle)
    for event in finish_events:
        finish_cycles_by_id[event.request_id].append(event.cycle)
    for cpu, samples in cnt_samples_by_cpu.items():
        cnt_values_by_cpu[cpu].extend(sample.value for sample in samples)

    return send_cycles_by_id, finish_cycles_by_id, cnt_values_by_cpu


def parse_log_events(
    lines: Iterable[str],
    send_stage: int,
    finish_stage: int,
    cnt_stage: int,
) -> Tuple[List[RpcEvent], List[RpcEvent], Dict[int, List[CntSample]]]:
    # Example:
    # CYCLE:   1081748394 [State #9.3][cpu=0] stage: 87, addr: 0x... (123)
    stage_re = re.compile(
        r"CYCLE:\s*(\d+).*?\[cpu=(\d+)\].*?stage:\s*([0-9]+),\s*addr:\s*0x[0-9a-fA-F]+\s*\(\s*([0-9]+)\s*\)"
    )

    send_events: List[RpcEvent] = []
    finish_events: List[RpcEvent] = []
    cnt_samples_by_cpu: Dict[int, List[CntSample]] = defaultdict(list)

    for line in lines:
        sm = stage_re.search(line)
        if not sm:
            continue

        cycle = int(sm.group(1))
        cpu = int(sm.group(2))
        stage = int(sm.group(3))
        addr_dec = int(sm.group(4))

        if stage == send_stage:
            send_events.append(RpcEvent(cycle=cycle, cpu=cpu, request_id=addr_dec))
        elif stage == finish_stage:
            finish_events.append(RpcEvent(cycle=cycle, cpu=cpu, request_id=addr_dec))
        elif stage == cnt_stage and cpu > 0:
            cnt_samples_by_cpu[cpu].append(CntSample(cycle=cycle, value=addr_dec))

    return send_events, finish_events, cnt_samples_by_cpu


def print_worker_cnt_throughput(
    cnt_samples_by_cpu: Dict[int, List[CntSample]],
    start_cycle: Optional[int] = None,
    end_cycle: Optional[int] = None,
) -> None:
    print("---- Worker Background Throughput (cnt delta) ----")
    if not cnt_samples_by_cpu:
        print("none")
        print()
        return

    print("cpu,first_cnt,last_cnt,cnt_delta,samples")
    for cpu in sorted(cnt_samples_by_cpu.keys()):
        samples = [
            sample.value
            for sample in cnt_samples_by_cpu[cpu]
            if (start_cycle is None or sample.cycle >= start_cycle)
            and (end_cycle is None or sample.cycle <= end_cycle)
        ]
        if not samples:
            continue
        first_cnt = samples[0]
        last_cnt = samples[-1]
        cnt_delta = last_cnt - first_cnt
        print(f"{cpu},{first_cnt},{last_cnt},{cnt_delta},{len(samples)}")
    print()


def write_per_cpu_rpc_logs(lines: Iterable[str], input_log: str) -> List[Path]:
    monitor_re = re.compile(r"\[ThreadManager\]\[monitor\]\[cpu=(\d+)\]")
    state_re = re.compile(r"\[State #9\.3\]\[cpu=(\d+)\]")
    sched_switch_re = re.compile(r"\[ThreadManager\]\[sched\] switch hart=(\d+)")

    per_cpu_lines: Dict[int, List[str]] = defaultdict(list)
    for line in lines:
        m = monitor_re.search(line)
        if m:
            cpu = int(m.group(1))
            if cpu > 0:
                per_cpu_lines[cpu].append(line)
            continue

        m = state_re.search(line)
        if m:
            cpu = int(m.group(1))
            if cpu > 0:
                per_cpu_lines[cpu].append(line)
            continue

        m = sched_switch_re.search(line)
        if m:
            cpu = int(m.group(1))
            if cpu > 0:
                per_cpu_lines[cpu].append(line)

    out_dir = Path(input_log).resolve().parent
    outputs: List[Path] = []
    for cpu in sorted(per_cpu_lines.keys()):
        out_path = out_dir / f"hw_rpc_{cpu}.log"
        with out_path.open("w", encoding="utf-8") as f:
            f.writelines(per_cpu_lines[cpu])
        outputs.append(out_path)
    return outputs


def build_records(
    send_cycles_by_id: Dict[int, List[int]],
    finish_cycles_by_id: Dict[int, List[int]],
) -> List[PacketRecord]:
    ids = sorted(set(send_cycles_by_id.keys()) | set(finish_cycles_by_id.keys()))
    records: List[PacketRecord] = []

    for req_id in ids:
        send_list = send_cycles_by_id.get(req_id, [])
        finish_list = finish_cycles_by_id.get(req_id, [])

        send_cycle = send_list[0] if send_list else None
        finish_cycle: Optional[int] = None

        if finish_list:
            if send_cycle is None:
                finish_cycle = finish_list[0]
            else:
                for cand in finish_list:
                    if cand >= send_cycle:
                        finish_cycle = cand
                        break

        records.append(
            PacketRecord(
                request_id=req_id,
                send_cycle=send_cycle,
                finish_cycle=finish_cycle,
            )
        )

    return records


def build_record_groups(
    send_events: List[RpcEvent],
    finish_events: List[RpcEvent],
    packets_per_group: Optional[int],
    group_gap_cycles: int,
) -> List[PacketRecordGroup]:
    if not send_events:
        return []

    finish_events_by_id: Dict[int, List[RpcEvent]] = defaultdict(list)
    for event in finish_events:
        finish_events_by_id[event.request_id].append(event)

    next_finish_index: Dict[int, int] = defaultdict(int)
    groups: List[PacketRecordGroup] = []

    if packets_per_group is not None:
        send_groups = [
            (group_start, send_events[group_start : group_start + packets_per_group])
            for group_start in range(0, len(send_events), packets_per_group)
        ]
    else:
        send_groups: List[Tuple[int, List[RpcEvent]]] = []
        group_start = 0
        prev_cycle = send_events[0].cycle
        for idx, event in enumerate(send_events[1:], start=1):
            if event.cycle - prev_cycle > group_gap_cycles:
                send_groups.append((group_start, send_events[group_start:idx]))
                group_start = idx
            prev_cycle = event.cycle
        send_groups.append((group_start, send_events[group_start:]))

    for group_start, group_sends in send_groups:
        records: List[PacketRecord] = []

        for send_event in group_sends:
            finish_list = finish_events_by_id.get(send_event.request_id, [])
            finish_index = next_finish_index[send_event.request_id]

            while (
                finish_index < len(finish_list)
                and finish_list[finish_index].cycle < send_event.cycle
            ):
                finish_index += 1

            finish_cycle: Optional[int] = None
            if finish_index < len(finish_list):
                finish_cycle = finish_list[finish_index].cycle
                finish_index += 1

            next_finish_index[send_event.request_id] = finish_index
            records.append(
                PacketRecord(
                    request_id=send_event.request_id,
                    send_cycle=send_event.cycle,
                    finish_cycle=finish_cycle,
                )
            )

        groups.append(
            PacketRecordGroup(
                start_send_index=group_start + 1,
                end_send_index=group_start + len(group_sends),
                records=records,
            )
        )

    return groups


def print_report(
    records: List[PacketRecord],
    max_print: Optional[int],
    title: str = "==== RPC Latency Analysis ====",
) -> None:
    send_to_finish_pairs = [
        (r.request_id, v) for r in records if (v := r.send_to_finish) is not None
    ]

    ok_cnt = sum(1 for r in records if r.status == "ok")
    miss_send_cnt = sum(1 for r in records if "missing_send" in r.status)
    miss_finish_cnt = sum(1 for r in records if "missing_finish" in r.status)

    print(title)
    print(f"total_ids: {len(records)}")
    print(f"fully_matched: {ok_cnt}")
    print(f"missing_send: {miss_send_cnt}")
    print(f"missing_finish: {miss_finish_cnt}")
    print()

    print("---- Latency (cycles) ----")
    print(format_latency_stats("send->finish: ", send_to_finish_pairs))
    print()

    pkt_cnt, first_send, last_fin, window, throughput = compute_throughput(records)
    print("---- Throughput ----")
    print(
        f"throughput(pkt/ms): {format_num(throughput)}, "
        f"packet_count={pkt_cnt}, "
        f"first_send_cycle={first_send if first_send is not None else 'NA'}, "
        f"last_finish_cycle={last_fin if last_fin is not None else 'NA'}, "
        f"window_cycles={window if window is not None else 'NA'}"
    )
    print()

    print("id,send_cycle,finish_cycle,send_to_finish,status")
    rows = records if max_print is None else records[: max_print]
    for r in rows:
        print(
            f"{r.request_id},"
            f"{r.send_cycle if r.send_cycle is not None else 'NA'},"
            f"{r.finish_cycle if r.finish_cycle is not None else 'NA'},"
            f"{r.send_to_finish if r.send_to_finish is not None else 'NA'},"
            f"{r.status}"
        )


def main() -> None:
    args = parse_args()

    with open(args.log, "r", encoding="utf-8", errors="ignore") as f:
        lines = f.readlines()
        send_events, finish_events, cnt_samples_by_cpu = parse_log_events(
            lines,
            args.send_stage,
            args.finish_stage,
            args.cnt_stage,
        )

    output_logs = write_per_cpu_rpc_logs(lines, args.log)
    record_groups = build_record_groups(
        send_events,
        finish_events,
        args.packets_per_group,
        args.group_gap_cycles,
    )

    print(f"log_path: {args.log}")
    print(
        f"parse_config: send_stage={args.send_stage}, finish_stage={args.finish_stage}, "
        f"cnt_stage={args.cnt_stage}"
    )
    if args.packets_per_group is not None:
        print(f"group_config: packets_per_group={args.packets_per_group}")
    else:
        print(f"group_config: gap_cycles={args.group_gap_cycles}")
    print(
        f"raw_events: send_events={len(send_events)}, "
        f"finish_events={len(finish_events)}"
    )
    if output_logs:
        print("cpu_rpc_logs: " + ", ".join(str(p) for p in output_logs))
    else:
        print("cpu_rpc_logs: none (no cpu id > 0 matched)")
    print()

    if not record_groups:
        print_worker_cnt_throughput(cnt_samples_by_cpu)
        print_report([], args.max_print)
        return

    total_groups = len(record_groups)
    for group_idx, group in enumerate(record_groups, start=1):
        records = group.records
        _, first_send, last_finish, _, _ = compute_throughput(records)

        print(
            f"==== RPC Group {group_idx}/{total_groups} "
            f"(send_events {group.start_send_index}-{group.end_send_index}) ===="
        )
        print_worker_cnt_throughput(cnt_samples_by_cpu, first_send, last_finish)
        print_report(
            records,
            args.max_print,
            title=f"==== RPC Latency Analysis (group {group_idx}/{total_groups}) ====",
        )
        if group_idx != total_groups:
            print()


if __name__ == "__main__":
    main()
