#!/usr/bin/env bash
set -euo pipefail

# 中文说明：停止 start-arb-runtime-live.sh 启动的正式实盘链路和 WSS 前置 monitor。

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
RUN_ROOT="${ARB_RUNTIME_LIVE_ROOT:-${REPO_ROOT}/target/arb-runtime/live}"
PREREQ_ROOT="${ARB_RUNTIME_LIVE_PREREQ_ROOT:-${REPO_ROOT}/target/arb-runtime/live-prereq}"
STATE_DIR="${PREREQ_ROOT}/state"
WSS_PID_FILE="${STATE_DIR}/wss-book-ticker.pids"
LIVE_PID_FILE="${STATE_DIR}/arb-runtime-live.pid"
PORTFOLIO_PID_FILE="${STATE_DIR}/portfolio-dashboard.pid"
STOP_GRACE_SECS="${ARB_RUNTIME_LIVE_STOP_GRACE_SECS:-3}"

usage() {
  cat <<'USAGE'
用法:
  scripts/stop-arb-runtime-live.sh

常用环境变量:
  ARB_RUNTIME_LIVE_ROOT=target/arb-runtime/live
  ARB_RUNTIME_LIVE_PREREQ_ROOT=target/arb-runtime/live-prereq
  ARB_RUNTIME_LIVE_STOP_GRACE_SECS=3
USAGE
}

is_alive() {
  local pid="$1"
  [[ "${pid}" =~ ^[0-9]+$ ]] && kill -0 "${pid}" 2>/dev/null
}

stop_live_parent() {
  [[ -s "${LIVE_PID_FILE}" ]] || return 0
  local pid
  pid="$(sed -n '1p' "${LIVE_PID_FILE}")"
  if is_alive "${pid}"; then
    echo "TERM arb-runtime-live pid=${pid}"
    kill -TERM "${pid}" 2>/dev/null || true
    sleep "${STOP_GRACE_SECS}"
    if is_alive "${pid}"; then
      echo "KILL arb-runtime-live pid=${pid}"
      kill -KILL "${pid}" 2>/dev/null || true
    fi
  fi
  mv "${LIVE_PID_FILE}" "${LIVE_PID_FILE}.stopped.$(date -u +%Y%m%dT%H%M%SZ)" 2>/dev/null || true
}

stop_wss_monitors() {
  if [[ ! -s "${WSS_PID_FILE}" ]]; then
    echo "no WSS monitor pid file found: ${WSS_PID_FILE}"
    return 0
  fi

  local pid
  local name
  local log_file
  local status_url
  local ready_symbol
  local required
  while IFS=$'\t' read -r pid name log_file status_url ready_symbol required; do
    if is_alive "${pid}"; then
      echo "TERM WSS pid=${pid} name=${name} log=${log_file}"
      kill -TERM "${pid}" 2>/dev/null || true
    fi
  done < "${WSS_PID_FILE}"

  sleep "${STOP_GRACE_SECS}"

  while IFS=$'\t' read -r pid name log_file status_url ready_symbol required; do
    if is_alive "${pid}"; then
      echo "KILL WSS pid=${pid} name=${name} log=${log_file}"
      kill -KILL "${pid}" 2>/dev/null || true
    fi
  done < "${WSS_PID_FILE}"

  mv "${WSS_PID_FILE}" "${WSS_PID_FILE}.stopped.$(date -u +%Y%m%dT%H%M%SZ)" 2>/dev/null || true
}

stop_portfolio_dashboard() {
  if [[ ! -s "${PORTFOLIO_PID_FILE}" ]]; then
    return 0
  fi
  local pid
  pid="$(sed -n '1p' "${PORTFOLIO_PID_FILE}")"
  if is_alive "${pid}"; then
    echo "TERM portfolio-dashboard pid=${pid}"
    kill -TERM "${pid}" 2>/dev/null || true
    sleep "${STOP_GRACE_SECS}"
    if is_alive "${pid}"; then
      echo "KILL portfolio-dashboard pid=${pid}"
      kill -KILL "${pid}" 2>/dev/null || true
    fi
  fi
  mv "${PORTFOLIO_PID_FILE}" "${PORTFOLIO_PID_FILE}.stopped.$(date -u +%Y%m%dT%H%M%SZ)" 2>/dev/null || true
}

if [[ "${1:-}" == "-h" || "${1:-}" == "--help" ]]; then
  usage
  exit 0
fi

echo "stopping arb-runtime live observer..."
BASIS_OBSERVER_ROOT="${RUN_ROOT}" "${SCRIPT_DIR}/stop-basis-opportunity-observer.sh" || true

stop_live_parent
stop_portfolio_dashboard
stop_wss_monitors

echo "stopped arb-runtime live stack."
