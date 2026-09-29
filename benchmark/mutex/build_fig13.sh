
#!/usr/bin/env bash
set -euo pipefail
SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
source "${SCRIPT_DIR}/../../script/internal-tools/env_fig13.sh"
cd "${SCRIPT_DIR}"

SOURCE_FILE="mutex.c"
MODULE_NAME="mutex"
THREAD_LIST=(4 8 16 32 64)

RESULT_DIR="build"
mkdir -p $RESULT_DIR

cp "$SOURCE_FILE" "${SOURCE_FILE}.bak"
trap 'cp "${SOURCE_FILE}.bak" "$SOURCE_FILE"' EXIT

for i in "${THREAD_LIST[@]}"
do
    sed -i "s/#define THREADS\s\+[0-9]\+/#define THREADS $i/" $SOURCE_FILE

    if make LINUXSRC="${LINUXSRC}"; then
        if [ -f "${MODULE_NAME}.ko" ]; then
            TARGET_NAME="${RESULT_DIR}/mutex_${i}.ko"
            mv "${MODULE_NAME}.ko" "$TARGET_NAME"
        else
            echo "[Fail] Not found ${MODULE_NAME}.ko" >&2
            exit 1
        fi
    else
        echo "[Fail] thread=$i error"
        cp "${SOURCE_FILE}.bak" "$SOURCE_FILE"
        exit 1
    fi
done

cp "${SOURCE_FILE}.bak" "$SOURCE_FILE"