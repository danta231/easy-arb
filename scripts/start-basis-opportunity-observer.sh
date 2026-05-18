#!/usr/bin/env bash
set -euo pipefail

# 中文说明：启动六交易所套利机会观察链路。
# 该脚本只启动公开行情监控和 dry-run 预下单验证，不传 --execute-live，
# 不提交订单、不撤单、不转账，不要求或记录任何密钥。
# 当前会主动轮询 spot-perp-basis monitor，并启动专用 funding-arb-monitor
# 聚合本机 basis status 快照，生成 cross-exchange-funding-arb 机会文件。

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"

usage() {
  cat <<'USAGE'
用法:
  scripts/start-basis-opportunity-observer.sh [binance] [bybit] [okx] [bitget] [aster] [hyperliquid]
  scripts/start-basis-opportunity-observer.sh --venues binance,bybit,okx,bitget,aster,hyperliquid --strategies spot-perp-basis,cross-exchange-funding-arb --dry-run

默认启动 binance、bybit、okx、bitget、aster、hyperliquid 六条公开行情链路。

核心行为:
  1. 启动六条只读 basis monitor，持续刷新公开行情。
  2. 轮询 /opportunities，实时记录 candidate_count > 0 的 spot-perp-basis 机会。
  3. 对支持 auto-once 的 /opportunities 候选 symbol 触发 guarded-live-auto-once --dry-run，
     生成候选、风险决策、人工门禁释放预览、分发计划等下单前 artifacts。
  4. 如果启用 cross-exchange-funding-arb，启动专用 funding-arb-monitor，
     聚合本机 basis /status 快照并记录真实候选，不伪造机会。
  5. 永远不传 --execute-live 和 --i-understand-basis-live-orders。

常用环境变量:
  BASIS_OBSERVER_ROOT=target/arb-opportunity-observer
  BASIS_OBSERVER_STRATEGIES=spot-perp-basis,cross-exchange-funding-arb
  BASIS_OBSERVER_MONITORS="binance bybit okx bitget aster hyperliquid"
  BASIS_OBSERVER_INTERVAL_SECS=5
  BASIS_OBSERVER_MIN_NET_BPS=5
  BASIS_OBSERVER_MIN_ABS_FUNDING_RATE=0
  BASIS_OBSERVER_NOTIONAL_USD=100.00
  BASIS_OBSERVER_SPOT_FEE_BPS=10
  BASIS_OBSERVER_PERP_FEE_BPS=5
  BASIS_OBSERVER_SLIPPAGE_BPS=5
  BASIS_OBSERVER_CONFIG=templates/personal_guarded_live.preflight.yaml
  BASIS_OBSERVER_VALIDATE_AUTO_ONCE=1
  BASIS_OBSERVER_AUTO_ONCE_COOLDOWN_SECS=60
  BASIS_OBSERVER_CURL_RETRIES=3
  BASIS_OBSERVER_CURL_RETRY_SLEEP_SECS=1
  BASIS_OBSERVER_CURL_TIMEOUT_SECS=10
  BASIS_OBSERVER_STARTUP_CHECK=1
  BASIS_OBSERVER_STARTUP_WAIT_SECS=180
  BASIS_OBSERVER_STOP_DRAIN_SECS=15
  BASIS_OBSERVER_STOP_GRACE_SECS=3
  BASIS_OBSERVER_FOREGROUND=0
  BINANCE_BASIS_BIND=127.0.0.1:8796
  BYBIT_BASIS_BIND=127.0.0.1:8797
  OKX_BASIS_BIND=127.0.0.1:8798
  BITGET_BASIS_BIND=127.0.0.1:8803
  ASTER_BASIS_BIND=127.0.0.1:8800
  HYPERLIQUID_BASIS_BIND=127.0.0.1:8799
  FUNDING_ARB_BIND=127.0.0.1:8804
  FUNDING_ARB_MAX_ENTRY_PRICE_DIVERGENCE_BPS=20

可选 WSS monitor URL:
  BINANCE_SPOT_WSS_MONITOR_URL=http://127.0.0.1:8786/api/binance-wss-book-ticker/status
  BINANCE_PERP_WSS_MONITOR_URL=http://127.0.0.1:8787/api/binance-wss-book-ticker/status
  BYBIT_SPOT_WSS_MONITOR_URL=http://127.0.0.1:8788/api/bybit-wss-book-ticker/status
  BYBIT_PERP_WSS_MONITOR_URL=http://127.0.0.1:8789/api/bybit-wss-book-ticker/status
  OKX_SPOT_WSS_MONITOR_URL=http://127.0.0.1:8790/api/okx-wss-book-ticker/status
  OKX_PERP_WSS_MONITOR_URL=http://127.0.0.1:8791/api/okx-wss-book-ticker/status
  BITGET_SPOT_WSS_MONITOR_URL=http://127.0.0.1:8792/api/bitget-wss-book-ticker/status
  BITGET_PERP_WSS_MONITOR_URL=http://127.0.0.1:8793/api/bitget-wss-book-ticker/status

输出:
  target/arb-opportunity-observer/logs/realtime-feedback.log
  target/arb-opportunity-observer/opportunities/spot-perp-basis.jsonl
  target/arb-opportunity-observer/opportunities/cross-exchange-funding-arb.jsonl
  target/arb-opportunity-observer/dry-run/dry-run-reports.jsonl
  target/arb-opportunity-observer/dry-run/<run-id>/
USAGE
}

die() {
  echo "error: $*" >&2
  exit 1
}

require_command() {
  command -v "$1" >/dev/null 2>&1 || die "missing required command: $1"
}

is_alive() {
  local pid="$1"
  [[ "${pid}" =~ ^[0-9]+$ ]] && kill -0 "${pid}" 2>/dev/null
}

opportunities_url() {
  case "$1" in
    binance) printf 'http://%s/api/basis/opportunities' "${BINANCE_BIND}" ;;
    bybit) printf 'http://%s/api/bybit-basis/opportunities' "${BYBIT_BIND}" ;;
    okx) printf 'http://%s/api/okx-basis/opportunities' "${OKX_BIND}" ;;
    bitget) printf 'http://%s/api/bitget-basis/opportunities' "${BITGET_BIND}" ;;
    aster) printf 'http://%s/api/aster-basis/opportunities' "${ASTER_BIND}" ;;
    hyperliquid) printf 'http://%s/api/hyperliquid-basis/opportunities' "${HYPERLIQUID_BIND}" ;;
    *) return 1 ;;
  esac
}

status_url() {
  case "$1" in
    binance) printf 'http://%s/api/basis/status' "${BINANCE_BIND}" ;;
    bybit) printf 'http://%s/api/bybit-basis/status' "${BYBIT_BIND}" ;;
    okx) printf 'http://%s/api/okx-basis/status' "${OKX_BIND}" ;;
    bitget) printf 'http://%s/api/bitget-basis/status' "${BITGET_BIND}" ;;
    aster) printf 'http://%s/api/aster-basis/status' "${ASTER_BIND}" ;;
    hyperliquid) printf 'http://%s/api/hyperliquid-basis/status' "${HYPERLIQUID_BIND}" ;;
    *) return 1 ;;
  esac
}

funding_arb_opportunities_url() {
  printf 'http://%s/api/funding-arb/opportunities' "${FUNDING_ARB_BIND}"
}

strategy_enabled() {
  local needle="$1"
  local list="${STRATEGIES:-${EFFECTIVE_STRATEGIES:-}}"
  local item
  local -a _strategy_items
  IFS=',' read -r -a _strategy_items <<< "${list}"
  for item in "${_strategy_items[@]}"; do
    item="${item//[[:space:]]/}"
    if [[ "${item}" == "${needle}" ]]; then
      return 0
    fi
  done
  return 1
}

supports_auto_once_dry_run() {
  case "$1" in
    binance|bybit|okx|bitget) return 0 ;;
    aster|hyperliquid) return 1 ;;
    *) return 1 ;;
  esac
}

auto_once_command() {
  case "$1" in
    binance) printf 'binance-basis-guarded-live-auto-once' ;;
    bybit) printf 'bybit-basis-guarded-live-auto-once' ;;
    okx) printf 'okx-basis-guarded-live-auto-once' ;;
    bitget) printf 'bitget-basis-guarded-live-auto-once' ;;
    *) return 1 ;;
  esac
}

wss_args_for_venue() {
  local venue="$1"
  local spot_var=""
  local perp_var=""
  case "${venue}" in
    binance)
      spot_var="${BINANCE_SPOT_WSS_MONITOR_URL:-}"
      perp_var="${BINANCE_PERP_WSS_MONITOR_URL:-}"
      ;;
    bybit)
      spot_var="${BYBIT_SPOT_WSS_MONITOR_URL:-}"
      perp_var="${BYBIT_PERP_WSS_MONITOR_URL:-}"
      ;;
    okx)
      spot_var="${OKX_SPOT_WSS_MONITOR_URL:-}"
      perp_var="${OKX_PERP_WSS_MONITOR_URL:-}"
      ;;
    bitget)
      spot_var="${BITGET_SPOT_WSS_MONITOR_URL:-}"
      perp_var="${BITGET_PERP_WSS_MONITOR_URL:-}"
      ;;
    aster|hyperliquid)
      return 0
      ;;
    *) return 1 ;;
  esac

  if [[ -n "${spot_var}" || -n "${perp_var}" ]]; then
    if [[ -z "${spot_var}" || -z "${perp_var}" ]]; then
      die "${venue} WSS URL must provide both spot and perp/swap monitor URLs"
    fi
    printf '%s\n%s\n' "${spot_var}" "${perp_var}"
  fi
}

append_json_line() {
  local file="$1"
  shift
  jq -cn "$@" >> "${file}"
}

symbol_slug() {
  jq -rn --arg symbol "$1" '$symbol | @uri'
}

fetch_url_with_retries() {
  local url="$1"
  local attempt=1
  local body

  while (( attempt <= CURL_RETRIES )); do
    if body="$(curl -fsS --max-time "${CURL_TIMEOUT_SECS}" "${url}" 2>> "${LOG_DIR}/curl-errors.log")"; then
      printf '%s\n' "${body}"
      return 0
    fi
    if (( attempt < CURL_RETRIES )); then
      sleep "${CURL_RETRY_SLEEP_SECS}"
    fi
    attempt="$((attempt + 1))"
  done

  return 1
}

pid_for_name() {
  local name="$1"
  local pid
  local entry_name
  local log_file
  [[ -s "${PID_FILE}" ]] || return 0
  while IFS=$'\t' read -r pid entry_name log_file; do
    if [[ "${entry_name}" == "${name}" ]]; then
      printf '%s\n' "${pid}"
    fi
  done < "${PID_FILE}" | tail -n 1
}

log_for_name() {
  local name="$1"
  local pid
  local entry_name
  local log_file
  [[ -s "${PID_FILE}" ]] || return 0
  while IFS=$'\t' read -r pid entry_name log_file; do
    if [[ "${entry_name}" == "${name}" ]]; then
      printf '%s\n' "${log_file}"
    fi
  done < "${PID_FILE}" | tail -n 1
}

stop_started_processes() {
  local pid
  [[ -s "${PID_FILE}" ]] || return 0
  while IFS=$'\t' read -r pid _name _log; do
    if is_alive "${pid}"; then
      kill "${pid}" 2>/dev/null || true
    fi
  done < "${PID_FILE}"
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

stop_core_processes() {
  local pid
  local name
  local log_file
  [[ -s "${PID_FILE}" ]] || return 0
  while IFS=$'\t' read -r pid name log_file; do
    if is_core_process_name "${name}" && is_alive "${pid}"; then
      kill "${pid}" 2>/dev/null || true
    fi
  done < "${PID_FILE}"
}

wait_for_validation_processes() {
  local timeout_secs="${STOP_DRAIN_SECS:-15}"
  local deadline="$((SECONDS + timeout_secs))"
  local pid
  local name
  local log_file
  local alive_count

  [[ -s "${PID_FILE}" ]] || return 0
  while (( SECONDS <= deadline )); do
    alive_count=0
    while IFS=$'\t' read -r pid name log_file; do
      if is_validation_process_name "${name}" && is_alive "${pid}"; then
        alive_count="$((alive_count + 1))"
      fi
    done < "${PID_FILE}"
    if (( alive_count == 0 )); then
      return 0
    fi
    echo "waiting for ${alive_count} validation process(es) to flush reports..."
    sleep 1
  done
  return 1
}

kill_remaining_started_processes() {
  local pid
  local name
  local log_file
  [[ -s "${PID_FILE}" ]] || return 0
  while IFS=$'\t' read -r pid name log_file; do
    if is_alive "${pid}"; then
      kill -TERM "${pid}" 2>/dev/null || true
    fi
  done < "${PID_FILE}"
  sleep "${STOP_GRACE_SECS:-3}"
  while IFS=$'\t' read -r pid name log_file; do
    if is_alive "${pid}"; then
      kill -KILL "${pid}" 2>/dev/null || true
    fi
  done < "${PID_FILE}"
}

graceful_stop_started_processes() {
  stop_core_processes
  if ! wait_for_validation_processes; then
    echo "validation drain timed out after ${STOP_DRAIN_SECS:-15}s; terminating remaining validation process(es)."
  fi
  kill_remaining_started_processes
}

supervise_started_processes() {
  local pid
  local name
  local log_file
  local failed

  trap 'echo "stopping foreground basis opportunity observer..."; graceful_stop_started_processes; rm -f "${PID_FILE}"; exit 0' INT TERM
  echo "foreground supervision enabled; press Ctrl-C to stop."

  while true; do
    if [[ ! -s "${PID_FILE}" ]]; then
      echo "pid file removed; foreground supervisor exiting."
      exit 0
    fi

    failed=0
    while IFS=$'\t' read -r pid name log_file; do
      case "${name}" in
        *-basis-monitor|funding-arb-monitor|opportunity-recorder)
          if ! is_alive "${pid}"; then
            echo "error: supervised process exited: ${name} pid=${pid}" >&2
            if [[ -n "${log_file}" && -f "${log_file}" ]]; then
              tail -n 40 "${log_file}" >&2 || true
            fi
            failed=1
          fi
          ;;
      esac
    done < "${PID_FILE}"

    if (( failed != 0 )); then
      stop_started_processes
      rm -f "${PID_FILE}"
      exit 1
    fi

    sleep "${INTERVAL_SECS}"
  done
}

wait_for_monitor_opportunities() {
  local venue="$1"
  local process_name="${venue}-basis-monitor"
  local pid
  local log_file
  local url
  local deadline
  local body
  local last_body=""
  local snapshot_status="unknown"

  pid="$(pid_for_name "${process_name}")"
  log_file="$(log_for_name "${process_name}")"
  url="$(opportunities_url "${venue}")"
  deadline="$((SECONDS + STARTUP_WAIT_SECS))"

  while (( SECONDS <= deadline )); do
    if [[ -n "${pid}" ]] && ! is_alive "${pid}"; then
      echo "error: ${venue} monitor exited before /opportunities became healthy" >&2
      if [[ -n "${log_file}" && -f "${log_file}" ]]; then
        tail -n 40 "${log_file}" >&2 || true
      fi
      return 1
    fi

    if body="$(curl -fsS --max-time "${CURL_TIMEOUT_SECS}" "${url}" 2>> "${LOG_DIR}/curl-errors.log")"; then
      last_body="${body}"
      snapshot_status="$(printf '%s\n' "${body}" | jq -r '.status // "unknown"' 2>> "${LOG_DIR}/jq-errors.log" || printf 'unknown')"
      if printf '%s\n' "${body}" | jq -e 'has("status") and .status == "healthy" and has("candidate_count") and ((.rows // []) | type == "array")' >/dev/null 2>> "${LOG_DIR}/jq-errors.log"; then
        echo "startup_check_ok venue=${venue} endpoint=${url}"
        return 0
      fi
    fi

    sleep 1
  done

  echo "error: ${venue} monitor did not provide a healthy /opportunities response within ${STARTUP_WAIT_SECS}s: ${url}; last_status=${snapshot_status}" >&2
  if [[ -n "${last_body}" ]]; then
    printf '%s\n' "${last_body}" | jq -c '{status:(.status // "unknown"),candidate_count:(.candidate_count // 0),updated_at:(.updated_at // "unknown"),rows:((.rows // []) | length)}' >&2 2>> "${LOG_DIR}/jq-errors.log" || true
  fi
  if [[ -n "${log_file}" && -f "${log_file}" ]]; then
    tail -n 40 "${log_file}" >&2 || true
  fi
  return 1
}

wait_for_funding_arb_opportunities() {
  local process_name="funding-arb-monitor"
  local pid
  local log_file
  local url
  local deadline
  local body
  local last_body=""
  local snapshot_status="unknown"

  pid="$(pid_for_name "${process_name}")"
  log_file="$(log_for_name "${process_name}")"
  url="$(funding_arb_opportunities_url)"
  deadline="$((SECONDS + STARTUP_WAIT_SECS))"

  while (( SECONDS <= deadline )); do
    if [[ -n "${pid}" ]] && ! is_alive "${pid}"; then
      echo "error: funding arb monitor exited before /opportunities became healthy" >&2
      if [[ -n "${log_file}" && -f "${log_file}" ]]; then
        tail -n 40 "${log_file}" >&2 || true
      fi
      return 1
    fi

    if body="$(curl -fsS --max-time "${CURL_TIMEOUT_SECS}" "${url}" 2>> "${LOG_DIR}/curl-errors.log")"; then
      last_body="${body}"
      snapshot_status="$(printf '%s\n' "${body}" | jq -r '.status // "unknown"' 2>> "${LOG_DIR}/jq-errors.log" || printf 'unknown')"
      if printf '%s\n' "${body}" | jq -e 'has("status") and .status == "healthy" and has("candidate_count") and ((.rows // []) | type == "array")' >/dev/null 2>> "${LOG_DIR}/jq-errors.log"; then
        echo "startup_check_ok strategy=cross-exchange-funding-arb endpoint=${url}"
        return 0
      fi
    fi

    sleep 1
  done

  echo "error: funding arb monitor did not provide a healthy /opportunities response within ${STARTUP_WAIT_SECS}s: ${url}; last_status=${snapshot_status}" >&2
  if [[ -n "${last_body}" ]]; then
    printf '%s\n' "${last_body}" | jq -c '{status:(.status // "unknown"),candidate_count:(.candidate_count // 0),updated_at:(.updated_at // "unknown"),rows:((.rows // []) | length)}' >&2 2>> "${LOG_DIR}/jq-errors.log" || true
  fi
  if [[ -n "${log_file}" && -f "${log_file}" ]]; then
    tail -n 40 "${log_file}" >&2 || true
  fi
  return 1
}

run_validation_job() {
  set +e
  local venue="$1"
  local symbol="$2"
  local run_id="$3"
  local out_dir="$4"
  local job_log="$5"
  local started_at
  local finished_at
  local status
  local command_name
  local report_file
  local validation_result_class="command_failed"
  local spot_wss=""
  local perp_wss=""
  local wss_values
  local args

  command_name="$(auto_once_command "${venue}")"
  report_file="${out_dir}/basis_auto_once_report.json"
  started_at="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
  mkdir -p "${out_dir}"

  {
    echo "[${started_at}] validation_start venue=${venue} symbol=${symbol} run_id=${run_id}"

    args=(
      "${RUNTIME_BIN}"
      "${command_name}"
      --symbol "${symbol}"
      --config "${CONFIG_PATH}"
      --out "${out_dir}"
      --min-net-bps "${MIN_NET_BPS}"
      --dry-run
    )

    wss_values="$(wss_args_for_venue "${venue}")"
    if [[ -n "${wss_values}" ]]; then
      spot_wss="$(printf '%s\n' "${wss_values}" | sed -n '1p')"
      perp_wss="$(printf '%s\n' "${wss_values}" | sed -n '2p')"
      args+=(--spot-wss-monitor-url "${spot_wss}" --perp-wss-monitor-url "${perp_wss}")
    fi

    "${args[@]}"
    status="$?"
    finished_at="$(date -u +%Y-%m-%dT%H:%M:%SZ)"

    if [[ -s "${report_file}" ]]; then
      validation_result_class="$(jq -r --argjson exit_status "${status}" '
        if $exit_status != 0 and any(.blocking_reasons[]?; startswith("input_parse_failed")) then
          "input_parse_failed"
        elif .dispatch_plan_built == true and (.dispatch_request_count // 0) == 2 then
          "pre_trade_flow_complete"
        elif .manual_gate_released == true then
          "manual_gate_released_dispatch_plan_missing"
        elif .signal_allowed == false then
          "signal_blocked"
        else
          "incomplete"
        end
      ' "${report_file}" 2>> "${LOG_DIR}/jq-errors.log")"
      jq -c \
        --arg venue "${venue}" \
        --arg symbol "${symbol}" \
        --arg run_id "${run_id}" \
        --arg started_at "${started_at}" \
        --arg finished_at "${finished_at}" \
        --arg out_dir "${out_dir}" \
        --arg validation_result_class "${validation_result_class}" \
        --argjson exit_status "${status}" \
        '. + {
          venue: $venue,
          symbol: $symbol,
          run_id: $run_id,
          validation_started_at: $started_at,
          validation_finished_at: $finished_at,
          validation_exit_status: $exit_status,
          validation_result_class: $validation_result_class,
          dry_run_output_dir: $out_dir,
          mutable_execution_started: false
        }' "${report_file}" >> "${DRY_RUN_REPORTS_JSONL}"
    else
      append_json_line "${DRY_RUN_REPORTS_JSONL}" \
        --arg venue "${venue}" \
        --arg symbol "${symbol}" \
        --arg run_id "${run_id}" \
        --arg started_at "${started_at}" \
        --arg finished_at "${finished_at}" \
        --arg out_dir "${out_dir}" \
        --argjson exit_status "${status}" \
        '{venue:$venue,symbol:$symbol,run_id:$run_id,validation_started_at:$started_at,validation_finished_at:$finished_at,validation_exit_status:$exit_status,validation_result_class:"report_missing",dry_run_output_dir:$out_dir,mutable_execution_started:false,report_missing:true}'
    fi

    echo "[${finished_at}] validation_end venue=${venue} symbol=${symbol} run_id=${run_id} exit_status=${status} out=${out_dir}"
  } >> "${job_log}" 2>&1
}

run_funding_arb_validation_job() {
  set +e
  local pair_id="$1"
  local symbol="$2"
  local snapshot_file="$3"
  local run_id="$4"
  local out_dir="$5"
  local job_log="$6"
  local started_at
  local finished_at
  local status
  local report_file
  local validation_result_class="command_failed"

  report_file="${out_dir}/funding_arb_guarded_dry_run_report.json"
  started_at="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
  mkdir -p "${out_dir}"

  {
    echo "[${started_at}] funding_validation_start pair_id=${pair_id} symbol=${symbol} run_id=${run_id}"

    "${RUNTIME_BIN}" funding-arb-guarded-dry-run-once \
      --snapshot "${snapshot_file}" \
      --pair-id "${pair_id}" \
      --config "${CONFIG_PATH}" \
      --out "${out_dir}" \
      --notional-usd "${NOTIONAL_USD}" \
      --taker-fee-bps "${PERP_FEE_BPS}" \
      --slippage-bps "${SLIPPAGE_BPS}" \
      --max-entry-price-divergence-bps "${FUNDING_ARB_MAX_ENTRY_PRICE_DIVERGENCE_BPS}" \
      --min-net-funding-bps "${MIN_NET_BPS}" \
      --dry-run
    status="$?"
    finished_at="$(date -u +%Y-%m-%dT%H:%M:%SZ)"

    if [[ -s "${report_file}" ]]; then
      validation_result_class="$(jq -r --argjson exit_status "${status}" '
        if $exit_status != 0 then
          "command_failed"
        elif .dispatch_plan_built == true and (.dispatch_request_count // 0) == 2 then
          "pre_trade_flow_complete"
        elif .manual_gate_released == true then
          "manual_gate_released_dispatch_plan_missing"
        elif .signal_allowed == false then
          "signal_blocked"
        else
          "incomplete"
        end
      ' "${report_file}" 2>> "${LOG_DIR}/jq-errors.log")"
      jq -c \
        --arg strategy "cross-exchange-funding-arb" \
        --arg pair_id "${pair_id}" \
        --arg symbol "${symbol}" \
        --arg run_id "${run_id}" \
        --arg started_at "${started_at}" \
        --arg finished_at "${finished_at}" \
        --arg out_dir "${out_dir}" \
        --arg validation_result_class "${validation_result_class}" \
        --argjson exit_status "${status}" \
        '. + {
          strategy: $strategy,
          pair_id: $pair_id,
          symbol: $symbol,
          run_id: $run_id,
          validation_started_at: $started_at,
          validation_finished_at: $finished_at,
          validation_exit_status: $exit_status,
          validation_result_class: $validation_result_class,
          dry_run_output_dir: $out_dir,
          mutable_execution_started: false
        }' "${report_file}" >> "${DRY_RUN_REPORTS_JSONL}"
    else
      append_json_line "${DRY_RUN_REPORTS_JSONL}" \
        --arg strategy "cross-exchange-funding-arb" \
        --arg pair_id "${pair_id}" \
        --arg symbol "${symbol}" \
        --arg run_id "${run_id}" \
        --arg started_at "${started_at}" \
        --arg finished_at "${finished_at}" \
        --arg out_dir "${out_dir}" \
        --argjson exit_status "${status}" \
        '{strategy:$strategy,pair_id:$pair_id,symbol:$symbol,run_id:$run_id,validation_started_at:$started_at,validation_finished_at:$finished_at,validation_exit_status:$exit_status,validation_result_class:"report_missing",dry_run_output_dir:$out_dir,mutable_execution_started:false,report_missing:true}'
    fi

    echo "[${finished_at}] funding_validation_end pair_id=${pair_id} symbol=${symbol} run_id=${run_id} exit_status=${status} out=${out_dir}"
  } >> "${job_log}" 2>&1
}

maybe_start_validation() {
  local venue="$1"
  local symbol="$2"
  local ts="$3"
  local now
  local last_file
  local last_value
  local inflight_file
  local inflight_pid
  local run_id
  local out_dir
  local job_log
  local slug
  local pid

  [[ "${VALIDATE_AUTO_ONCE}" == "1" ]] || return 0

  [[ -n "${symbol}" ]] || return 0
  if ! supports_auto_once_dry_run "${venue}"; then
    append_json_line "${VALIDATION_EVENTS_JSONL}" \
      --arg venue "${venue}" \
      --arg symbol "${symbol}" \
      --arg recorded_at "${ts}" \
      '{recorded_at:$recorded_at,venue:$venue,symbol:$symbol,event:"validation_skipped",reason:"auto_once_dry_run_not_supported",mutable_execution_started:false}'
    return 0
  fi

  slug="$(symbol_slug "${symbol}")"

  inflight_file="${STATE_DIR}/validation-${venue}-${slug}.pid"
  if [[ -s "${inflight_file}" ]]; then
    inflight_pid="$(sed -n '1p' "${inflight_file}")"
    if is_alive "${inflight_pid}"; then
      append_json_line "${VALIDATION_EVENTS_JSONL}" \
        --arg venue "${venue}" \
        --arg symbol "${symbol}" \
        --arg recorded_at "${ts}" \
        --argjson pid "${inflight_pid}" \
        '{recorded_at:$recorded_at,venue:$venue,symbol:$symbol,event:"validation_skipped",reason:"validation_in_progress",pid:$pid}'
      return 0
    fi
  fi

  now="$(date -u +%s)"
  last_file="${STATE_DIR}/last-validation-${venue}-${slug}.epoch"
  if [[ "${AUTO_ONCE_COOLDOWN_SECS}" != "0" && -s "${last_file}" ]]; then
    last_value="$(sed -n '1p' "${last_file}")"
    if [[ "${last_value}" =~ ^[0-9]+$ ]] && (( now - last_value < AUTO_ONCE_COOLDOWN_SECS )); then
      append_json_line "${VALIDATION_EVENTS_JSONL}" \
        --arg venue "${venue}" \
        --arg symbol "${symbol}" \
        --arg recorded_at "${ts}" \
        --argjson age "$((now - last_value))" \
        --argjson cooldown "${AUTO_ONCE_COOLDOWN_SECS}" \
        '{recorded_at:$recorded_at,venue:$venue,symbol:$symbol,event:"validation_skipped",reason:"cooldown",age_secs:$age,cooldown_secs:$cooldown}'
      return 0
    fi
  fi
  printf '%s\n' "${now}" > "${last_file}"

  run_id="$(date -u +%Y%m%dT%H%M%SZ)-${venue}-${slug}-$$-${RANDOM}"
  out_dir="${DRY_RUN_DIR}/${run_id}"
  job_log="${LOG_DIR}/${venue}-dry-run-${run_id}.log"
  run_validation_job "${venue}" "${symbol}" "${run_id}" "${out_dir}" "${job_log}" &
  pid="$!"
  printf '%s\n' "${pid}" > "${inflight_file}"
  printf '%s\t%s\t%s\n' "${pid}" "validation-${venue}-${slug}-${run_id}" "${job_log}" >> "${PID_FILE}"
  append_json_line "${VALIDATION_EVENTS_JSONL}" \
    --arg venue "${venue}" \
    --arg symbol "${symbol}" \
    --arg recorded_at "${ts}" \
    --arg run_id "${run_id}" \
    --arg out_dir "${out_dir}" \
    --argjson pid "${pid}" \
    '{recorded_at:$recorded_at,venue:$venue,symbol:$symbol,event:"validation_started",run_id:$run_id,pid:$pid,dry_run_output_dir:$out_dir,mutable_execution_started:false}'
}

maybe_start_funding_arb_validation() {
  local pair_id="$1"
  local symbol="$2"
  local ts="$3"
  local body="$4"
  local now
  local last_file
  local last_value
  local inflight_file
  local inflight_pid
  local run_id
  local out_dir
  local job_log
  local pair_slug
  local symbol_part
  local snapshot_file
  local pid

  [[ "${VALIDATE_AUTO_ONCE}" == "1" ]] || return 0
  [[ -n "${pair_id}" && -n "${symbol}" ]] || return 0

  pair_slug="$(symbol_slug "${pair_id}")"
  symbol_part="$(symbol_slug "${symbol}")"
  inflight_file="${STATE_DIR}/validation-funding-arb-${pair_slug}.pid"
  if [[ -s "${inflight_file}" ]]; then
    inflight_pid="$(sed -n '1p' "${inflight_file}")"
    if is_alive "${inflight_pid}"; then
      append_json_line "${VALIDATION_EVENTS_JSONL}" \
        --arg strategy "cross-exchange-funding-arb" \
        --arg pair_id "${pair_id}" \
        --arg symbol "${symbol}" \
        --arg recorded_at "${ts}" \
        --argjson pid "${inflight_pid}" \
        '{recorded_at:$recorded_at,strategy:$strategy,pair_id:$pair_id,symbol:$symbol,event:"validation_skipped",reason:"validation_in_progress",pid:$pid,mutable_execution_started:false}'
      return 0
    fi
  fi

  now="$(date -u +%s)"
  last_file="${STATE_DIR}/last-validation-funding-arb-${pair_slug}.epoch"
  if [[ "${AUTO_ONCE_COOLDOWN_SECS}" != "0" && -s "${last_file}" ]]; then
    last_value="$(sed -n '1p' "${last_file}")"
    if [[ "${last_value}" =~ ^[0-9]+$ ]] && (( now - last_value < AUTO_ONCE_COOLDOWN_SECS )); then
      append_json_line "${VALIDATION_EVENTS_JSONL}" \
        --arg strategy "cross-exchange-funding-arb" \
        --arg pair_id "${pair_id}" \
        --arg symbol "${symbol}" \
        --arg recorded_at "${ts}" \
        --argjson age "$((now - last_value))" \
        --argjson cooldown "${AUTO_ONCE_COOLDOWN_SECS}" \
        '{recorded_at:$recorded_at,strategy:$strategy,pair_id:$pair_id,symbol:$symbol,event:"validation_skipped",reason:"cooldown",age_secs:$age,cooldown_secs:$cooldown,mutable_execution_started:false}'
      return 0
    fi
  fi
  printf '%s\n' "${now}" > "${last_file}"

  run_id="$(date -u +%Y%m%dT%H%M%SZ)-funding-arb-${symbol_part}-$$-${RANDOM}"
  out_dir="${DRY_RUN_DIR}/${run_id}"
  job_log="${LOG_DIR}/funding-arb-dry-run-${run_id}.log"
  snapshot_file="${out_dir}/funding_arb_opportunities_snapshot.input.json"
  mkdir -p "${out_dir}"
  printf '%s\n' "${body}" > "${snapshot_file}"

  run_funding_arb_validation_job "${pair_id}" "${symbol}" "${snapshot_file}" "${run_id}" "${out_dir}" "${job_log}" &
  pid="$!"
  printf '%s\n' "${pid}" > "${inflight_file}"
  printf '%s\t%s\t%s\n' "${pid}" "validation-funding-arb-${pair_slug}-${run_id}" "${job_log}" >> "${PID_FILE}"
  append_json_line "${VALIDATION_EVENTS_JSONL}" \
    --arg strategy "cross-exchange-funding-arb" \
    --arg pair_id "${pair_id}" \
    --arg symbol "${symbol}" \
    --arg recorded_at "${ts}" \
    --arg run_id "${run_id}" \
    --arg out_dir "${out_dir}" \
    --argjson pid "${pid}" \
    '{recorded_at:$recorded_at,strategy:$strategy,pair_id:$pair_id,symbol:$symbol,event:"validation_started",run_id:$run_id,pid:$pid,dry_run_output_dir:$out_dir,mutable_execution_started:false}'
}

start_validations_for_candidates() {
  local venue="$1"
  local body="$2"
  local ts="$3"
  local symbol

  printf '%s\n' "${body}" | jq -r '.rows[]? | select(.is_candidate == true) | .symbol' |
    while IFS= read -r symbol; do
      maybe_start_validation "${venue}" "${symbol}" "${ts}"
    done
}

start_funding_arb_validations_for_candidates() {
  local body="$1"
  local ts="$2"
  local pair_id
  local symbol

  printf '%s\n' "${body}" | jq -r '.rows[]? | select(.is_candidate == true) | [.pair_id, .symbol] | @tsv' |
    while IFS=$'\t' read -r pair_id symbol; do
      maybe_start_funding_arb_validation "${pair_id}" "${symbol}" "${ts}" "${body}"
    done
}

poll_venue() {
  local venue="$1"
  local ts
  local url
  local body
  local count
  local total_rows
  local snapshot_status
  local updated_at
  local summary

  ts="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
  url="$(opportunities_url "${venue}")"
  if ! body="$(fetch_url_with_retries "${url}")"; then
    append_json_line "${HEALTH_EVENTS_JSONL}" \
      --arg recorded_at "${ts}" \
      --arg venue "${venue}" \
      --arg endpoint "${url}" \
      --argjson retries "${CURL_RETRIES}" \
      '{recorded_at:$recorded_at,venue:$venue,event:"poll_failed",endpoint:$endpoint,retries:$retries,mutable_execution_started:false}'
    return 0
  fi

  count="$(printf '%s\n' "${body}" | jq -r '.candidate_count // 0' 2>> "${LOG_DIR}/jq-errors.log")"
  if ! [[ "${count}" =~ ^[0-9]+$ ]]; then
    append_json_line "${HEALTH_EVENTS_JSONL}" \
      --arg recorded_at "${ts}" \
      --arg venue "${venue}" \
      --arg endpoint "${url}" \
      '{recorded_at:$recorded_at,venue:$venue,event:"invalid_candidate_count",endpoint:$endpoint,mutable_execution_started:false}'
    return 0
  fi

  total_rows="$(printf '%s\n' "${body}" | jq -r '(.rows // []) | length' 2>> "${LOG_DIR}/jq-errors.log" || printf '0')"
  snapshot_status="$(printf '%s\n' "${body}" | jq -r '.status // "unknown"' 2>> "${LOG_DIR}/jq-errors.log" || printf 'unknown')"
  updated_at="$(printf '%s\n' "${body}" | jq -r '.updated_at // .refreshed_at // "unknown"' 2>> "${LOG_DIR}/jq-errors.log" || printf 'unknown')"
  if ! [[ "${total_rows}" =~ ^[0-9]+$ ]]; then
    total_rows="0"
  fi
  append_json_line "${HEALTH_EVENTS_JSONL}" \
    --arg recorded_at "${ts}" \
    --arg venue "${venue}" \
    --arg endpoint "${url}" \
    --arg status "${snapshot_status}" \
    --arg updated_at "${updated_at}" \
    --argjson candidate_count "${count}" \
    --argjson total_rows "${total_rows}" \
    '{recorded_at:$recorded_at,venue:$venue,event:"poll_ok",endpoint:$endpoint,status:$status,updated_at:$updated_at,candidate_count:$candidate_count,total_rows:$total_rows,mutable_execution_started:false}'

  if (( count > 0 )); then
    printf '%s\n' "${body}" | jq -c \
      --arg recorded_at "${ts}" \
      --arg venue "${venue}" \
      --arg endpoint "${url}" \
      '. + {recorded_at:$recorded_at,venue:$venue,endpoint:$endpoint,mutable_execution_started:false}' \
      >> "${OPPORTUNITY_DIR}/${venue}-opportunities.jsonl"
    printf '%s\n' "${body}" | jq -c \
      --arg recorded_at "${ts}" \
      --arg venue "${venue}" \
      --arg endpoint "${url}" \
      '. + {recorded_at:$recorded_at,venue:$venue,endpoint:$endpoint,mutable_execution_started:false}' \
      >> "${OPPORTUNITY_DIR}/spot-perp-basis.jsonl"
    printf '%s\n' "${body}" | jq -c \
      --arg recorded_at "${ts}" \
      --arg venue "${venue}" \
      --arg endpoint "${url}" \
      '. + {recorded_at:$recorded_at,venue:$venue,endpoint:$endpoint,mutable_execution_started:false}' \
      >> "${OPPORTUNITY_DIR}/all-opportunities.jsonl"

    summary="$(printf '%s\n' "${body}" | jq -r '[.rows[]? | "\(.symbol):net=\(.net_basis_bps // "null")bps profit=\(.expected_profit_usd // "null")"] | join(", ")')"
    echo "[${ts}] opportunity venue=${venue} candidate_count=${count} ${summary}" | tee -a "${FEEDBACK_LOG}" >/dev/null
    start_validations_for_candidates "${venue}" "${body}" "${ts}"
  fi
}

poll_funding_arb() {
  local ts
  local url
  local body
  local count
  local total_rows
  local snapshot_status
  local updated_at
  local summary

  ts="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
  url="$(funding_arb_opportunities_url)"
  if ! body="$(fetch_url_with_retries "${url}")"; then
    append_json_line "${HEALTH_EVENTS_JSONL}" \
      --arg recorded_at "${ts}" \
      --arg strategy "cross-exchange-funding-arb" \
      --arg endpoint "${url}" \
      --argjson retries "${CURL_RETRIES}" \
      '{recorded_at:$recorded_at,strategy:$strategy,event:"poll_failed",endpoint:$endpoint,retries:$retries,mutable_execution_started:false}'
    return 0
  fi

  count="$(printf '%s\n' "${body}" | jq -r '.candidate_count // 0' 2>> "${LOG_DIR}/jq-errors.log")"
  if ! [[ "${count}" =~ ^[0-9]+$ ]]; then
    append_json_line "${HEALTH_EVENTS_JSONL}" \
      --arg recorded_at "${ts}" \
      --arg strategy "cross-exchange-funding-arb" \
      --arg endpoint "${url}" \
      '{recorded_at:$recorded_at,strategy:$strategy,event:"invalid_candidate_count",endpoint:$endpoint,mutable_execution_started:false}'
    return 0
  fi

  total_rows="$(printf '%s\n' "${body}" | jq -r '(.rows // []) | length' 2>> "${LOG_DIR}/jq-errors.log" || printf '0')"
  snapshot_status="$(printf '%s\n' "${body}" | jq -r '.status // "unknown"' 2>> "${LOG_DIR}/jq-errors.log" || printf 'unknown')"
  updated_at="$(printf '%s\n' "${body}" | jq -r '.updated_at // "unknown"' 2>> "${LOG_DIR}/jq-errors.log" || printf 'unknown')"
  if ! [[ "${total_rows}" =~ ^[0-9]+$ ]]; then
    total_rows="0"
  fi
  append_json_line "${HEALTH_EVENTS_JSONL}" \
    --arg recorded_at "${ts}" \
    --arg strategy "cross-exchange-funding-arb" \
    --arg endpoint "${url}" \
    --arg status "${snapshot_status}" \
    --arg updated_at "${updated_at}" \
    --argjson candidate_count "${count}" \
    --argjson total_rows "${total_rows}" \
    '{recorded_at:$recorded_at,strategy:$strategy,event:"poll_ok",endpoint:$endpoint,status:$status,updated_at:$updated_at,candidate_count:$candidate_count,total_rows:$total_rows,mutable_execution_started:false}'

  if (( count > 0 )); then
    printf '%s\n' "${body}" | jq -c \
      --arg recorded_at "${ts}" \
      --arg strategy "cross-exchange-funding-arb" \
      --arg endpoint "${url}" \
      '. + {recorded_at:$recorded_at,strategy:$strategy,endpoint:$endpoint,mutable_execution_started:false}' \
      >> "${OPPORTUNITY_DIR}/cross-exchange-funding-arb.jsonl"
    printf '%s\n' "${body}" | jq -c \
      --arg recorded_at "${ts}" \
      --arg strategy "cross-exchange-funding-arb" \
      --arg endpoint "${url}" \
      '. + {recorded_at:$recorded_at,strategy:$strategy,endpoint:$endpoint,mutable_execution_started:false}' \
      >> "${OPPORTUNITY_DIR}/all-opportunities.jsonl"

    summary="$(printf '%s\n' "${body}" | jq -r '[.rows[]? | "\(.pair_id):net=\(.net_funding_bps // "null")bps funding=\(.expected_funding_usd // "null")"] | join(", ")')"
    echo "[${ts}] opportunity strategy=cross-exchange-funding-arb candidate_count=${count} ${summary}" | tee -a "${FEEDBACK_LOG}" >/dev/null
    start_funding_arb_validations_for_candidates "${body}" "${ts}"
  fi
}

run_recorder() {
  cd "${REPO_ROOT}"
  trap 'echo "[$(date -u +%Y-%m-%dT%H:%M:%SZ)] recorder_stop" >> "${FEEDBACK_LOG}"; exit 0' INT TERM
  mkdir -p "${LOG_DIR}" "${STATE_DIR}" "${OPPORTUNITY_DIR}" "${DRY_RUN_DIR}" "${SNAPSHOT_DIR}"
  touch "${OPPORTUNITY_DIR}/all-opportunities.jsonl" "${OPPORTUNITY_DIR}/spot-perp-basis.jsonl" "${OPPORTUNITY_DIR}/cross-exchange-funding-arb.jsonl"
  IFS=' ' read -r -a RECORDER_MONITORS <<< "${EFFECTIVE_MONITORS}"
  echo "[$(date -u +%Y-%m-%dT%H:%M:%SZ)] recorder_start monitors=${EFFECTIVE_MONITORS} strategies=${EFFECTIVE_STRATEGIES} interval_secs=${INTERVAL_SECS} min_net_bps=${MIN_NET_BPS}" >> "${FEEDBACK_LOG}"
  append_json_line "${HEALTH_EVENTS_JSONL}" \
    --arg recorded_at "$(date -u +%Y-%m-%dT%H:%M:%SZ)" \
    --arg strategies "${EFFECTIVE_STRATEGIES}" \
    '{recorded_at:$recorded_at,event:"observer_strategies_configured",strategies:$strategies,mutable_execution_started:false}'
  while true; do
    if strategy_enabled "spot-perp-basis"; then
      for venue in "${RECORDER_MONITORS[@]}"; do
        poll_venue "${venue}"
      done
    fi
    if strategy_enabled "cross-exchange-funding-arb"; then
      poll_funding_arb
    fi
    sleep "${INTERVAL_SECS}"
  done
}

if [[ "${1:-}" == "--recorder" ]]; then
  RUN_ROOT="${BASIS_OBSERVER_ROOT:-${REPO_ROOT}/target/arb-opportunity-observer}"
  LOG_DIR="${RUN_ROOT}/logs"
  STATE_DIR="${RUN_ROOT}/state"
  SNAPSHOT_DIR="${RUN_ROOT}/snapshots"
  OPPORTUNITY_DIR="${RUN_ROOT}/opportunities"
  DRY_RUN_DIR="${RUN_ROOT}/dry-run"
  PID_FILE="${STATE_DIR}/basis-observer.pids"
  FEEDBACK_LOG="${LOG_DIR}/realtime-feedback.log"
  HEALTH_EVENTS_JSONL="${LOG_DIR}/health-events.jsonl"
  VALIDATION_EVENTS_JSONL="${DRY_RUN_DIR}/validation-events.jsonl"
  DRY_RUN_REPORTS_JSONL="${DRY_RUN_DIR}/dry-run-reports.jsonl"
  RUNTIME_BIN="${BASIS_OBSERVER_RUNTIME_BIN:-${REPO_ROOT}/target/debug/arb-runtime}"
  CONFIG_PATH="${BASIS_OBSERVER_CONFIG:-templates/personal_guarded_live.preflight.yaml}"
  INTERVAL_SECS="${BASIS_OBSERVER_INTERVAL_SECS:-5}"
  MIN_NET_BPS="${BASIS_OBSERVER_MIN_NET_BPS:-5}"
  NOTIONAL_USD="${BASIS_OBSERVER_NOTIONAL_USD:-100.00}"
  PERP_FEE_BPS="${BASIS_OBSERVER_PERP_FEE_BPS:-5}"
  SLIPPAGE_BPS="${BASIS_OBSERVER_SLIPPAGE_BPS:-5}"
  FUNDING_ARB_MAX_ENTRY_PRICE_DIVERGENCE_BPS="${FUNDING_ARB_MAX_ENTRY_PRICE_DIVERGENCE_BPS:-20}"
  VALIDATE_AUTO_ONCE="${BASIS_OBSERVER_VALIDATE_AUTO_ONCE:-1}"
  AUTO_ONCE_COOLDOWN_SECS="${BASIS_OBSERVER_AUTO_ONCE_COOLDOWN_SECS:-60}"
  CURL_TIMEOUT_SECS="${BASIS_OBSERVER_CURL_TIMEOUT_SECS:-10}"
  CURL_RETRIES="${BASIS_OBSERVER_CURL_RETRIES:-3}"
  CURL_RETRY_SLEEP_SECS="${BASIS_OBSERVER_CURL_RETRY_SLEEP_SECS:-1}"
  EFFECTIVE_MONITORS="${BASIS_OBSERVER_EFFECTIVE_MONITORS:-binance bybit okx bitget aster hyperliquid}"
  EFFECTIVE_STRATEGIES="${BASIS_OBSERVER_EFFECTIVE_STRATEGIES:-spot-perp-basis,cross-exchange-funding-arb}"
  BINANCE_BIND="${BINANCE_BASIS_BIND:-127.0.0.1:8796}"
  BYBIT_BIND="${BYBIT_BASIS_BIND:-127.0.0.1:8797}"
  OKX_BIND="${OKX_BASIS_BIND:-127.0.0.1:8798}"
  BITGET_BIND="${BITGET_BASIS_BIND:-127.0.0.1:8803}"
  ASTER_BIND="${ASTER_BASIS_BIND:-127.0.0.1:8800}"
  HYPERLIQUID_BIND="${HYPERLIQUID_BASIS_BIND:-127.0.0.1:8799}"
  FUNDING_ARB_BIND="${FUNDING_ARB_BIND:-127.0.0.1:8804}"
  run_recorder
  exit 0
fi

if [[ "${1:-}" == "-h" || "${1:-}" == "--help" ]]; then
  usage
  exit 0
fi

require_command cargo
require_command curl
require_command jq

CLI_MONITORS=()
CLI_STRATEGIES=""
while [[ "$#" -gt 0 ]]; do
  case "$1" in
    --venues)
      [[ "$#" -ge 2 ]] || die "--venues requires a comma-separated value"
      IFS=',' read -r -a CLI_MONITORS <<< "$2"
      shift 2
      ;;
    --strategies)
      [[ "$#" -ge 2 ]] || die "--strategies requires a comma-separated value"
      CLI_STRATEGIES="$2"
      shift 2
      ;;
    --dry-run)
      shift
      ;;
    --*)
      die "unknown option: $1"
      ;;
    *)
      CLI_MONITORS+=("$1")
      shift
      ;;
  esac
done

RUN_ROOT="${BASIS_OBSERVER_ROOT:-${REPO_ROOT}/target/arb-opportunity-observer}"
LOG_DIR="${RUN_ROOT}/logs"
STATE_DIR="${RUN_ROOT}/state"
SNAPSHOT_DIR="${RUN_ROOT}/snapshots"
OPPORTUNITY_DIR="${RUN_ROOT}/opportunities"
DRY_RUN_DIR="${RUN_ROOT}/dry-run"
PID_FILE="${STATE_DIR}/basis-observer.pids"
RUNTIME_BIN="${REPO_ROOT}/target/debug/arb-runtime"
CONFIG_PATH="${BASIS_OBSERVER_CONFIG:-templates/personal_guarded_live.preflight.yaml}"
INTERVAL_SECS="${BASIS_OBSERVER_INTERVAL_SECS:-5}"
MIN_ABS_FUNDING_RATE="${BASIS_OBSERVER_MIN_ABS_FUNDING_RATE:-0}"
MIN_NET_BPS="${BASIS_OBSERVER_MIN_NET_BPS:-5}"
NOTIONAL_USD="${BASIS_OBSERVER_NOTIONAL_USD:-100.00}"
SPOT_FEE_BPS="${BASIS_OBSERVER_SPOT_FEE_BPS:-10}"
PERP_FEE_BPS="${BASIS_OBSERVER_PERP_FEE_BPS:-5}"
SLIPPAGE_BPS="${BASIS_OBSERVER_SLIPPAGE_BPS:-5}"
BINANCE_BIND="${BINANCE_BASIS_BIND:-127.0.0.1:8796}"
BYBIT_BIND="${BYBIT_BASIS_BIND:-127.0.0.1:8797}"
OKX_BIND="${OKX_BASIS_BIND:-127.0.0.1:8798}"
BITGET_BIND="${BITGET_BASIS_BIND:-127.0.0.1:8803}"
ASTER_BIND="${ASTER_BASIS_BIND:-127.0.0.1:8800}"
HYPERLIQUID_BIND="${HYPERLIQUID_BASIS_BIND:-127.0.0.1:8799}"
FUNDING_ARB_BIND="${FUNDING_ARB_BIND:-127.0.0.1:8804}"
FUNDING_ARB_MAX_ENTRY_PRICE_DIVERGENCE_BPS="${FUNDING_ARB_MAX_ENTRY_PRICE_DIVERGENCE_BPS:-20}"
VALIDATE_AUTO_ONCE="${BASIS_OBSERVER_VALIDATE_AUTO_ONCE:-1}"
AUTO_ONCE_COOLDOWN_SECS="${BASIS_OBSERVER_AUTO_ONCE_COOLDOWN_SECS:-60}"
CURL_TIMEOUT_SECS="${BASIS_OBSERVER_CURL_TIMEOUT_SECS:-10}"
CURL_RETRIES="${BASIS_OBSERVER_CURL_RETRIES:-3}"
CURL_RETRY_SLEEP_SECS="${BASIS_OBSERVER_CURL_RETRY_SLEEP_SECS:-1}"
STARTUP_CHECK="${BASIS_OBSERVER_STARTUP_CHECK:-1}"
STARTUP_WAIT_SECS="${BASIS_OBSERVER_STARTUP_WAIT_SECS:-180}"
STOP_DRAIN_SECS="${BASIS_OBSERVER_STOP_DRAIN_SECS:-15}"
STOP_GRACE_SECS="${BASIS_OBSERVER_STOP_GRACE_SECS:-3}"
FOREGROUND="${BASIS_OBSERVER_FOREGROUND:-0}"
STRATEGIES="${CLI_STRATEGIES:-${BASIS_OBSERVER_STRATEGIES:-spot-perp-basis,cross-exchange-funding-arb}}"

if [[ "${#CLI_MONITORS[@]}" -eq 0 ]]; then
  if [[ -n "${BASIS_OBSERVER_MONITORS:-}" ]]; then
    IFS=' ' read -r -a MONITORS <<< "${BASIS_OBSERVER_MONITORS}"
  else
    MONITORS=(binance bybit okx bitget aster hyperliquid)
  fi
else
  MONITORS=("${CLI_MONITORS[@]}")
fi

for monitor in "${MONITORS[@]}"; do
  case "${monitor}" in
    binance|bybit|okx|bitget|aster|hyperliquid) ;;
    *) die "unknown monitor: ${monitor}" ;;
  esac
done

IFS=',' read -r -a STRATEGY_LIST <<< "${STRATEGIES}"
for strategy in "${STRATEGY_LIST[@]}"; do
  strategy="${strategy//[[:space:]]/}"
  case "${strategy}" in
    spot-perp-basis|cross-exchange-funding-arb) ;;
    *) die "unknown strategy: ${strategy}" ;;
  esac
done

mkdir -p "${LOG_DIR}" "${STATE_DIR}" "${SNAPSHOT_DIR}" "${OPPORTUNITY_DIR}" "${DRY_RUN_DIR}"
touch "${OPPORTUNITY_DIR}/all-opportunities.jsonl" "${OPPORTUNITY_DIR}/spot-perp-basis.jsonl" "${OPPORTUNITY_DIR}/cross-exchange-funding-arb.jsonl"

if [[ -s "${PID_FILE}" ]]; then
  while IFS=$'\t' read -r pid _name _log; do
    if is_alive "${pid}"; then
      die "observer already appears to be running; stop it first with scripts/stop-basis-opportunity-observer.sh"
    fi
  done < "${PID_FILE}"
fi
: > "${PID_FILE}"

cd "${REPO_ROOT}"
echo "building arb-runtime with live-exec feature..."
cargo build -p arb-runtime --features live-exec --manifest-path "${REPO_ROOT}/Cargo.toml"

COMMON_ARGS=(
  --interval-secs "${INTERVAL_SECS}"
  --min-abs-funding-rate "${MIN_ABS_FUNDING_RATE}"
  --min-net-bps "${MIN_NET_BPS}"
  --notional-usd "${NOTIONAL_USD}"
  --spot-fee-bps "${SPOT_FEE_BPS}"
  --perp-fee-bps "${PERP_FEE_BPS}"
  --slippage-bps "${SLIPPAGE_BPS}"
)

start_monitor() {
  local venue="$1"
  local command="$2"
  local bind_addr="$3"
  local log_file="${LOG_DIR}/${venue}-basis-monitor.log"
  local out_dir="${SNAPSHOT_DIR}/${venue}"
  local pid

  echo "starting ${venue} monitor: http://${bind_addr}/dashboard"
  nohup "${RUNTIME_BIN}" "${command}" \
    --bind "${bind_addr}" \
    --out "${out_dir}" \
    "${COMMON_ARGS[@]}" \
    >> "${log_file}" 2>&1 &
  pid="$!"
  printf '%s\t%s\t%s\n' "${pid}" "${venue}-basis-monitor" "${log_file}" >> "${PID_FILE}"
  echo "  pid=${pid} log=${log_file}"
}

start_funding_arb_monitor() {
  local log_file="${LOG_DIR}/funding-arb-monitor.log"
  local out_dir="${SNAPSHOT_DIR}/funding-arb"
  local pid
  local monitor
  local source
  local -a source_args

  source_args=(--clear-sources)
  for monitor in "${MONITORS[@]}"; do
    source="$(status_url "${monitor}")"
    source_args+=(--source "${monitor}=${source}")
  done

  echo "starting funding arb monitor: http://${FUNDING_ARB_BIND}/dashboard"
  nohup "${RUNTIME_BIN}" funding-arb-monitor \
    --bind "${FUNDING_ARB_BIND}" \
    --interval-secs "${INTERVAL_SECS}" \
    --notional-usd "${NOTIONAL_USD}" \
    --taker-fee-bps "${PERP_FEE_BPS}" \
    --slippage-bps "${SLIPPAGE_BPS}" \
    --max-entry-price-divergence-bps "${FUNDING_ARB_MAX_ENTRY_PRICE_DIVERGENCE_BPS}" \
    --min-net-funding-bps "${MIN_NET_BPS}" \
    --out "${out_dir}" \
    "${source_args[@]}" \
    >> "${log_file}" 2>&1 &
  pid="$!"
  printf '%s\t%s\t%s\n' "${pid}" "funding-arb-monitor" "${log_file}" >> "${PID_FILE}"
  echo "  pid=${pid} log=${log_file}"
}

for monitor in "${MONITORS[@]}"; do
  case "${monitor}" in
    binance) start_monitor binance binance-basis-monitor "${BINANCE_BIND}" ;;
    bybit) start_monitor bybit bybit-basis-monitor "${BYBIT_BIND}" ;;
    okx) start_monitor okx okx-basis-monitor "${OKX_BIND}" ;;
    bitget) start_monitor bitget bitget-basis-monitor "${BITGET_BIND}" ;;
    aster) start_monitor aster aster-basis-monitor "${ASTER_BIND}" ;;
    hyperliquid) start_monitor hyperliquid hyperliquid-basis-monitor "${HYPERLIQUID_BIND}" ;;
  esac
done

if strategy_enabled "cross-exchange-funding-arb"; then
  start_funding_arb_monitor
fi

if [[ "${STARTUP_CHECK}" == "1" ]]; then
  echo "checking /opportunities endpoints before starting recorder..."
  for monitor in "${MONITORS[@]}"; do
    if ! wait_for_monitor_opportunities "${monitor}"; then
      stop_started_processes
      rm -f "${PID_FILE}"
      exit 1
    fi
  done
  if strategy_enabled "cross-exchange-funding-arb"; then
    if ! wait_for_funding_arb_opportunities; then
      stop_started_processes
      rm -f "${PID_FILE}"
      exit 1
    fi
  fi
fi

EFFECTIVE_MONITORS="${MONITORS[*]}"
RECORDER_LOG="${LOG_DIR}/recorder.log"
nohup env \
  BASIS_OBSERVER_ROOT="${RUN_ROOT}" \
  BASIS_OBSERVER_RUNTIME_BIN="${RUNTIME_BIN}" \
  BASIS_OBSERVER_CONFIG="${CONFIG_PATH}" \
  BASIS_OBSERVER_INTERVAL_SECS="${INTERVAL_SECS}" \
  BASIS_OBSERVER_MIN_NET_BPS="${MIN_NET_BPS}" \
  BASIS_OBSERVER_NOTIONAL_USD="${NOTIONAL_USD}" \
  BASIS_OBSERVER_PERP_FEE_BPS="${PERP_FEE_BPS}" \
  BASIS_OBSERVER_SLIPPAGE_BPS="${SLIPPAGE_BPS}" \
  FUNDING_ARB_MAX_ENTRY_PRICE_DIVERGENCE_BPS="${FUNDING_ARB_MAX_ENTRY_PRICE_DIVERGENCE_BPS}" \
  BASIS_OBSERVER_VALIDATE_AUTO_ONCE="${VALIDATE_AUTO_ONCE}" \
  BASIS_OBSERVER_AUTO_ONCE_COOLDOWN_SECS="${AUTO_ONCE_COOLDOWN_SECS}" \
  BASIS_OBSERVER_CURL_TIMEOUT_SECS="${CURL_TIMEOUT_SECS}" \
  BASIS_OBSERVER_CURL_RETRIES="${CURL_RETRIES}" \
  BASIS_OBSERVER_CURL_RETRY_SLEEP_SECS="${CURL_RETRY_SLEEP_SECS}" \
  BASIS_OBSERVER_EFFECTIVE_MONITORS="${EFFECTIVE_MONITORS}" \
  BASIS_OBSERVER_EFFECTIVE_STRATEGIES="${STRATEGIES}" \
  BINANCE_BASIS_BIND="${BINANCE_BIND}" \
  BYBIT_BASIS_BIND="${BYBIT_BIND}" \
  OKX_BASIS_BIND="${OKX_BIND}" \
  BITGET_BASIS_BIND="${BITGET_BIND}" \
  ASTER_BASIS_BIND="${ASTER_BIND}" \
  HYPERLIQUID_BASIS_BIND="${HYPERLIQUID_BIND}" \
  FUNDING_ARB_BIND="${FUNDING_ARB_BIND}" \
  "${SCRIPT_DIR}/start-basis-opportunity-observer.sh" --recorder >> "${RECORDER_LOG}" 2>&1 &
RECORDER_PID="$!"
printf '%s\t%s\t%s\n' "${RECORDER_PID}" "opportunity-recorder" "${RECORDER_LOG}" >> "${PID_FILE}"

cat <<EOF

started basis opportunity observer.
pid file: ${PID_FILE}

dashboards:
  Binance: http://${BINANCE_BIND}/dashboard
  Bybit:   http://${BYBIT_BIND}/dashboard
  OKX:     http://${OKX_BIND}/dashboard
  Bitget:  http://${BITGET_BIND}/dashboard
  Aster:   http://${ASTER_BIND}/dashboard
  Hyperliquid: http://${HYPERLIQUID_BIND}/dashboard
  Funding arb: http://${FUNDING_ARB_BIND}/dashboard

real-time feedback:
  tail -f ${LOG_DIR}/realtime-feedback.log

opportunity logs:
  ${OPPORTUNITY_DIR}/all-opportunities.jsonl
  ${OPPORTUNITY_DIR}/spot-perp-basis.jsonl
  ${OPPORTUNITY_DIR}/cross-exchange-funding-arb.jsonl
  ${OPPORTUNITY_DIR}/binance-opportunities.jsonl
  ${OPPORTUNITY_DIR}/bybit-opportunities.jsonl
  ${OPPORTUNITY_DIR}/okx-opportunities.jsonl
  ${OPPORTUNITY_DIR}/bitget-opportunities.jsonl
  ${OPPORTUNITY_DIR}/aster-opportunities.jsonl
  ${OPPORTUNITY_DIR}/hyperliquid-opportunities.jsonl

dry-run validation reports:
  ${DRY_RUN_DIR}/dry-run-reports.jsonl

中文说明：脚本不会传 --execute-live，也不会传真实下单确认参数；mutable_execution_started=false。
EOF

if [[ "${FOREGROUND}" == "1" ]]; then
  supervise_started_processes
fi
