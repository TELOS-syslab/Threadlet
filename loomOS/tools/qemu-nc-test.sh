#!/bin/bash
NUM_CLIENTS=5          # Client 数量 (i=0..4)
PACKET_COUNT=20        # 每个 Client 发送次数
BASE_PORT=11181        # 基础端口
HOST="127.0.0.1"
INTERVAL=0.1           # 发送间隔

TOTAL_CORES=$(nproc)

echo "Starting $NUM_CLIENTS UDP clients..."
echo "Each sending $PACKET_COUNT packets."
echo "CPU Affinity: Distributing across $TOTAL_CORES available cores."
echo "----------------------------------------"

# --- 循环启动 Client ---
for ((i=0; i<NUM_CLIENTS; i++)); do
    TARGET_PORT=$((BASE_PORT + i))
    
    CORE_ID=$((i % TOTAL_CORES))
    
    taskset -c $CORE_ID bash -c "
        for ((c=1; c<=$PACKET_COUNT; c++)); do
            echo -n 'hello from client $i' | nc -u -w1 $HOST $TARGET_PORT
            sleep $INTERVAL
        done
        echo '[Client $i] Finished on Core $CORE_ID'
    " & 
    
    echo "[Client $i] Launched on Core $CORE_ID -> Target $HOST:$TARGET_PORT"
done

echo "----------------------------------------"
echo "Clients are running. Waiting for completion..."

wait

echo "----------------------------------------"
echo "All clients finished."