#!/bin/bash

# --- 配置目标 ---
TARGET_IP="172.16.0.2"
TARGET_PORT=11133
# --- ---

STRING_LENGTH=27
SLEEP_TIME=0.3

echo "开始发送数据到 $TARGET_IP，端口从 $TARGET_PORT 开始递增..."

for char in {a..n}
do
    current_string=$(printf "$char%.0s" $(seq 1 $STRING_LENGTH))
    echo "send: $current_string to port: $TARGET_PORT"
    echo -n "$current_string" | nc -u -w1 "$TARGET_IP" "$TARGET_PORT"
    ((TARGET_PORT++))
    sleep $SLEEP_TIME
done
