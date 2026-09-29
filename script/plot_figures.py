#!/usr/bin/env python3
"""Plot Threadlet AE result CSV files."""

from __future__ import annotations

import argparse
import csv
import math
from dataclasses import dataclass
from pathlib import Path

import matplotlib

matplotlib.use("Agg")
import matplotlib.pyplot as plt  # noqa: E402
from matplotlib.figure import Figure  # noqa: E402


FIGURE_COLUMNS = {
    7: {"metric", "component", "latency_cycles", "samples"},
    8: {
        "workers",
        "total_cycles",
        "scheduler_cycles",
        "worker_throughput_ktps",
        "scheduling_overhead_percent",
    },
    10: {
        "subplot",
        "group",
        "config",
        "requests",
        "matched",
        "p99_slowdown",
        "p99_tail_latency_cycles",
        "throughput_pkt_per_ms",
    },
    13: {"threads_per_cpu", "latency_cycles", "throughput_per_1m_cycles"},
}

LEGACY_COLUMNS = {
    7: {"metric", "time"},
    8: {"threads", "linux_sched", "linux_cs", "throughput"},
    10: {"p99slowdown", "p99latency", "throughput"},
}

FIGURE8_BASELINE_COLUMNS = {
    "threads",
    "tasks",
    "total_cycles",
    "scheduler_cycles",
    "throughput_ktps",
    "overhead_percent",
}

FIGURE10_LEGACY_SUBPLOTS = {
    "bi": "figure10a_bimodal",
    "ht": "figure10b_heavy",
    "kv": "figure10c_kv",
}

FIGURE7_BASELINE_CYCLES_PER_TICK = 1000.0

SYSTEM_LABELS = {
    "threadlet": "LoomOS",
    "linux": "Linux",
    "linux_poll": "Linux_poll",
    "shinjuku": "Shinjuku",
    "nanopu": "NanoPU",
    "ipi": "IPI",
    "polling": "Polling",
    "interrupt": "Interrupt",
    "mutex": "Mutex",
    "argobots": "Argobots",
    "mcs-b": "mcs-b",
    "baseline": "Baseline",
}


@dataclass(frozen=True)
class SystemRows:
    name: str
    label: str
    rows: list[dict[str, str]]


def display_name(stem: str) -> str:
    return SYSTEM_LABELS.get(stem, stem.replace("_", " ").title())


def load_system_rows(result_dir: Path, required: set[str]) -> list[SystemRows]:
    if not result_dir.is_dir():
        raise ValueError(f"result directory not found: {result_dir}")

    paths = sorted(
        result_dir.glob("*.csv"),
        key=lambda path: (path.stem != "threadlet", path.stem),
    )
    if not any(path.stem == "threadlet" for path in paths):
        raise ValueError(f"threadlet.csv not found in {result_dir}")

    systems: list[SystemRows] = []
    for path in paths:
        with path.open("r", encoding="utf-8", newline="") as csv_file:
            reader = csv.DictReader(csv_file)
            columns = set(reader.fieldnames or [])
            missing = required - columns
            if missing:
                raise ValueError(
                    f"{path} is missing columns: {', '.join(sorted(missing))}"
                )
            rows = list(reader)
        if not rows:
            raise ValueError(f"{path} has no data rows")
        systems.append(
            SystemRows(
                name=path.stem,
                label=display_name(path.stem),
                rows=rows,
            )
        )
    return systems


def normalized_number(value: float) -> str:
    return f"{value:.12g}"


def normalize_legacy_rows(
    figure: int,
    path: Path,
    columns: set[str],
    rows: list[dict[str, str]],
) -> tuple[str, list[dict[str, str]]]:
    required = FIGURE_COLUMNS[figure]
    if figure == 8 and columns == FIGURE8_BASELINE_COLUMNS:
        normalized = [
            {
                "workers": row["threads"],
                "total_cycles": row["total_cycles"],
                "scheduler_cycles": row["scheduler_cycles"],
                "worker_throughput_ktps": row["throughput_ktps"],
                "scheduling_overhead_percent": row["overhead_percent"],
            }
            for row in rows
        ]
        return "linux", normalized

    if required <= columns:
        if figure == 8 and path.stem == "baseline":
            return "linux", rows
        return path.stem, rows

    if columns != LEGACY_COLUMNS.get(figure):
        missing = required - columns
        raise ValueError(
            f"{path} is missing columns: {', '.join(sorted(missing))}"
        )

    if figure == 7:
        normalized = []
        for row in rows:
            latency = numeric(row, "time", path.stem)
            if path.stem == "baseline" and row["metric"] == "t_t_notification":
                # The legacy IPI benchmark reports shared 1 MHz time ticks.
                latency *= FIGURE7_BASELINE_CYCLES_PER_TICK
            normalized.append(
                {
                    "metric": row["metric"],
                    "component": "software",
                    "latency_cycles": normalized_number(latency),
                    "samples": "NA",
                }
            )
        return path.stem, normalized

    if figure == 8:
        raise ValueError(
            f"{path}: invalid legacy throughput; rebuild benchmark/task_over "
            "and rerun the Figure 8 Linux baseline"
        )

    prefix, separator, suffix = path.stem.rpartition("-")
    if not separator or suffix not in FIGURE10_LEGACY_SUBPLOTS:
        raise ValueError(f"{path}: cannot infer the Figure 10 subplot")
    system_name = "linux_poll" if prefix == "linux" else prefix
    normalized = []
    for index, row in enumerate(rows, start=1):
        normalized.append(
            {
                "subplot": FIGURE10_LEGACY_SUBPLOTS[suffix],
                "group": f"{suffix}_{index}",
                "config": "legacy_baseline",
                "requests": "NA",
                "matched": "NA",
                "p99_slowdown": row["p99slowdown"],
                "p99_tail_latency_cycles": normalized_number(
                    numeric(row, "p99latency", path.stem) * 1000.0
                ),
                "throughput_pkt_per_ms": normalized_number(
                    numeric(row, "throughput", path.stem) / 1000.0
                ),
            }
        )
    return system_name, normalized


def load_figure_rows(result_dir: Path, figure: int) -> list[SystemRows]:
    if figure not in FIGURE_COLUMNS:
        raise ValueError(f"unsupported figure: {figure}")
    if not result_dir.is_dir():
        raise ValueError(f"result directory not found: {result_dir}")

    if figure == 13:
        names = ("threadlet", "mutex", "argobots", "mcs-b")
        paths = [result_dir / f"{name}.csv" for name in names]
        missing = [path.name for path in paths if not path.is_file()]
        if missing:
            raise ValueError(f"Figure 13 requires all four systems; missing: {', '.join(missing)}")
    else:
        paths = sorted(
            result_dir.glob("*.csv"),
            key=lambda path: (path.stem != "threadlet", path.stem),
        )
        if not any(path.stem == "threadlet" for path in paths):
            raise ValueError(f"threadlet.csv not found in {result_dir}")

    systems: list[SystemRows] = []
    for path in paths:
        with path.open("r", encoding="utf-8", newline="") as csv_file:
            reader = csv.DictReader(csv_file)
            columns = set(reader.fieldnames or [])
            rows = list(reader)
        if not rows:
            raise ValueError(f"{path} has no data rows")
        name, rows = normalize_legacy_rows(figure, path, columns, rows)
        systems.append(SystemRows(name=name, label=display_name(name), rows=rows))
    return systems


def numeric(row: dict[str, str], field: str, system: str) -> float:
    value = row.get(field, "")
    try:
        result = float(value)
    except (TypeError, ValueError) as error:
        raise ValueError(f"{system}: invalid {field} value {value!r}") from error
    if not math.isfinite(result):
        raise ValueError(f"{system}: non-finite {field} value {value!r}")
    return result


def style_axis(axis, *, grid: bool = True) -> None:
    if grid:
        axis.grid(axis="y", linestyle=":", linewidth=0.7, alpha=0.65)
        axis.set_axisbelow(True)
    axis.spines["top"].set_visible(False)
    axis.spines["right"].set_visible(False)


def annotate_bars(axis, containers, cap: float | None = None) -> None:
    for container in containers:
        for bar in container:
            height = bar.get_height()
            if height <= 0:
                continue
            endpoint = bar.get_y() + height
            clipped = cap is not None and endpoint > cap
            label_y = cap if clipped else endpoint
            axis.annotate(
                f"{height:.1f}",
                xy=(bar.get_x() + bar.get_width() / 2, label_y),
                xytext=(0, -2 if clipped else 2),
                textcoords="offset points",
                ha="center",
                va="top" if clipped else "bottom",
                fontsize=8,
            )


def rows_for_metric(system: SystemRows, metric: str) -> list[dict[str, str]]:
    return [row for row in system.rows if row.get("metric") == metric]


def plot_metric_bars(
    axis, systems: list[SystemRows], metric: str, cap: float
) -> None:
    labels: list[str] = []
    values: list[float] = []
    for system in systems:
        rows = rows_for_metric(system, metric)
        if not rows:
            continue
        if len(rows) != 1:
            raise ValueError(f"{system.name}: expected one {metric} row")
        labels.append(system.label)
        values.append(numeric(rows[0], "latency_cycles", system.name))

    if not values:
        axis.text(0.5, 0.5, "No data", ha="center", va="center", transform=axis.transAxes)
        axis.set_xticks([])
        return
    container = axis.bar(range(len(values)), values, width=0.55)
    axis.set_xticks(range(len(labels)), labels)
    annotate_bars(axis, [container], cap=cap)


def plot_figure7(systems: list[SystemRows]) -> Figure:
    figure, raw_axes = plt.subplots(2, 2, figsize=(8.0, 6.5))
    axes = tuple(raw_axes.flat)

    panels = (
        (axes[0], "context_switch", "(a) Context Switch", (0, 800), range(0, 801, 200)),
        (axes[2], "t_t_notification", "(c) T-T Notification", (0, 400), range(0, 401, 100)),
        (axes[3], "d_t_notification", "(d) D-T Notification", (0, 400), range(0, 401, 100)),
    )
    for axis, metric, title, limits, ticks in panels:
        plot_metric_bars(axis, systems, metric, cap=limits[1])
        axis.set_title(title)
        axis.set_ylabel("Time (cycles)")
        axis.set_ylim(*limits)
        axis.set_yticks(list(ticks))
        style_axis(axis)

    scheduling_systems = [
        system for system in systems if rows_for_metric(system, "scheduling")
    ]
    scheduling_axis = axes[1]
    if scheduling_systems:
        labels = [system.label for system in scheduling_systems]
        software: list[float] = []
        hardware: list[float] = []
        for system in scheduling_systems:
            components = {
                row.get("component", ""): numeric(row, "latency_cycles", system.name)
                for row in rows_for_metric(system, "scheduling")
            }
            software.append(components.get("software", 0.0))
            hardware.append(components.get("hardware", 0.0))
        positions = list(range(len(labels)))
        software_bars = scheduling_axis.bar(
            positions, software, width=0.55, label="Software"
        )
        hardware_bars = scheduling_axis.bar(
            positions,
            hardware,
            width=0.55,
            bottom=software,
            label="Hardware",
        )
        scheduling_axis.set_xticks(positions, labels)
        annotate_bars(scheduling_axis, [software_bars, hardware_bars], cap=400)
        scheduling_axis.legend(frameon=False, fontsize=8)
    else:
        scheduling_axis.text(
            0.5, 0.5, "No data", ha="center", va="center", transform=scheduling_axis.transAxes
        )
        scheduling_axis.set_xticks([])
    scheduling_axis.set_title("(b) Scheduling Overhead")
    scheduling_axis.set_ylabel("Time (cycles)")
    scheduling_axis.set_ylim(0, 400)
    scheduling_axis.set_yticks(range(0, 401, 100))
    style_axis(scheduling_axis)

    figure.tight_layout()
    return figure


def worker_rows(system: SystemRows) -> dict[int, dict[str, str]]:
    result: dict[int, dict[str, str]] = {}
    for row in system.rows:
        workers = int(numeric(row, "workers", system.name))
        result[workers] = row
    return result


def plot_figure8(systems: list[SystemRows]) -> Figure:
    figure, axes = plt.subplots(1, 2, figsize=(8.0, 3.4))
    workers = (2, 4, 8)
    positions = list(range(len(workers)))
    width = 0.7 / max(2, len(systems))

    bar_containers = []
    for system_index, system in enumerate(systems):
        by_worker = worker_rows(system)
        offset = (system_index - (len(systems) - 1) / 2) * width
        present = [(index, worker) for index, worker in enumerate(workers) if worker in by_worker]
        x_values = [index + offset for index, _ in present]
        overhead = [
            numeric(by_worker[worker], "scheduling_overhead_percent", system.name)
            for _, worker in present
        ]
        bars = axes[0].bar(x_values, overhead, width=width, label=system.label)
        bar_containers.append(bars)

        throughput = [
            numeric(by_worker[worker], "worker_throughput_ktps", system.name)
            for _, worker in present
        ]
        axes[1].bar(
            x_values,
            throughput,
            width=width,
            label=system.label,
        )

    annotate_bars(axes[0], bar_containers, cap=40)
    axes[0].set_title("(a) Scheduling Overhead")
    axes[0].set_ylabel("Overhead (%)")
    axes[0].set_ylim(0, 40)
    axes[0].set_yticks(range(0, 41, 10))
    axes[1].set_title("(b) Throughput")
    axes[1].set_ylabel("Throughput (KTPS)")
    axes[1].set_ylim(60, 100)
    axes[1].set_yticks(range(60, 101, 10))

    for axis in axes:
        axis.set_xlabel("#Threads")
        axis.set_xticks(positions, [str(worker) for worker in workers])
        axis.legend(frameon=False, fontsize=8)
        style_axis(axis)
    axes[1].set_xlim(-0.45, 2.9)
    figure.tight_layout()
    return figure


def plot_figure10(systems: list[SystemRows]) -> Figure:
    figure, axes = plt.subplots(1, 3, figsize=(12.0, 3.5))
    panels = (
        ("figure10a_bimodal", "(a) Bimodal", "p99_slowdown", "p99 slowdown", (0, 60)),
        ("figure10b_heavy", "(b) Heavy-tailed", "p99_slowdown", "p99 slowdown", (0, 1000)),
        (
            "figure10c_kv",
            "(c) 99.5% GET, 0.5% SCAN",
            "p99_tail_latency_cycles",
            "p99 tail latency (μs)",
            (0, 1000),
        ),
    )

    for axis, (subplot, title, value_field, ylabel, xlimits) in zip(axes, panels):
        plotted = False
        for system in systems:
            selected = [row for row in system.rows if row.get("subplot") == subplot]
            points = sorted(
                (
                    numeric(row, "throughput_pkt_per_ms", system.name),
                    numeric(row, value_field, system.name),
                )
                for row in selected
            )
            if not points:
                continue
            x_values = [point[0] for point in points]
            scale = 1000.0 if value_field == "p99_tail_latency_cycles" else 1.0
            y_values = [point[1] / scale for point in points]
            axis.plot(
                x_values,
                y_values,
                marker="o",
                linewidth=1.8,
                label=system.label,
            )
            plotted = True
        if not plotted:
            axis.text(0.5, 0.5, "No data", ha="center", va="center", transform=axis.transAxes)
        axis.set_title(title)
        axis.set_xlabel("Throughput (KRPS)")
        axis.set_ylabel(ylabel)
        axis.set_xlim(*xlimits)
        axis.set_ylim(0, 100)
        axis.set_yticks(range(0, 101, 25))
        if xlimits[1] == 60:
            axis.set_xticks(range(0, 61, 15))
        else:
            axis.set_xticks(range(0, 1001, 250))
        if plotted:
            axis.legend(frameon=False, fontsize=8)
        style_axis(axis)

    figure.tight_layout()
    return figure


def thread_count_rows(system: SystemRows) -> dict[int, dict[str, str]]:
    result: dict[int, dict[str, str]] = {}
    for row in system.rows:
        threads = numeric(row, "threads_per_cpu", system.name)
        if threads not in (1, 2, 4, 8):
            raise ValueError(f"{system.name}: expected threads_per_cpu in 1, 2, 4, 8")
        total_threads = int(threads) * 4
        if total_threads in result:
            raise ValueError(f"{system.name}: duplicate threads_per_cpu={threads:g}")
        if numeric(row, "latency_cycles", system.name) < 0 or numeric(row, "throughput_per_1m_cycles", system.name) <= 0:
            raise ValueError(f"{system.name}: invalid lock metrics")
        result[total_threads] = row
    if set(result) != {4, 8, 16, 32}:
        raise ValueError(f"{system.name}: expected all four thread configurations")
    return result


def plot_figure13(systems: list[SystemRows]) -> Figure:
    figure, axes = plt.subplots(1, 2, figsize=(8.0, 3.4))
    thread_counts = (4, 8, 16, 32)

    markers = ("o", "s", "^", "D")
    for system_index, system in enumerate(systems):
        by_threads = thread_count_rows(system)
        present = [
            (index, threads)
            for index, threads in enumerate(thread_counts)
            if threads in by_threads
        ]
        positions = [float(index) for index, _ in present]
        throughput = [
            numeric(by_threads[threads], "throughput_per_1m_cycles", system.name)
            for _, threads in present
        ]
        latency = [
            numeric(by_threads[threads], "latency_cycles", system.name)
            for _, threads in present
        ]
        axes[0].plot(positions, throughput, marker=markers[system_index % len(markers)], linewidth=1.8, label=system.label)
        axes[1].plot(positions, latency, marker=markers[system_index % len(markers)], linewidth=1.8, label=system.label)

    axes[0].set_title("(a) Throughput")
    axes[0].set_ylabel("Throughput (Kops/s)")
    peak_throughput = max(max(line.get_ydata()) for line in axes[0].lines)
    axes[0].set_ylim(0, 100 if peak_throughput <= 100 else peak_throughput * 1.1)
    axes[0].set_yticks(range(0, 101, 25))
    axes[1].set_title("(b) Average Latency")
    axes[1].set_ylabel("Average latency (cycles)")
    peak_latency = max(max(line.get_ydata()) for line in axes[1].lines)
    axes[1].set_ylim(0, 12000 if peak_latency <= 12000 else peak_latency * 1.1)
    axes[1].set_yticks(range(0, 12001, 3000))

    positions = list(range(len(thread_counts)))
    for axis in axes:
        axis.set_xlabel("#Threads")
        axis.set_xlim(-0.5, 3.5)
        axis.set_xticks(positions, [str(count) for count in thread_counts])
        axis.legend(frameon=False, fontsize=8)
        style_axis(axis)
    figure.tight_layout()
    return figure


PLOTTERS = {
    7: plot_figure7,
    8: plot_figure8,
    10: plot_figure10,
    13: plot_figure13,
}


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("figure", type=int, choices=sorted(PLOTTERS))
    parser.add_argument("result_dir", type=Path)
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    try:
        systems = load_figure_rows(args.result_dir, args.figure)
        figure = PLOTTERS[args.figure](systems)
        output = args.result_dir / f"figure{args.figure}.png"
        figure.savefig(output, dpi=200, bbox_inches="tight")
        plt.close(figure)
    except (OSError, ValueError) as error:
        raise SystemExit(f"[error] {error}") from error
    print(f"Generated {output}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
