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
