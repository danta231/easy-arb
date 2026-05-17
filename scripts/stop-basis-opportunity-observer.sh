#!/usr/bin/env bash
set -euo pipefail

# 中文说明：停止 start-basis-opportunity-observer.sh 启动的只读观察链路。
# 该脚本只根据 pid 文件停止本地 monitor、机会记录器和 dry-run 验证进程。

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
RUN_ROOT="${BASIS_OBSERVER_ROOT:-${REPO_ROOT}/target/basis-opportunity-observer}"
STATE_DIR="${RUN_ROOT}/state"
PID_FILE="${STATE_DIR}/basis-observer.pids"
STOP_GRACE_SECS="${BASIS_OBSERVER_STOP_GRACE_SECS:-3}"

usage() {
  cat <<'USAGE'
用法:
  scripts/stop-basis-opportunity-observer.sh

常用环境变量:
  BASIS_OBSERVER_ROOT=target/basis-opportunity-observer
  BASIS_OBSERVER_STOP_GRACE_SECS=3

行为:
  读取 state/basis-observer.pids，先发送 TERM，等待宽限时间后对仍存活的
  相关 pid 发送 KILL，然后归档 pid 文件。
USAGE
}

is_alive() {
  local pid="$1"
  [[ "${pid}" =~ ^[0-9]+$ ]] && kill -0 "${pid}" 2>/dev/null
}

if [[ "${1:-}" == "-h" || "${1:-}" == "--help" ]]; then
  usage
  exit 0
fi

if [[ ! -s "${PID_FILE}" ]]; then
  echo "no running basis opportunity observer found: ${PID_FILE}"
  exit 0
fi

PID_LINES=()
while IFS= read -r line; do
  PID_LINES+=("${line}")
done < "${PID_FILE}"

echo "stopping basis opportunity observer from ${PID_FILE}"
for line in "${PID_LINES[@]}"; do
  IFS=$'\t' read -r pid name log_file <<< "${line}"
  if is_alive "${pid}"; then
    echo "TERM pid=${pid} name=${name} log=${log_file}"
    kill -TERM "${pid}" 2>/dev/null || true
  fi
done

sleep "${STOP_GRACE_SECS}"

for line in "${PID_LINES[@]}"; do
  IFS=$'\t' read -r pid name log_file <<< "${line}"
  if is_alive "${pid}"; then
    echo "KILL pid=${pid} name=${name} log=${log_file}"
    kill -KILL "${pid}" 2>/dev/null || true
  fi
done

STOPPED_FILE="${PID_FILE}.stopped.$(date -u +%Y%m%dT%H%M%SZ)"
mv "${PID_FILE}" "${STOPPED_FILE}"
echo "stopped. archived pid file: ${STOPPED_FILE}"
