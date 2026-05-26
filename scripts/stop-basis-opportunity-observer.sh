#!/usr/bin/env bash
set -euo pipefail

# 中文说明：停止 start-basis-opportunity-observer.sh 启动的只读观察链路。
# 该脚本只根据 pid 文件停止本地 monitor、spot-perp-basis 常驻 runner、
# cross-exchange-funding-arb 常驻 runner、机会记录器和 dry-run/实盘验证进程。

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
RUN_ROOT="${BASIS_OBSERVER_ROOT:-${REPO_ROOT}/target/arb-opportunity-observer}"
STATE_DIR="${RUN_ROOT}/state"
PID_FILE="${STATE_DIR}/basis-observer.pids"
RUNTIME_BIN="${BASIS_OBSERVER_RUNTIME_BIN:-${REPO_ROOT}/target/debug/arb-runtime}"
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
  子任务 flush report，再停止 monitor、spot-perp-basis 常驻 runner 和
  cross-exchange-funding-arb 常驻 runner；
  超时后对仍存活的相关 pid 发送 KILL，然后归档 pid 文件。
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
    *-basis-monitor|funding-arb-monitor|opportunity-recorder|spot-perp-basis-resident-live|funding-arb-resident-live|target-wss-*) return 0 ;;
    *) return 1 ;;
  esac
}

append_pid_file_lines() {
  local file="$1"
  [[ -s "${file}" ]] || return 0
  while IFS= read -r line; do
    [[ -n "${line}" ]] && PID_LINES+=("${line}")
  done < "${file}"
}

command_line_matches_rust_recorder() {
  local command_line="$1"

  [[ "${command_line}" == *" opportunity-recorder"* ]] || return 1
  [[ "${command_line}" == *"--root ${RUN_ROOT} "* ||
     "${command_line}" == *"--root ${RUN_ROOT}" ]] || return 1
  if [[ -n "${RUNTIME_BIN}" && "${command_line}" == *"${RUNTIME_BIN} opportunity-recorder"* ]]; then
    return 0
  fi
  [[ "${command_line}" == *"${REPO_ROOT}/target/debug/arb-runtime opportunity-recorder"* ||
     "${command_line}" == *"${REPO_ROOT}/target/release/arb-runtime opportunity-recorder"* ]]
}

append_live_rust_recorder_pid_lines() {
  local pid
  local command_line

  command -v ps >/dev/null 2>&1 || return 0
  while read -r pid command_line; do
    [[ "${pid}" =~ ^[0-9]+$ ]] || continue
    if command_line_matches_rust_recorder "${command_line}"; then
      PID_LINES+=("${pid}"$'\t'"opportunity-recorder"$'\t'"${RUN_ROOT}/logs/recorder.log")
    fi
  done < <(ps axww -o pid= -o command= 2>/dev/null || true)
}

read_pid_lines() {
  PID_LINES=()
  append_pid_file_lines "${PID_FILE}"
}

read_rescue_pid_lines() {
  local file
  local pid
  local name
  local log_file

  PID_LINES=()
  append_pid_file_lines "${PID_FILE}"

  for file in "${STATE_DIR}"/basis-observer.pids.stopped.* "${STATE_DIR}"/basis-observer.pids.stale-*; do
    [[ -e "${file}" ]] || continue
    append_pid_file_lines "${file}"
  done

  for file in "${STATE_DIR}"/target-wss-warmup-*.pid; do
    [[ -s "${file}" ]] || continue
    pid="$(sed -n '1p' "${file}")"
    name="$(basename "${file}" .pid)"
    log_file="${RUN_ROOT}/logs/${name}.log"
    PID_LINES+=("${pid}"$'\t'"${name}"$'\t'"${log_file}")
  done

  append_live_rust_recorder_pid_lines
}

managed_process_matches_name() {
  local pid="$1"
  local name="$2"
  local command_line

  command_line="$(ps -p "${pid}" -o command= 2>/dev/null || true)"
  [[ -n "${command_line}" ]] || return 1

  case "${name}" in
    opportunity-recorder)
      [[ "${command_line}" == *"start-basis-opportunity-observer.sh --recorder"* ]] ||
        command_line_matches_rust_recorder "${command_line}"
      ;;
    validation-*)
      [[ "${command_line}" == *"${REPO_ROOT}"* || "${command_line}" == *"arb-runtime"* ]]
      ;;
    *)
      [[ "${command_line}" == *"${REPO_ROOT}/target/"*"arb-runtime"* || "${command_line}" == *"/target/debug/arb-runtime"* || "${command_line}" == *"/target/release/arb-runtime"* ]]
      ;;
  esac
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

terminate_rescue_processes() {
  local signal="$1"
  local line
  local pid
  local name
  local log_file
  local seen=" "

  read_rescue_pid_lines
  for line in "${PID_LINES[@]}"; do
    IFS=$'\t' read -r pid name log_file <<< "${line}"
    [[ "${pid}" =~ ^[0-9]+$ ]] || continue
    [[ "${seen}" == *" ${pid} "* ]] && continue
    seen+="${pid} "
    is_core_process_name "${name}" || continue
    if is_alive "${pid}" && managed_process_matches_name "${pid}" "${name}"; then
      echo "${signal#-} rescue pid=${pid} name=${name} log=${log_file}"
      kill "${signal}" "${pid}" 2>/dev/null || true
    fi
  done
}

refresh_funding_arb_summary_position_counts() {
  local summary_path="$1"
  local positions_path
  local counts_json
  local tmp

  [[ "${summary_path}" == */funding_arb_resident_live_summary.json ]] || return 0
  positions_path="${summary_path%/funding_arb_resident_live_summary.json}/funding_arb_resident_positions.jsonl"
  [[ -s "${positions_path}" && -s "${summary_path}" ]] || return 0
  command -v jq >/dev/null 2>&1 || return 0

  counts_json="$(
    jq -s -c '
      (
      reduce .[] as $event ({};
        ($event.position_id // "") as $position_id
        | if $position_id == "" then .
          elif ($event.event_type // "") == "position_opened" then .[$position_id] = "open"
          elif ($event.event_type // "") == "position_unknown" then .[$position_id] = "unknown"
          elif ($event.event_type // "") == "position_closed" then .[$position_id] = "closed"
          elif ($event.event_type // "") == "position_flat_cancelled" then .[$position_id] = "flat_cancelled"
          else .
          end
      )
      ) as $positions
      | {
          open_position_count: ([$positions[] | select(. == "open")] | length),
          closed_position_count: ([$positions[] | select(. == "closed")] | length),
          unknown_position_count: ([$positions[] | select(. == "unknown")] | length),
          live_entry_count: ([$positions[] | select(. != "unknown")] | length)
        }
    ' "${positions_path}"
  )" || return 0

  tmp="${summary_path}.tmp.$$"
  jq --argjson counts "${counts_json}" '
    .open_position_count = $counts.open_position_count
    | .closed_position_count = $counts.closed_position_count
    | .unknown_position_count = $counts.unknown_position_count
    | .live_entry_count = $counts.live_entry_count
  ' "${summary_path}" > "${tmp}" && mv "${tmp}" "${summary_path}"
}

mark_running_resident_artifacts_stopped() {
  local state_path="$1"
  local summary_path="$2"
  local label="$3"
  local reason="stopped by stop-basis-opportunity-observer.sh"
  local updated_at
  local cycles
  local tmp

  [[ -s "${state_path}" ]] || return 0
  command -v jq >/dev/null 2>&1 || return 0
  if ! jq -e '(.phase // "") == "running"' "${state_path}" >/dev/null 2>&1; then
    return 0
  fi

  updated_at="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
  cycles="$(jq -r '(.cycles // 0) | tonumber' "${state_path}" 2>/dev/null || printf '0')"
  tmp="${state_path}.tmp.$$"
  jq --arg reason "${reason}" --arg updated_at "${updated_at}" \
    '.phase = "stopped" | .halt_reason = $reason | .updated_at = $updated_at' \
    "${state_path}" > "${tmp}" && mv "${tmp}" "${state_path}"

  if [[ -s "${summary_path}" ]]; then
    tmp="${summary_path}.tmp.$$"
    jq --arg reason "${reason}" --argjson cycles "${cycles}" \
      '.phase = "stopped" | .cycles = $cycles | .halt_reason = $reason' \
      "${summary_path}" > "${tmp}" && mv "${tmp}" "${summary_path}"
    refresh_funding_arb_summary_position_counts "${summary_path}"
  fi
  echo "resident_state_marked_stopped label=${label} state=${state_path}"
}

mark_resident_artifacts_stopped() {
  mark_running_resident_artifacts_stopped \
    "${RUN_ROOT}/resident-live/spot-perp-basis/multi_venue_resident_live_state.json" \
    "${RUN_ROOT}/resident-live/spot-perp-basis/multi_venue_resident_live_summary.json" \
    "spot-perp-basis"
  mark_running_resident_artifacts_stopped \
    "${RUN_ROOT}/resident-live/cross-exchange-funding-arb/funding_arb_resident_live_state.json" \
    "${RUN_ROOT}/resident-live/cross-exchange-funding-arb/funding_arb_resident_live_summary.json" \
    "cross-exchange-funding-arb"
}

archive_stopped_resident_lock() {
  local lock_path="$1"
  local label="$2"
  local archived_lock

  [[ -e "${lock_path}" ]] || return 0
  archived_lock="${lock_path}.stopped.$(date -u +%Y%m%dT%H%M%SZ).$$"
  mv "${lock_path}" "${archived_lock}"
  echo "resident_lock_archived label=${label} reason=observer_stop lock=${lock_path} archived=${archived_lock}"
}

archive_stopped_resident_locks() {
  archive_stopped_resident_lock \
    "${RUN_ROOT}/resident-live/spot-perp-basis/multi_venue_resident_live.lock" \
    "spot-perp-basis"
  archive_stopped_resident_lock \
    "${RUN_ROOT}/resident-live/cross-exchange-funding-arb/funding_arb_resident_live.lock" \
    "cross-exchange-funding-arb"
}

if [[ "${1:-}" == "-h" || "${1:-}" == "--help" ]]; then
  usage
  exit 0
fi

if [[ ! -s "${PID_FILE}" ]]; then
  echo "no running basis opportunity observer found: ${PID_FILE}"
  terminate_rescue_processes "-TERM"
  sleep "${STOP_GRACE_SECS}"
  terminate_rescue_processes "-KILL"
  mark_resident_artifacts_stopped
  archive_stopped_resident_locks
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
terminate_rescue_processes "-TERM"

sleep "${STOP_GRACE_SECS}"

terminate_remaining_processes "-KILL"
terminate_rescue_processes "-KILL"
mark_resident_artifacts_stopped
archive_stopped_resident_locks

echo "stopped. archived pid file: ${STOPPED_FILE}"
