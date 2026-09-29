#!/bin/bash

SOURCE_FILE="switch.c"
MODULE_NAME="switch"
THREADS_LIST=(1 2 4 8 16 32 64)

RESULT_DIR="build"
mkdir -p $RESULT_DIR

cp $SOURCE_FILE "${SOURCE_FILE}.bak"

for i in "${THREADS_LIST[@]}"
do
    sed -i "s/#define THREADS\s\+[0-9]\+/#define THREADS $i/" $SOURCE_FILE

    if make > /dev/null 2>&1; then
        if [ -f "${MODULE_NAME}.ko" ]; then
            TARGET_NAME="${RESULT_DIR}/switch_${i}.ko"
            mv "${MODULE_NAME}.ko" "$TARGET_NAME"
        else
            echo "[Fail] Not found ${MODULE_NAME}.ko"
        fi
    else
        echo "[Fail] THREADS=$i error"
        mv "${SOURCE_FILE}.bak" $SOURCE_FILE
        exit 1
    fi
done
