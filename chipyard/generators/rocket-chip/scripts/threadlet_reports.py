import argparse
import csv
import re
import sys
from pathlib import Path

SECTIONS = [
    {
        "name": "Full Chip",
        "util_patterns": [
            "utilization_post_route.rpt",
            "*utilization*post_route*.rpt",
            "utilization_post_synth.rpt",
            "*utilization*post_synth*.rpt",
            "*utilization*.rpt",
        ],
        "power_patterns": [
            "power_post_route_vectorless.rpt",
            "*power*post_route*vectorless*.rpt",
            "*power*post_route*.rpt",
            "power_post_synth_vectorless.rpt",
            "*power*post_synth*vectorless*.rpt",
            "*power*post_synth*.rpt",
            "*power*.rpt",
        ],
    },
    {
        "name": "Rocket Core",
        "util_patterns": [
            "utilization_rocketcore_only.rpt",
            "*utilization*rocketcore*.rpt",
            "utilization_rockettile_only.rpt",
            "*utilization*rockettile*.rpt",
        ],
        "power_patterns": [
            "power_rocketcore_only.rpt",
            "*power*rocketcore*.rpt",
            "power_rockettile_only.rpt",
            "*power*rockettile*.rpt",
        ],
    },
    {
        "name": "ThreadManager",
        "util_patterns": [
            "utilization_threadmanager_only.rpt",
            "utilization_threadmanager_tm_*.rpt",
            "*utilization*threadmanager*.rpt",
        ],
        "power_patterns": [
            "power_threadmanager_only.rpt",
            "power_threadmanager_tm_*.rpt",
            "*power*threadmanager*.rpt",
        ],
    },
    {
        "name": "Threadlet Logic",
        "util_patterns": [
            "utilization_threadlet_logic_only.rpt",
            "*utilization*threadlet_logic*.rpt",
        ],
        "power_patterns": [
            "power_threadlet_logic_only.rpt",
            "*power*threadlet_logic*.rpt",
        ],
    },
    {
        "name": "ThreadContextManager",
        "util_patterns": [
            "utilization_threadcontextmanager_only.rpt",
            "*utilization*threadcontextmanager*.rpt",
            "*utilization*threadctxmgr*.rpt",
        ],
        "power_patterns": [
            "power_threadcontextmanager_only.rpt",
            "*power*threadcontextmanager*.rpt",
            "*power*threadctxmgr*.rpt",
        ],
    },
    {
        "name": "ThreadContextSaver",
        "util_patterns": [
            "utilization_threadcontextsaver_only.rpt",
            "*utilization*threadcontextsaver*.rpt",
            "*utilization*threadctxsaver*.rpt",
        ],
        "power_patterns": [
            "power_threadcontextsaver_only.rpt",
            "*power*threadcontextsaver*.rpt",
            "*power*threadctxsaver*.rpt",
        ],
    },
    {
        "name": "ThreadContextDmemArbiter",
        "util_patterns": [
            "utilization_threadcontextdmemarbiter_only.rpt",
            "*utilization*threadcontextdmemarbiter*.rpt",
            "*utilization*threadctxdmemarb*.rpt",
        ],
        "power_patterns": [
            "power_threadcontextdmemarbiter_only.rpt",
            "*power*threadcontextdmemarbiter*.rpt",
            "*power*threadctxdmemarb*.rpt",
        ],
    },
    {
        "name": "Threadlet SRAM-like",
        "util_patterns": [
            "utilization_threadlet_sram_like_only.rpt",
            "*utilization*threadlet_sram_like*.rpt",
        ],
        "power_patterns": [
            "power_threadlet_sram_like_only.rpt",
            "*power*threadlet_sram_like*.rpt",
        ],
    },
    {
        "name": "Synthesized Printf",
        "util_patterns": [
            "utilization_synthesized_printf_only.rpt",
            "*utilization*synthesized_printf*.rpt",
            "*utilization*printf*.rpt",
        ],
        "power_patterns": [
            "power_synthesized_printf_only.rpt",
            "*power*synthesized_printf*.rpt",
            "*power*printf*.rpt",
        ],
    },
    {
        "name": "Synthesized Assert",
        "util_patterns": [
            "utilization_synthesized_assert_only.rpt",
            "*utilization*synthesized_assert*.rpt",
            "*utilization*assert*.rpt",
        ],
        "power_patterns": [
            "power_synthesized_assert_only.rpt",
            "*power*synthesized_assert*.rpt",
            "*power*assert*.rpt",
        ],
    },
]

AREA_REGEX = {
    "LUT": [
        r"^\|\s*(?:CLB LUTs\*?|Slice LUTs\*?)\s*\|\s*([0-9][0-9,]*(?:\.[0-9]+)?(?:[eE][+-]?[0-9]+)?)\s*\|",
    ],
    "FF": [
        r"^\|\s*(?:CLB Registers\*?|Slice Registers\*?)\s*\|\s*([0-9][0-9,]*(?:\.[0-9]+)?(?:[eE][+-]?[0-9]+)?)\s*\|",
    ],
    "BRAM_TILE": [
        r"^\|\s*Block RAM Tile\s*\|\s*([0-9][0-9,]*(?:\.[0-9]+)?(?:[eE][+-]?[0-9]+)?)\s*\|",
    ],
    "URAM": [
        r"^\|\s*URAM\*?\s*\|\s*([0-9][0-9,]*(?:\.[0-9]+)?(?:[eE][+-]?[0-9]+)?)\s*\|",
    ],
    "DSP": [
        r"^\|\s*DSPs\*?\s*\|\s*([0-9][0-9,]*(?:\.[0-9]+)?(?:[eE][+-]?[0-9]+)?)\s*\|",
    ],
    "LUT_LOGIC": [
        r"^\|\s*LUT as Logic\s*\|\s*([0-9][0-9,]*(?:\.[0-9]+)?(?:[eE][+-]?[0-9]+)?)\s*\|",
    ],
    "LUT_MEMORY": [
        r"^\|\s*LUT as Memory\s*\|\s*([0-9][0-9,]*(?:\.[0-9]+)?(?:[eE][+-]?[0-9]+)?)\s*\|",
    ],
    "RAMB36": [
        r"^\|\s*RAMB36(?:/FIFO)?\*?\s*\|\s*([0-9][0-9,]*(?:\.[0-9]+)?(?:[eE][+-]?[0-9]+)?)\s*\|",
    ],
    "RAMB18": [
        r"^\|\s*RAMB18\*?\s*\|\s*([0-9][0-9,]*(?:\.[0-9]+)?(?:[eE][+-]?[0-9]+)?)\s*\|",
    ],
    "MUXF7": [
        r"^\|\s*MUXF7\s*\|\s*([0-9][0-9,]*(?:\.[0-9]+)?(?:[eE][+-]?[0-9]+)?)\s*\|",
    ],
    "MUXF8": [
        r"^\|\s*MUXF8\s*\|\s*([0-9][0-9,]*(?:\.[0-9]+)?(?:[eE][+-]?[0-9]+)?)\s*\|",
    ],
}

POWER_REGEX = {
    "TOTAL_POWER_W": [
        r"^\|\s*Total On-Chip Power\s*\(W\)\s*\|\s*([0-9][0-9,]*(?:\.[0-9]+)?(?:[eE][+-]?[0-9]+)?)\s*\|",
        r"Total On-Chip Power\s*\(W\)\s*[:=]\s*([0-9][0-9,]*(?:\.[0-9]+)?(?:[eE][+-]?[0-9]+)?)",
    ],
    "DYNAMIC_POWER_W": [
        r"^\|\s*Total Dynamic Power\s*\(W\)\s*\|\s*([0-9][0-9,]*(?:\.[0-9]+)?(?:[eE][+-]?[0-9]+)?)\s*\|",
        r"^\|\s*Dynamic\s*\(W\)\s*\|\s*([0-9][0-9,]*(?:\.[0-9]+)?(?:[eE][+-]?[0-9]+)?)\s*\|",
        r"Total Dynamic Power\s*\(W\)\s*[:=]\s*([0-9][0-9,]*(?:\.[0-9]+)?(?:[eE][+-]?[0-9]+)?)",
        r"Dynamic\s*\(W\)\s*[:=]\s*([0-9][0-9,]*(?:\.[0-9]+)?(?:[eE][+-]?[0-9]+)?)",
    ],
    "STATIC_POWER_W": [
        r"^\|\s*Device Static\s*\(W\)\s*\|\s*([0-9][0-9,]*(?:\.[0-9]+)?(?:[eE][+-]?[0-9]+)?)\s*\|",
        r"^\|\s*Static\s*\(W\)\s*\|\s*([0-9][0-9,]*(?:\.[0-9]+)?(?:[eE][+-]?[0-9]+)?)\s*\|",
        r"Device Static\s*\(W\)\s*[:=]\s*([0-9][0-9,]*(?:\.[0-9]+)?(?:[eE][+-]?[0-9]+)?)",
        r"Static\s*\(W\)\s*[:=]\s*([0-9][0-9,]*(?:\.[0-9]+)?(?:[eE][+-]?[0-9]+)?)",
    ],
}

AREA_SUMMARY_KEYS = [
    ("LUT", "LUT"),
    ("FF", "FF"),
    ("BRAM_TILE", "BRAM Tile"),
    ("URAM", "URAM"),
    ("DSP", "DSP"),
    ("LUT_LOGIC", "LUT as Logic"),
    ("LUT_MEMORY", "LUT as Memory"),
    ("RAMB36", "RAMB36"),
    ("RAMB18", "RAMB18"),
    ("MUXF7", "MUXF7"),
    ("MUXF8", "MUXF8"),
]

POWER_SUMMARY_KEYS = [
    ("TOTAL_POWER_W", "Total On-Chip Power"),
    ("DYNAMIC_POWER_W", "Total Dynamic Power"),
    ("STATIC_POWER_W", "Device Static Power"),
]


def parse_number(token: str) -> float:
    return float(token.replace(",", ""))


def find_first_report(root_dir: Path, patterns):
    for pattern in patterns:
        matches = [m for m in root_dir.rglob(pattern) if m.is_file()]
        # Hierarchical reports use a different table format that the summary
        # regexes cannot parse; never let a broad fallback glob pick one up.
        if "hier" not in pattern:
            matches = [m for m in matches if "hier" not in m.name.lower()]
        if matches:
            return max(matches, key=lambda p: (p.stat().st_mtime, str(p)))
    return None


def extract_first(text: str, regex_list):
    for pattern in regex_list:
        m = re.search(pattern, text, flags=re.IGNORECASE | re.MULTILINE)
        if m:
            return parse_number(m.group(1))
    return None


def parse_util_report(path: Path):
    text = path.read_text(encoding="utf-8", errors="ignore")
    data = {}
    for key, regex_list in AREA_REGEX.items():
        value = extract_first(text, regex_list)
        if value is not None:
            data[key] = value

    if "LUT" not in data and ("LUT_LOGIC" in data or "LUT_MEMORY" in data):
        data["LUT"] = data.get("LUT_LOGIC", 0.0) + data.get("LUT_MEMORY", 0.0)

    if "BRAM_TILE" not in data and ("RAMB36" in data or "RAMB18" in data):
        data["BRAM_TILE"] = data.get("RAMB36", 0.0) + data.get("RAMB18", 0.0) / 2.0

    # Vivado's primitives table omits zero-count rows. A successfully parsed
    # report without a MUXF7/MUXF8 row therefore means zero, not unknown;
    # leaving them as None would blank every derived residual row.
    if "LUT" in data:
        data.setdefault("MUXF7", 0.0)
        data.setdefault("MUXF8", 0.0)

    return data


def parse_power_report(path: Path):
    text = path.read_text(encoding="utf-8", errors="ignore")
    data = {}
    for key, regex_list in POWER_REGEX.items():
        value = extract_first(text, regex_list)
        if value is not None:
            data[key] = value
    return data


def fmt(name: str, value):
    if value is None:
        return "N/A"
    if name.endswith("_W"):
        return f"{value:.3f}"
    if abs(value - round(value)) < 1e-9:
        return str(int(round(value)))
    return f"{value:.2f}"


def infer_stage(report_path: Path):
    name = report_path.name.lower()
    if "post_route" in name:
        return "post_route"
    if "post_synth" in name:
        return "post_synth"
    return "custom"


def print_area_summary(area_data):
    print(f"{'Metric':<24} {'Used':>14}")
    for key, label in AREA_SUMMARY_KEYS:
        print(f"{label:<24} {fmt(key, area_data.get(key)):>14}")


def print_power_summary(power_data):
    print(f"{'Metric':<24} {'Value(W)':>14}")
    for key, label in POWER_SUMMARY_KEYS:
        print(f"{label:<24} {fmt(key, power_data.get(key)):>14}")


def summarize_one_line(area_data, power_data):
    return (
        "Area "
        f"LUT={fmt('LUT', area_data.get('LUT'))}, "
        f"FF={fmt('FF', area_data.get('FF'))}, "
        f"BRAM={fmt('BRAM_TILE', area_data.get('BRAM_TILE'))}, "
        f"URAM={fmt('URAM', area_data.get('URAM'))}, "
        f"DSP={fmt('DSP', area_data.get('DSP'))}; "
        "Power "
        f"Total={fmt('TOTAL_POWER_W', power_data.get('TOTAL_POWER_W'))}W, "
        f"Dynamic={fmt('DYNAMIC_POWER_W', power_data.get('DYNAMIC_POWER_W'))}W, "
        f"Static={fmt('STATIC_POWER_W', power_data.get('STATIC_POWER_W'))}W."
    )


def load_section_reports(threadlet_dir: Path, section):
    util_report = find_first_report(threadlet_dir, section["util_patterns"])
    power_report = find_first_report(threadlet_dir, section["power_patterns"])

    area_data = parse_util_report(util_report) if util_report else {}
    power_data = parse_power_report(power_report) if power_report else {}

    return {
        "name": section["name"],
        "util_report": util_report,
        "power_report": power_report,
        "util_stage": infer_stage(util_report) if util_report else "N/A",
        "power_stage": infer_stage(power_report) if power_report else "N/A",
        "area_data": area_data,
        "power_data": power_data,
    }


def load_build_report(report_dir: Path):
    return {section["name"]: load_section_reports(report_dir, section) for section in SECTIONS}


def compare_metric(base_value, threadlet_value):
    if base_value is None or threadlet_value is None:
        return {
            "baseline": base_value,
            "threadlet": threadlet_value,
            "delta": None,
            "percent": None,
        }

    delta = threadlet_value - base_value
    percent = None if base_value == 0 else delta * 100.0 / base_value
    return {
        "baseline": base_value,
        "threadlet": threadlet_value,
        "delta": delta,
        "percent": percent,
    }


def compare_build_reports(baseline_dir: Path, threadlet_dir: Path):
    baseline = load_build_report(Path(baseline_dir).expanduser())
    threadlet = load_build_report(Path(threadlet_dir).expanduser())

    comparison = {}
    for section in SECTIONS:
        name = section["name"]
        base_sec = baseline[name]
        threadlet_sec = threadlet[name]

        area = {}
        for key, _ in AREA_SUMMARY_KEYS:
            area[key] = compare_metric(
                base_sec["area_data"].get(key),
                threadlet_sec["area_data"].get(key),
            )

        power = {}
        for key, _ in POWER_SUMMARY_KEYS:
            power[key] = compare_metric(
                base_sec["power_data"].get(key),
                threadlet_sec["power_data"].get(key),
            )

        comparison[name] = {
            "baseline": base_sec,
            "threadlet": threadlet_sec,
            "area": area,
            "power": power,
        }

    return comparison


def printf_build_flavor(comparison):
    section = comparison.get("Synthesized Printf")

    def flavor(side):
        if not section:
            return "unknown"
        value = section[side]["area_data"].get("LUT")
        if value is None or value == 0:
            return "no-print"
        return "with-print"

    return flavor("baseline"), flavor("threadlet")


def printf_parity_warning(comparison):
    base_flavor, thr_flavor = printf_build_flavor(comparison)
    if base_flavor == thr_flavor:
        if thr_flavor == "with-print":
            return (
                "NOTE: both builds carry synthesized printf hardware. Numbers "
                "are valid for relative iteration tracking; modules containing "
                "synthesized printfs report UPPER BOUNDS (printf entangles "
                "with functional logic). Use a paired no-print build for "
                "paper-final area."
            )
        return None
    return (
        "WARNING: printf-synthesis mismatch (baseline={}, threadlet={}). "
        "Modules containing synthesized printfs report UPPER BOUNDS only, "
        "and full-chip deltas include printf infrastructure. Do NOT use "
        "this comparison for paper-final area; re-run a paired no-print "
        "build.".format(base_flavor, thr_flavor)
    )


def fmt_delta(name: str, value):
    if value is None:
        return "N/A"
    if name.endswith("_W"):
        return f"{value:+.3f}"
    if abs(value - round(value)) < 1e-9:
        return f"{int(round(value)):+d}"
    return f"{value:+.2f}"


def fmt_percent(value):
    if value is None:
        return "N/A"
    return f"{value:+.2f}%"


def print_compare_table(title, rows, unit_suffix=""):
    print(f"\n{title}")
    print("-" * len(title))
    print(
        f"{'Metric':<24} {'Baseline':>14} {'Threadlet':>14} "
        f"{'Delta':>14} {'Delta %':>10}"
    )
    for key, label, data in rows:
        print(
            f"{label:<24} "
            f"{fmt(key, data['baseline']):>14} "
            f"{fmt(key, data['threadlet']):>14} "
            f"{fmt_delta(key, data['delta']):>14} "
            f"{fmt_percent(data['percent']):>10}"
        )


def print_compare_summary(comparison):
    print("Threadlet vs Baseline Vivado Report")
    print("===================================")

    base_flavor, thr_flavor = printf_build_flavor(comparison)
    print(f"\nbaseline build flavor : {base_flavor}")
    print(f"threadlet build flavor: {thr_flavor}")
    warning = printf_parity_warning(comparison)
    if warning:
        print(f"\n{warning}")

    for section_name, sec in comparison.items():
        print(f"\n[{section_name}]")
        print("-" * (len(section_name) + 2))
        print(f"baseline utilization: {sec['baseline']['util_report'] or 'N/A'}")
        print(f"threadlet utilization: {sec['threadlet']['util_report'] or 'N/A'}")
        print(f"baseline power      : {sec['baseline']['power_report'] or 'N/A'}")
        print(f"threadlet power      : {sec['threadlet']['power_report'] or 'N/A'}")

        area_rows = [(key, label, sec["area"][key]) for key, label in AREA_SUMMARY_KEYS]
        power_rows = [(key, label, sec["power"][key]) for key, label in POWER_SUMMARY_KEYS]

        print_compare_table("Area Delta", area_rows)
        print_compare_table("Power Delta", power_rows)


def write_compare_csv(comparison, csv_path: Path):
    with csv_path.open("w", newline="") as csv_file:
        writer = csv.writer(csv_file)
        writer.writerow(
            ["section", "kind", "metric", "baseline", "threadlet", "delta", "delta_percent"]
        )
        for section_name, sec in comparison.items():
            for key, label in AREA_SUMMARY_KEYS:
                data = sec["area"][key]
                writer.writerow(
                    [
                        section_name,
                        "area",
                        label,
                        data["baseline"],
                        data["threadlet"],
                        data["delta"],
                        data["percent"],
                    ]
                )
            for key, label in POWER_SUMMARY_KEYS:
                data = sec["power"][key]
                writer.writerow(
                    [
                        section_name,
                        "power",
                        label,
                        data["baseline"],
                        data["threadlet"],
                        data["delta"],
                        data["percent"],
                    ]
                )


def metric_value(comparison, section_name: str, metric: str, side: str):
    section = comparison.get(section_name)
    if not section:
        return None
    if side == "baseline":
        return section["baseline"]["area_data"].get(metric)
    if side == "threadlet":
        return section["threadlet"]["area_data"].get(metric)
    if side == "delta":
        return section["area"][metric]["delta"]
    raise ValueError(f"unknown metric side: {side}")


def write_phase3_breakdown_csv(comparison, csv_path: Path):
    rows = []
    sections = [
        "Full Chip",
        "Rocket Core",
        "Threadlet Logic",
        "ThreadManager",
        "ThreadContextManager",
        "ThreadContextSaver",
        "ThreadContextDmemArbiter",
        "Threadlet SRAM-like",
        "Synthesized Printf",
        "Synthesized Assert",
    ]

    base_flavor, thr_flavor = printf_build_flavor(comparison)
    rows.append([
        "Build Flavor",
        "printf_synthesis",
        base_flavor,
        thr_flavor,
        None,
        printf_parity_warning(comparison) or "paired no-print builds",
    ])

    for section_name in sections:
        section = comparison.get(section_name)
        if not section:
            continue
        for key, label in AREA_SUMMARY_KEYS:
            rows.append([
                section_name,
                label,
                metric_value(comparison, section_name, key, "baseline"),
                metric_value(comparison, section_name, key, "threadlet"),
                metric_value(comparison, section_name, key, "delta"),
                "direct",
            ])

    explicit_sections = [
        "ThreadManager",
        "ThreadContextManager",
        "ThreadContextSaver",
        "ThreadContextDmemArbiter",
    ]
    for key, label in AREA_SUMMARY_KEYS:
        rocket_delta = metric_value(comparison, "Rocket Core", key, "delta")
        explicit_total = 0.0
        explicit_complete = rocket_delta is not None
        for section_name in explicit_sections:
            value = metric_value(comparison, section_name, key, "threadlet")
            if value is None:
                explicit_complete = False
                break
            explicit_total += value

        residual_delta = None
        if explicit_complete:
            residual_delta = rocket_delta - explicit_total

        rows.append([
            "RocketCore residual",
            label,
            None,
            None,
            residual_delta,
            "Rocket Core delta minus explicit threadlet submodules",
        ])

        residual_abs = None
        rocket_threadlet = metric_value(comparison, "Rocket Core", key, "threadlet")
        if explicit_complete and rocket_threadlet is not None:
            residual_abs = rocket_threadlet - explicit_total
        rows.append([
            "RocketCore residual absolute",
            label,
            None,
            residual_abs,
            None,
            "Threadlet Rocket Core minus explicit threadlet submodules",
        ])

    csv_path.parent.mkdir(parents=True, exist_ok=True)
    with csv_path.open("w", newline="") as csv_file:
        writer = csv.writer(csv_file)
        writer.writerow(["section", "metric", "baseline", "threadlet", "delta", "note"])
        writer.writerows(rows)


def print_single_build_summary(threadlet_dir: Path):
    if not threadlet_dir.exists():
        print(f"ERROR: THREADLET_DIR does not exist: {threadlet_dir}")
        sys.exit(1)

    print("Threadlet Bitstream Report")
    print("========================")
    print(f"THREADLET_DIR: {threadlet_dir}")

    sections = list(load_build_report(threadlet_dir).values())

    # Full-chip utilization is mandatory; others are optional.
    if sections[0]["util_report"] is None:
        print("ERROR: full-chip utilization report not found in THREADLET_DIR")
        sys.exit(1)

    for sec in sections:
        print(f"\n[{sec['name']}]")
        print("-" * (len(sec["name"]) + 2))
        print(
            f"utilization: {sec['util_report'] if sec['util_report'] else 'N/A'} "
            f"({sec['util_stage']})"
        )
        print(
            f"power      : {sec['power_report'] if sec['power_report'] else 'N/A'} "
            f"({sec['power_stage']})"
        )

        print("\nArea Summary")
        print("------------")
        print_area_summary(sec["area_data"])

        print("\nPower Summary")
        print("-------------")
        print_power_summary(sec["power_data"])

        print("\nOne-line Summary")
        print("----------------")
        print(summarize_one_line(sec["area_data"], sec["power_data"]))


def parse_args(argv):
    parser = argparse.ArgumentParser(
        description=(
            "Summarize Vivado utilization/power reports or compare a baseline "
            "Rocket build against a threadlet build."
        )
    )
    parser.add_argument(
        "report_dir",
        nargs="?",
        help="Directory containing generated Vivado .rpt files for one build.",
    )
    parser.add_argument(
        "--compare",
        nargs=2,
        metavar=("BASELINE_DIR", "THREADLET_DIR"),
        help="Compare two directories containing generated Vivado .rpt files.",
    )
    parser.add_argument(
        "--csv",
        type=Path,
        help="Optional CSV output path for --compare.",
    )
    parser.add_argument(
        "--phase3-csv",
        type=Path,
        help="Optional Phase 3 area breakdown CSV output path for --compare.",
    )
    return parser.parse_args(argv)


def main(argv=None):
    args = parse_args(argv)

    if args.compare:
        baseline_dir = Path(args.compare[0]).expanduser()
        threadlet_dir = Path(args.compare[1]).expanduser()
        comparison = compare_build_reports(baseline_dir, threadlet_dir)
        print_compare_summary(comparison)
        if args.csv:
            write_compare_csv(comparison, args.csv)
            print(f"\nCSV written to: {args.csv}")
        if args.phase3_csv:
            write_phase3_breakdown_csv(comparison, args.phase3_csv)
            print(f"Phase 3 area breakdown CSV written to: {args.phase3_csv}")
        return

    if not args.report_dir:
        print("ERROR: provide report_dir or --compare BASELINE_DIR THREADLET_DIR")
        sys.exit(1)

    print_single_build_summary(Path(args.report_dir).expanduser())


if __name__ == "__main__":
    main()
