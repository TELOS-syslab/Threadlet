#!/usr/bin/env bash
set -euo pipefail

DEFAULT_THREADLET_BUILD_DIR="/home/qxh/CHIPYARD_TEST/chipyard/sims/firesim/deploy/results-build/2026-07-05--03-11-10-threadlet_sosploom3_7_noprint/cl_xilinx_vcu118-firesim-FireSim-FireSimLoopbackNICRocket4GiBDRAMSV484CoreConfig-BaseXilinxVCU118Config/build"
DEFAULT_BASELINE_BUILD_DIR="/home/qxh/CHIPYARD_TEST/chipyard/sims/firesim/deploy/results-build/2026-03-15--03-34-24-linux_4cores_withnewNIC/cl_xilinx_vcu118-firesim-FireSim-FireSimLoopbackNICRocket4GiBDRAMSV484CoreConfig-BaseXilinxVCU118Config/build"

usage() {
  cat <<'USAGE'
Usage:
  analysis_vivado_threadlet.sh
  analysis_vivado_threadlet.sh OUT_DIR
  analysis_vivado_threadlet.sh BASELINE_BUILD_DIR THREADLET_BUILD_DIR [OUT_DIR]
  analysis_vivado_threadlet.sh --skip-vivado BASELINE_REPORT_DIR THREADLET_REPORT_DIR [OUT_DIR]

With no arguments, the script uses the hardcoded baseline Rocket and Threadlet
2.9g build directories. The normal mode opens post_route.dcp in both build
directories, generates post-route utilization and vectorless power reports,
then prints the Threadlet minus baseline delta.

The --skip-vivado mode only parses existing reports. It expects each report
directory to contain files named like utilization_post_route.rpt and
power_post_route_vectorless.rpt.
USAGE
}

abs_path() {
  local path="$1"
  if [[ -d "$path" ]]; then
    (cd "$path" && pwd)
  else
    local dir
    local base
    dir="$(dirname "$path")"
    base="$(basename "$path")"
    if [[ -d "$dir" ]]; then
      echo "$(cd "$dir" && pwd)/$base"
    else
      echo "$path"
    fi
  fi
}

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
skip_vivado=0

if [[ "${1:-}" == "-h" || "${1:-}" == "--help" ]]; then
  usage
  exit 0
fi

if [[ "${1:-}" == "--skip-vivado" ]]; then
  skip_vivado=1
  shift
fi

if [[ "$skip_vivado" -eq 0 ]]; then
  case "$#" in
    0)
      baseline_input="$DEFAULT_BASELINE_BUILD_DIR"
      threadlet_input="$DEFAULT_THREADLET_BUILD_DIR"
      out_dir="$PWD/threadlet_vivado_compare_$(date +%Y%m%d_%H%M%S)"
      ;;
    1)
      baseline_input="$DEFAULT_BASELINE_BUILD_DIR"
      threadlet_input="$DEFAULT_THREADLET_BUILD_DIR"
      out_dir="$1"
      ;;
    2|3)
      baseline_input="$1"
      threadlet_input="$2"
      out_dir="${3:-"$PWD/threadlet_vivado_compare_$(date +%Y%m%d_%H%M%S)"}"
      ;;
    *)
      usage >&2
      exit 2
      ;;
  esac
else
  if [[ $# -lt 2 || $# -gt 3 ]]; then
    usage >&2
    exit 2
  fi
  baseline_input="$1"
  threadlet_input="$2"
  out_dir="${3:-"$PWD/threadlet_vivado_compare_$(date +%Y%m%d_%H%M%S)"}"
fi

baseline_input="$(abs_path "$baseline_input")"
threadlet_input="$(abs_path "$threadlet_input")"
mkdir -p "$out_dir"
out_dir="$(abs_path "$out_dir")"

generate_reports() {
  local build_dir="$1"
  local label="$2"
  local report_dir="$out_dir/$label"
  local tcl="$report_dir/run_vivado_reports.tcl"

  if [[ ! -f "$build_dir/post_route.dcp" ]]; then
    echo "ERROR: missing post_route.dcp in $build_dir" >&2
    exit 1
  fi

  mkdir -p "$report_dir"

  cat > "$tcl" <<'TCL'
set checkpoint_path [lindex $argv 0]
set out_dir [lindex $argv 1]

file mkdir $out_dir
open_checkpoint $checkpoint_path

proc write_cells {path cells} {
  set fh [open $path w]
  foreach c $cells {
    puts $fh $c
  }
  close $fh
}

proc report_optional_cells {label cells util_name power_name} {
  global out_dir
  set cells [lsort -unique $cells]
  write_cells [file join $out_dir "${label}_cells.txt"] $cells

  if {[llength $cells] > 0} {
    report_utilization -cells $cells -file [file join $out_dir $util_name]

    # Vivado 2020.2 uses report_power -cell, not report_power -cells.
    # Try the aggregate object list first; if the version rejects a list,
    # fall back to per-root reports and emit a parser-friendly summed file.
    if {[catch {report_power -cell $cells -file [file join $out_dir $power_name]} err]} {
      set total_power 0.0
      set dynamic_power 0.0
      set static_power 0.0
      set ok_count 0
      set idx 0

      foreach cell $cells {
        incr idx
        set cell_power_file [file join $out_dir [format "power_%s_cell_%02d.rpt" $label $idx]]
        if {[catch {report_power -cell $cell -file $cell_power_file} cell_err]} {
          set fh [open $cell_power_file w]
          puts $fh "WARNING: report_power -cell failed for $cell"
          puts $fh $cell_err
          close $fh
        } else {
          set fh [open $cell_power_file r]
          set text [read $fh]
          close $fh

          set value 0.0
          if {[regexp {\|\s*Total On-Chip Power\s*\(W\)\s*\|\s*([0-9.]+)} $text -> value]} {
            set total_power [expr {$total_power + $value}]
          }
          set has_dynamic [regexp {\|\s*Dynamic\s*\(W\)\s*\|\s*([0-9.]+)} $text -> value]
          if {!$has_dynamic} {
            set has_dynamic [regexp {\|\s*Total Dynamic Power\s*\(W\)\s*\|\s*([0-9.]+)} $text -> value]
          }
          if {$has_dynamic} {
            set dynamic_power [expr {$dynamic_power + $value}]
          }
          if {[regexp {\|\s*Device Static\s*\(W\)\s*\|\s*([0-9.]+)} $text -> value]} {
            set static_power [expr {$static_power + $value}]
          }
          incr ok_count
        }
      }

      set fh [open [file join $out_dir $power_name] w]
      if {$ok_count > 0} {
        puts $fh "Power Report"
        puts $fh ""
        puts $fh "+--------------------------+--------------+"
        puts $fh [format "| Total On-Chip Power (W)  | %.6f     |" $total_power]
        puts $fh [format "| Total Dynamic Power (W)  | %.6f     |" $dynamic_power]
        puts $fh [format "| Device Static (W)        | %.6f     |" $static_power]
        puts $fh "+--------------------------+--------------+"
        puts $fh ""
        puts $fh "NOTE: Summed from $ok_count non-overlapping root-cell report_power -cell reports."
        puts $fh "Original aggregate report_power -cell object-list error:"
        puts $fh $err
      } else {
        puts $fh "WARNING: report_power -cell failed for all $label cells."
        puts $fh "Original aggregate report_power -cell object-list error:"
        puts $fh $err
      }
      close $fh
    }
  } else {
    set fh [open [file join $out_dir $util_name] w]
    puts $fh "WARNING: No cells matched for $label."
    close $fh

    set fh [open [file join $out_dir $power_name] w]
    puts $fh "WARNING: No cells matched for $label."
    close $fh
  }
}

proc report_optional_cells_hier {label cells util_name} {
  global out_dir
  set cells [lsort -unique $cells]
  write_cells [file join $out_dir "${label}_cells.txt"] $cells

  if {[llength $cells] > 0} {
    if {[catch {report_utilization -hierarchical -hierarchical_depth 10 -cells $cells -file [file join $out_dir $util_name]} err]} {
      set fh [open [file join $out_dir $util_name] w]
      puts $fh "WARNING: hierarchical report_utilization failed for $label."
      puts $fh $err
      close $fh
    }
  } else {
    set fh [open [file join $out_dir $util_name] w]
    puts $fh "WARNING: No cells matched for $label."
    close $fh
  }
}

proc prune_descendants {cells} {
  # report_utilization -cells double counts when a cell list contains both a
  # cell and one of its descendants; keep only the hierarchy roots. Relies on
  # lexicographic order placing "x" before "x/..." before any sibling "x0...".
  set sorted [lsort -unique $cells]
  set result {}
  set last ""
  foreach c $sorted {
    if {$last ne "" && [string first "$last/" $c] == 0} {
      continue
    }
    lappend result $c
    set last $c
  }
  return $result
}

proc collect_cells_by_patterns {patterns} {
  set cells {}
  foreach pattern $patterns {
    set cells [concat $cells [get_cells -hier -quiet -filter "NAME =~ $pattern"]]
  }
  return [prune_descendants $cells]
}

proc report_rf_fingerprint {} {
  # Report-level evidence for the Phase 3.3 fingerprint: whether the integer
  # RF is implemented as LUTRAM (RAMD/RAMS/RAM*M refs) or as flip-flops (FDRE).
  global out_dir
  set rf_cells [get_cells -hier -quiet -filter {NAME =~ *element_reset_domain_rockettile/core/rf_reg*}]
  set fh [open [file join $out_dir rf_fingerprint.rpt] w]
  puts $fh "Integer RF implementation fingerprint (cells matching core/rf_reg*)"
  puts $fh "Matched cells (incl. macro children): [llength $rf_cells]"
  puts $fh ""
  set counts [dict create]
  foreach c $rf_cells {
    set ref [get_property -quiet REF_NAME $c]
    if {$ref eq ""} { set ref UNKNOWN }
    dict incr counts $ref
  }
  puts $fh [format "%-24s %10s" "REF_NAME" "count"]
  foreach ref [lsort [dict keys $counts]] {
    puts $fh [format "%-24s %10d" $ref [dict get $counts $ref]]
  }
  puts $fh ""
  puts $fh "LUTRAM implementation shows RAMD32/RAMD64E/RAMS32/RAM*M refs."
  puts $fh "FF implementation shows FDRE/FDSE refs (Phase 3.3 regression)."
  close $fh
}

proc try_report {cmd path} {
  set err ""
  if {[catch {uplevel 1 $cmd} err]} {
    set fh [open $path w]
    puts $fh "WARNING: Vivado report command failed."
    puts $fh $err
    close $fh
  }
}

# Full design reports.
report_timing_summary -warn_on_violation -file [file join $out_dir timing_summary_post_route.rpt]
report_utilization -file [file join $out_dir utilization_post_route.rpt]
report_utilization -hierarchical -hierarchical_depth 10 -file [file join $out_dir utilization_hier_post_route.rpt]
report_power -file [file join $out_dir power_post_route_vectorless.rpt]
report_power -hier all -file [file join $out_dir power_hier_post_route_vectorless.rpt]
report_clock_utilization -file [file join $out_dir clock_utilization_post_route.rpt]
report_route_status -file [file join $out_dir route_status_post_route.rpt]
report_drc -file [file join $out_dir drc_post_route.rpt]
try_report {report_high_fanout_nets -file [file join $out_dir high_fanout_nets_post_route.rpt]} \
  [file join $out_dir high_fanout_nets_post_route.rpt]

# Rocket reports. These aggregate root cells in multicore designs. Do not use
# trailing wildcards here; including both a root and its descendants can
# over-count utilization.
set rocket_tile_cells [get_cells -hier -quiet -filter {NAME =~ *chiptop0/system/tile_prci_domain*/element_reset_domain_rockettile}]
set rocket_core_cells [get_cells -hier -quiet -filter {NAME =~ *chiptop0/system/tile_prci_domain*/element_reset_domain_rockettile/core}]

report_optional_cells rockettile $rocket_tile_cells utilization_rockettile_only.rpt power_rockettile_only.rpt
report_optional_cells rocketcore $rocket_core_cells utilization_rocketcore_only.rpt power_rocketcore_only.rpt
report_optional_cells_hier rocketcore_hier $rocket_core_cells utilization_rocketcore_hier.rpt

# Threadlet-specific cells are useful as a sanity check, but the primary paper
# number should compare the same hierarchy in both builds.
set threadmanager_cells [get_cells -hier -quiet -filter {NAME =~ *chiptop0/system/tile_prci_domain*/element_reset_domain_rockettile/core/tm}]
report_optional_cells threadmanager $threadmanager_cells utilization_threadmanager_only.rpt power_threadmanager_only.rpt
report_optional_cells_hier threadmanager_hier $threadmanager_cells utilization_threadmanager_hier.rpt

set threadcontextmanager_cells [get_cells -hier -quiet -filter {NAME =~ *chiptop0/system/tile_prci_domain*/element_reset_domain_rockettile/core/threadCtxMgr}]
report_optional_cells threadcontextmanager $threadcontextmanager_cells utilization_threadcontextmanager_only.rpt power_threadcontextmanager_only.rpt

set threadcontextsaver_cells [get_cells -hier -quiet -filter {NAME =~ *chiptop0/system/tile_prci_domain*/element_reset_domain_rockettile/core/threadCtxSaver}]
report_optional_cells threadcontextsaver $threadcontextsaver_cells utilization_threadcontextsaver_only.rpt power_threadcontextsaver_only.rpt

set threadcontextdmemarbiter_cells [get_cells -hier -quiet -filter {NAME =~ *chiptop0/system/tile_prci_domain*/element_reset_domain_rockettile/core/threadCtxDmemArb}]
report_optional_cells threadcontextdmemarbiter $threadcontextdmemarbiter_cells utilization_threadcontextdmemarbiter_only.rpt power_threadcontextdmemarbiter_only.rpt

set threadlet_cells [collect_cells_by_patterns {
  *chiptop0/system/tile_prci_domain*/element_reset_domain_rockettile/core/tm
  *chiptop0/system/tile_prci_domain*/element_reset_domain_rockettile/core/threadCtxMgr
  *chiptop0/system/tile_prci_domain*/element_reset_domain_rockettile/core/threadCtxSaver
  *chiptop0/system/tile_prci_domain*/element_reset_domain_rockettile/core/threadCtxDmemArb
}]
report_optional_cells threadlet_logic $threadlet_cells utilization_threadlet_logic_only.rpt power_threadlet_logic_only.rpt
report_optional_cells_hier threadlet_logic_hier $threadlet_cells utilization_threadlet_logic_hier.rpt

# SRAM-like storage cells. rf_reg* anchors on the register-file cell names
# emitted by Vivado (LUTRAM macros rf_reg_r*_..., or FDRE rf_reg[...] in the
# degraded FF form); a bare *rf* glob also swept csr/div/fpu cells whose
# names merely contain the substring (e.g. "perf") and corrupted the bucket.
set threadlet_sram_like_cells [collect_cells_by_patterns {
  *chiptop0/system/tile_prci_domain*/element_reset_domain_rockettile/core/rf_reg*
  *chiptop0/system/tile_prci_domain*/element_reset_domain_rockettile/core/tm/*tptPriorityMem*
  *chiptop0/system/tile_prci_domain*/element_reset_domain_rockettile/core/tm/*tptSliceMem*
  *chiptop0/system/tile_prci_domain*/element_reset_domain_rockettile/core/tm/*tptDeadlineMem*
  *chiptop0/system/tile_prci_domain*/element_reset_domain_rockettile/core/tm/*coldPcBanks*
  *chiptop0/system/tile_prci_domain*/element_reset_domain_rockettile/core/tm/*monitorLineBanks*
}]
report_optional_cells threadlet_sram_like $threadlet_sram_like_cells utilization_threadlet_sram_like_only.rpt power_threadlet_sram_like_only.rpt
report_rf_fingerprint

set synthesized_printf_cells [collect_cells_by_patterns {
  *synthesizedPrintf*
  *SynthesizePrintf*
  *PrintBridge*
  *printf*
}]
report_optional_cells synthesized_printf $synthesized_printf_cells utilization_synthesized_printf_only.rpt power_synthesized_printf_only.rpt

set synthesized_assert_cells [collect_cells_by_patterns {
  *assert*
  *Assert*
}]
report_optional_cells synthesized_assert $synthesized_assert_cells utilization_synthesized_assert_only.rpt power_synthesized_assert_only.rpt

close_project
exit
TCL

  echo "Generating Vivado reports for $label:"
  echo "  build : $build_dir"
  echo "  output: $report_dir"
  vivado -mode batch -source "$tcl" -tclargs "$build_dir/post_route.dcp" "$report_dir" 2>&1 | tee "$report_dir/vivado.log"

  # Archive the synthesis-time RAM mapping tables (Phase 3.1 fingerprint
  # evidence). They only exist in the build's synthesis logs; the report run
  # above opens a routed checkpoint and never prints them.
  local mapping_out="$report_dir/ram_mapping_from_synth.log"
  local synth_log=""
  while IFS= read -r candidate; do
    if grep -qsm1 "RAM: Final Mapping" "$candidate"; then
      synth_log="$candidate"
      break
    fi
  done < <(find "$build_dir" -maxdepth 3 -name "*.log" 2>/dev/null | sort)
  if [[ -n "$synth_log" ]]; then
    {
      echo "Source: $synth_log"
      awk '/(Block RAM|Distributed RAM): Final Mapping/{f=1} f' "$synth_log" | head -4000
    } > "$mapping_out"
  else
    echo "WARNING: no synthesis log containing 'RAM: Final Mapping' found under $build_dir" > "$mapping_out"
  fi
}

if [[ "$skip_vivado" -eq 0 ]]; then
  if ! command -v vivado >/dev/null 2>&1; then
    echo "ERROR: vivado not found. Run on the FPGA build server or use --skip-vivado." >&2
    exit 1
  fi

  generate_reports "$baseline_input" baseline
  generate_reports "$threadlet_input" threadlet
  baseline_report_dir="$out_dir/baseline"
  threadlet_report_dir="$out_dir/threadlet"
else
  baseline_report_dir="$baseline_input"
  threadlet_report_dir="$threadlet_input"
fi

summary_txt="$out_dir/threadlet_vs_baseline_summary.txt"
summary_csv="$out_dir/threadlet_vs_baseline_summary.csv"

if ! python3 "$script_dir/threadlet_reports.py" --help 2>&1 | grep -q -- "--compare"; then
  echo "ERROR: $script_dir/threadlet_reports.py does not support --compare." >&2
  echo "Copy the updated threadlet_reports.py together with this shell script." >&2
  exit 1
fi

python3 "$script_dir/threadlet_reports.py" \
  --compare "$baseline_report_dir" "$threadlet_report_dir" \
  --csv "$summary_csv" \
  --phase3-csv "$out_dir/area_breakdown_phase3.csv" | tee "$summary_txt"

echo
echo "Summary written to: $summary_txt"
echo "CSV written to    : $summary_csv"
echo "Phase 3 CSV written to: $out_dir/area_breakdown_phase3.csv"
