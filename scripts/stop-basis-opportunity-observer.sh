#!/usr/bin/env bash
set -euo pipefail

# 中文说明：停止 start-basis-opportunity-observer.sh 启动的只读观察链路。
# 该脚本只根据 pid 文件停止本地 monitor、机会记录器和 dry-run 验证进程。

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
RUN_ROOT="${BASIS_OBSERVER_ROOT:-${REPO_ROOT}/target/arb-opportunity-observer}"
STATE_DIR="${RUN_ROOT}/state"
PID_FILE="${STATE_DIR}/basis-observer.pids"
STOP_GRACE_SECS="${BASIS_OBSERVER_STOP_GRACE_SECS:-3}"
STOP_DRAIN_SECS="${BASIS_OBSERVER_STOP_DRAIN_SECS:-15}"

usage() {
  cat <<'USAGE'
用法:
  scripts/stop-basis-opportunity-observer.sh

常用环境变量:
  BASIS_OBSERVER_ROOT=target/arb-opportunity-observer
  BASIS_OBSERVER_STOP_GRACE_SECS=3
  BASIS_OBSERVER_STOP_DRAIN_SECS=15

行为:
  读取 state/basis-observer.pids，先停止 recorder，等待已启动的 validation
  子任务 flush report，再停止 monitor；超时后对仍存活的相关 pid 发送 KILL，
  然后归档 pid 文件。
USAGE
}

is_alive() {
  local pid="$1"
  [[ "${pid}" =~ ^[0-9]+$ ]] && kill -0 "${pid}" 2>/dev/null
}

is_validation_process_name() {
  [[ "$1" == validation-* ]]
}

is_core_process_name() {
  case "$1" in
    *-basis-monitor|funding-arb-monitor|opportunity-recorder) return 0 ;;
    *) return 1 ;;
  esac
}

read_pid_lines() {
  PID_LINES=()
  [[ -s "${PID_FILE}" ]] || return 0
  while IFS= read -r line; do
    PID_LINES+=("${line}")
  done < "${PID_FILE}"
}

term_processes_by_name() {
  local expected_name="$1"
  local line
  local pid
  local name
  local log_file
  for line in "${PID_LINES[@]}"; do
    IFS=$'\t' read -r pid name log_file <<< "${line}"
    if [[ "${name}" == "${expected_name}" ]] && is_alive "${pid}"; then
      echo "TERM pid=${pid} name=${name} log=${log_file}"
      kill -TERM "${pid}" 2>/dev/null || true
    fi
  done
}

term_core_processes() {
  local line
  local pid
  local name
  local log_file
  for line in "${PID_LINES[@]}"; do
    IFS=$'\t' read -r pid name log_file <<< "${line}"
    if is_core_process_name "${name}" && is_alive "${pid}"; then
      echo "TERM pid=${pid} name=${name} log=${log_file}"
      kill -TERM "${pid}" 2>/dev/null || true
    fi
  done
}

wait_for_validation_processes() {
  local deadline="$((SECONDS + STOP_DRAIN_SECS))"
  local line
  local pid
  local name
  local log_file
  local alive_count

  while (( SECONDS <= deadline )); do
    alive_count=0
    for line in "${PID_LINES[@]}"; do
      IFS=$'\t' read -r pid name log_file <<< "${line}"
      if is_validation_process_name "${name}" && is_alive "${pid}"; then
        alive_count="$((alive_count + 1))"
      fi
    done
    if (( alive_count == 0 )); then
      return 0
    fi
    echo "waiting for ${alive_count} validation process(es) to flush reports..."
    sleep 1
  done
  return 1
}

terminate_remaining_processes() {
  local signal="$1"
  local line
  local pid
  local name
  local log_file
  for line in "${PID_LINES[@]}"; do
    IFS=$'\t' read -r pid name log_file <<< "${line}"
    if is_alive "${pid}"; then
      echo "${signal#-} pid=${pid} name=${name} log=${log_file}"
      kill "${signal}" "${pid}" 2>/dev/null || true
    fi
  done
}

if [[ "${1:-}" == "-h" || "${1:-}" == "--help" ]]; then
  usage
  exit 0
fi

if [[ ! -s "${PID_FILE}" ]]; then
  echo "no running basis opportunity observer found: ${PID_FILE}"
  exit 0
fi

read_pid_lines

echo "stopping basis opportunity observer from ${PID_FILE}"
term_processes_by_name "opportunity-recorder"
sleep 1

if [[ -s "${PID_FILE}" ]]; then
  read_pid_lines
fi

STOPPED_FILE="${PID_FILE}.stopped.$(date -u +%Y%m%dT%H%M%SZ)"
if [[ -e "${PID_FILE}" ]]; then
  mv "${PID_FILE}" "${STOPPED_FILE}"
else
  printf '%s\n' "${PID_LINES[@]}" > "${STOPPED_FILE}"
fi

term_core_processes
if ! wait_for_validation_processes; then
  echo "validation drain timed out after ${STOP_DRAIN_SECS}s; terminating remaining validation process(es)."
fi

terminate_remaining_processes "-TERM"

sleep "${STOP_GRACE_SECS}"

terminate_remaining_processes "-KILL"

echo "stopped. archived pid file: ${STOPPED_FILE}"
