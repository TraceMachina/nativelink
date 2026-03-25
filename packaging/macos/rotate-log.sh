#!/bin/bash
# Log rotation for NativeLink.
# Runs daily via launchd. Truncates in place so launchd's file descriptor
# stays valid — no service restart needed.
set -euo pipefail

LOGFILE="${HOME}/Library/Logs/nativelink.log"
MAX_BYTES=$((10 * 1024 * 1024))  # 10 MB
KEEP=5

[ ! -f "$LOGFILE" ] && exit 0

SIZE=$(stat -f%z "$LOGFILE" 2>/dev/null || echo 0)
[ "$SIZE" -lt "$MAX_BYTES" ] && exit 0

# Shift compressed archives (oldest first)
rm -f "${LOGFILE}.${KEEP}.gz"
for ((i=KEEP-1; i>=1; i--)); do
    [ -f "${LOGFILE}.${i}.gz" ] && mv "${LOGFILE}.${i}.gz" "${LOGFILE}.$((i+1)).gz"
done

# Compress current log, then truncate in place
gzip -c "$LOGFILE" > "${LOGFILE}.1.gz"
: > "$LOGFILE"
