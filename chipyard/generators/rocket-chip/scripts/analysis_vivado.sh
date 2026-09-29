# BUILD_DIR="/home/qxh/CHIPYARD_TEST/chipyard/sims/firesim/deploy/results-build/2025-12-04--05-23-21-xilinx_vcu118_firesim_rocket_dualcore_4GB_with_nic_sv48_threadlet_without_csrs_only_for_linux_syn_print2/cl_xilinx_vcu118-firesim-FireSim-FireSimLoopbackNICRocket4GiBDRAMSV48DualCoreConfig-WithPrintfSynthesis_BaseXilinxVCU118Config/build"
BUILD_DIR="/home/qxh/CHIPYARD_TEST/chipyard/sims/firesim/deploy/results-build/2026-03-07--02-29-44-threadlet_release_2c/cl_xilinx_vcu118-firesim-FireSim-FireSimLoopbackNICRocket4GiBDRAMSV48DualCoreConfig-WithPrintfSynthesis_BaseXilinxVCU118Config/build"
cd "$BUILD_DIR"
ls -lh post_route.dcp post_synth.dcp

cat > run_post_route_reports.tcl <<'TCL'
open_checkpoint post_route.dcp

report_timing_summary -warn_on_violation -file timing_summary_post_route.rpt
report_utilization -file utilization_post_route.rpt
report_utilization -hierarchical -hierarchical_depth 8 -file utilization_hier_post_route.rpt
report_power -file power_post_route_vectorless.rpt
report_clock_utilization -file clock_utilization_post_route.rpt
report_route_status -file route_status_post_route.rpt
report_drc -file drc_post_route.rpt

# Rocket-only reports
set rocket_tile_cells [get_cells -hier -filter {NAME =~ *chiptop0/system/tile_prci_domain*/element_reset_domain_rockettile*}]
set rocket_core_cells [get_cells -hier -filter {NAME =~ *chiptop0/system/tile_prci_domain*/element_reset_domain_rockettile/core*}]

puts "INFO: rocket_tile_cells count = [llength $rocket_tile_cells]"
puts "INFO: rocket_core_cells count = [llength $rocket_core_cells]"

if {[llength $rocket_tile_cells] > 0} {
  report_utilization -cells $rocket_tile_cells -file utilization_rockettile_only.rpt
  if {[catch {report_power -cells $rocket_tile_cells -file power_rockettile_only.rpt} err]} {
    puts "WARNING: report_power -cells failed for rockettile: $err"
    set fh [open power_rockettile_only.rpt w]
    puts $fh "WARNING: report_power -cells failed for rockettile."
    puts $fh "$err"
    close $fh
  }
} else {
  puts "WARNING: No cells matched for rockettile."
  set fh [open utilization_rockettile_only.rpt w]
  puts $fh "WARNING: No cells matched for rockettile."
  close $fh
  set fh [open power_rockettile_only.rpt w]
  puts $fh "WARNING: No cells matched for rockettile."
  close $fh
}

if {[llength $rocket_core_cells] > 0} {
  report_utilization -cells $rocket_core_cells -file utilization_rocketcore_only.rpt
  if {[catch {report_power -cells $rocket_core_cells -file power_rocketcore_only.rpt} err]} {
    puts "WARNING: report_power -cells failed for rocketcore: $err"
    set fh [open power_rocketcore_only.rpt w]
    puts $fh "WARNING: report_power -cells failed for rocketcore."
    puts $fh "$err"
    close $fh
  }
} else {
  puts "WARNING: No cells matched for rocketcore."
  set fh [open utilization_rocketcore_only.rpt w]
  puts $fh "WARNING: No cells matched for rocketcore."
  close $fh
  set fh [open power_rocketcore_only.rpt w]
  puts $fh "WARNING: No cells matched for rocketcore."
  close $fh
}

exit
TCL

vivado -mode batch -source run_post_route_reports.tcl
ls -lh *post_route*.rpt utilization_rockettile_only.rpt utilization_rocketcore_only.rpt power_rockettile_only.rpt power_rocketcore_only.rpt *.rpt | sed -n '1,260p'
