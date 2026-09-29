#!/bin/sh

read p99_slowdown p99_latency throughput <<EOF
$(dmesg | tail -n 11 | awk '
  /P99 Latency:/ {
    if (match($0, /P99 Latency:[[:space:]]+[0-9]+/)) {
      lat = substr($0, RSTART)
      sub(/P99 Latency:[[:space:]]+/, "", lat)
      sub(/[[:space:]].*$/, "", lat)
    }
  }

  /P99 Slowdown:/ {
    if (match($0, /P99 Slowdown:[[:space:]]+[0-9]+/)) {
      sd = substr($0, RSTART)
      sub(/P99 Slowdown:[[:space:]]+/, "", sd)
      sub(/[[:space:]].*$/, "", sd)
    }
  }

  /Throughput:/ && !/Be Throughput:/ {
    if (match($0, /Throughput:[[:space:]]+[0-9]+/)) {
      tp = substr($0, RSTART)
      sub(/Throughput:[[:space:]]+/, "", tp)
      sub(/[[:space:]].*$/, "", tp)
    }
  }

  END {
    print sd, lat, tp
  }
')
EOF

echo "$p99_slowdown,$p99_latency,$throughput"