#!/usr/bin/env bash
set -euo pipefail

# 中文说明：一键启动正式实盘链路。
# 该脚本会先启动实盘下单前置所需的公开 WSS bookTicker monitor，再启动
# arb-runtime live。脚本不会打印密钥；如需加载凭证，可传 --env-file .env.local。

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"

usage() {
  cat <<'USAGE'
用法:
  scripts/start-arb-runtime-live.sh [--detach] [--env-file path] [--no-build]

行为:
  1. 构建 arb-runtime live-exec。
  2. 启动 Binance/Bybit/OKX/Bitget 的 spot/perp WSS，以及 Aster/Hyperliquid perp WSS monitor。
  3. 等待 WSS monitor 进入 streaming，且已收到真实 WSS 更新。
  4. 启动 arb-runtime live --i-understand-live-orders。
  5. arb-runtime live 会默认用常驻 runner 管理 spot-perp-basis 和
     cross-exchange-funding-arb，不再由 observer 反复触发 auto-once。
  6. 打印所有实时 dashboard、日志和停止命令。

常用环境变量:
  ARB_RUNTIME_LIVE_ROOT=target/arb-runtime/live
  ARB_RUNTIME_LIVE_PREREQ_ROOT=target/arb-runtime/live-prereq
  ARB_RUNTIME_LIVE_ENV_FILE=.env.local
  ARB_RUNTIME_LIVE_DETACH=0
  ARB_RUNTIME_LIVE_WSS_READY_TIMEOUT_SECS=120
  ARB_RUNTIME_LIVE_BUILD=1
  ARB_RUNTIME_LIVE_OKX_WSS_SYMBOL=BTC-USDT
  ARB_RUNTIME_LIVE_BITGET_WSS_SYMBOL=BTCUSDT
  ARB_RUNTIME_LIVE_ASTER_WSS_SYMBOL=ALL_USDT
  ARB_RUNTIME_LIVE_HYPERLIQUID_WSS_SYMBOL=ALL_USDT
  ARB_RUNTIME_LIVE_PORTFOLIO_BIND=127.0.0.1:8805
  BASIS_OBSERVER_BASIS_RESIDENT_INTERVAL_SECS=60
  BASIS_OBSERVER_BASIS_RESIDENT_MAX_LIVE_ENTRIES=1
  BASIS_OBSERVER_BASIS_RESIDENT_MAX_CONCURRENT_POSITIONS=1
  BASIS_OBSERVER_BASIS_RESIDENT_MAX_TOTAL_NOTIONAL_USDT=10.00
  BASIS_OBSERVER_FUNDING_ARB_MODE=resident
  BASIS_OBSERVER_FUNDING_ARB_RESIDENT_INTERVAL_SECS=60
  BASIS_OBSERVER_FUNDING_ARB_RESIDENT_MAX_LIVE_ENTRIES=1
  BASIS_OBSERVER_FUNDING_ARB_RESIDENT_MAX_CYCLES=
  BASIS_OBSERVER_FUNDING_SETTLEMENT_LEDGER=
  BASIS_OBSERVER_FUNDING_SETTLEMENT_RAW_SNAPSHOT=
  ARB_RUNTIME_PORTFOLIO_ACCOUNT_SNAPSHOT=target/account_snapshot.json
  ARB_RUNTIME_PORTFOLIO_POSITION_SNAPSHOT=target/position_snapshot.json

正式实盘凭证请放在 shell 环境或 --env-file 指向的本地文件中，不要写入命令行。
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

tail_log_on_error() {
  local log_file="$1"
  [[ -f "${log_file}" ]] && tail -n 40 "${log_file}" >&2 || true
}

DETACH="${ARB_RUNTIME_LIVE_DETACH:-0}"
ENV_FILE="${ARB_RUNTIME_LIVE_ENV_FILE:-}"
BUILD="${ARB_RUNTIME_LIVE_BUILD:-1}"

while [[ "$#" -gt 0 ]]; do
  case "$1" in
    --detach)
      DETACH="1"
      shift
      ;;
    --env-file)
      [[ "$#" -ge 2 ]] || die "--env-file requires a path"
      ENV_FILE="$2"
      shift 2
      ;;
    --no-build)
      BUILD="0"
      shift
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    --*)
      die "unknown option: $1"
      ;;
    *)
      die "unexpected argument: $1"
      ;;
  esac
done

if [[ -n "${ENV_FILE}" ]]; then
  [[ -r "${ENV_FILE}" ]] || die "env file is not readable: ${ENV_FILE}"
  set -a
  # shellcheck disable=SC1090
  source "${ENV_FILE}"
  set +a
fi

require_command cargo
require_command curl
require_command jq

RUN_ROOT="${ARB_RUNTIME_LIVE_ROOT:-${REPO_ROOT}/target/arb-runtime/live}"
PREREQ_ROOT="${ARB_RUNTIME_LIVE_PREREQ_ROOT:-${REPO_ROOT}/target/arb-runtime/live-prereq}"
LOG_DIR="${PREREQ_ROOT}/logs"
STATE_DIR="${PREREQ_ROOT}/state"
WSS_PID_FILE="${STATE_DIR}/wss-book-ticker.pids"
LIVE_PID_FILE="${STATE_DIR}/arb-runtime-live.pid"
PORTFOLIO_PID_FILE="${STATE_DIR}/portfolio-dashboard.pid"
RUNTIME_BIN="${ARB_RUNTIME_LIVE_BIN:-${REPO_ROOT}/target/debug/arb-runtime}"
WSS_READY_TIMEOUT_SECS="${ARB_RUNTIME_LIVE_WSS_READY_TIMEOUT_SECS:-120}"
WSS_RECONNECT_DELAY_SECS="${ARB_RUNTIME_LIVE_WSS_RECONNECT_DELAY_SECS:-2}"
PORTFOLIO_BIND="${ARB_RUNTIME_LIVE_PORTFOLIO_BIND:-127.0.0.1:8805}"

BINANCE_SPOT_WSS_BIND="${ARB_RUNTIME_LIVE_BINANCE_SPOT_WSS_BIND:-127.0.0.1:8786}"
BINANCE_PERP_WSS_BIND="${ARB_RUNTIME_LIVE_BINANCE_PERP_WSS_BIND:-127.0.0.1:8787}"
BYBIT_SPOT_WSS_BIND="${ARB_RUNTIME_LIVE_BYBIT_SPOT_WSS_BIND:-127.0.0.1:8788}"
BYBIT_PERP_WSS_BIND="${ARB_RUNTIME_LIVE_BYBIT_PERP_WSS_BIND:-127.0.0.1:8789}"
OKX_SPOT_WSS_BIND="${ARB_RUNTIME_LIVE_OKX_SPOT_WSS_BIND:-127.0.0.1:8790}"
OKX_PERP_WSS_BIND="${ARB_RUNTIME_LIVE_OKX_PERP_WSS_BIND:-127.0.0.1:8791}"
BITGET_SPOT_WSS_BIND="${ARB_RUNTIME_LIVE_BITGET_SPOT_WSS_BIND:-127.0.0.1:8792}"
BITGET_PERP_WSS_BIND="${ARB_RUNTIME_LIVE_BITGET_PERP_WSS_BIND:-127.0.0.1:8793}"
ASTER_PERP_WSS_BIND="${ARB_RUNTIME_LIVE_ASTER_PERP_WSS_BIND:-127.0.0.1:8794}"
HYPERLIQUID_PERP_WSS_BIND="${ARB_RUNTIME_LIVE_HYPERLIQUID_PERP_WSS_BIND:-127.0.0.1:8795}"

BINANCE_WSS_SYMBOL="${ARB_RUNTIME_LIVE_BINANCE_WSS_SYMBOL:-ALL_USDT}"
BYBIT_WSS_SYMBOL="${ARB_RUNTIME_LIVE_BYBIT_WSS_SYMBOL:-ALL_USDT}"
OKX_WSS_SYMBOL="${ARB_RUNTIME_LIVE_OKX_WSS_SYMBOL:-BTC-USDT}"
BITGET_WSS_SYMBOL="${ARB_RUNTIME_LIVE_BITGET_WSS_SYMBOL:-BTCUSDT}"
ASTER_WSS_SYMBOL="${ARB_RUNTIME_LIVE_ASTER_WSS_SYMBOL:-ALL_USDT}"
HYPERLIQUID_WSS_SYMBOL="${ARB_RUNTIME_LIVE_HYPERLIQUID_WSS_SYMBOL:-ALL_USDT}"

mkdir -p "${LOG_DIR}" "${STATE_DIR}"

if [[ -s "${WSS_PID_FILE}" ]]; then
  while IFS=$'\t' read -r pid name _log _url; do
    if is_alive "${pid}"; then
      die "WSS monitor already running: ${name} pid=${pid}; stop first with scripts/stop-arb-runtime-live.sh"
    fi
  done < "${WSS_PID_FILE}"
fi
: > "${WSS_PID_FILE}"

if [[ -s "${PORTFOLIO_PID_FILE}" ]]; then
  portfolio_pid="$(sed -n '1p' "${PORTFOLIO_PID_FILE}")"
  if is_alive "${portfolio_pid}"; then
    die "portfolio dashboard already running: pid=${portfolio_pid}; stop first with scripts/stop-arb-runtime-live.sh"
  fi
fi

if [[ "${BUILD}" == "1" ]]; then
  echo "building arb-runtime with live-exec feature..."
  cargo build -p arb-runtime --features live-exec --manifest-path "${REPO_ROOT}/Cargo.toml"
fi

start_wss_monitor() {
  local name="$1"
  local command_name="$2"
  local bind_addr="$3"
  local symbol="$4"
  local market="$5"
  local api_prefix="$6"
  local log_file="${LOG_DIR}/${name}.log"
  local status_url="http://${bind_addr}${api_prefix}/status"
  local pid

  echo "starting ${name}: http://${bind_addr}/dashboard"
  nohup env ARB_RUNTIME_ENABLE_LEGACY_COMMANDS=1 "${RUNTIME_BIN}" "${command_name}" \
    --bind "${bind_addr}" \
    --symbol "${symbol}" \
    --market "${market}" \
    --reconnect-delay-secs "${WSS_RECONNECT_DELAY_SECS}" \
    >> "${log_file}" 2>&1 &
  pid="$!"
  printf '%s\t%s\t%s\t%s\n' "${pid}" "${name}" "${log_file}" "${status_url}" >> "${WSS_PID_FILE}"
  echo "  pid=${pid} log=${log_file}"
}

wait_for_wss_monitor() {
  local name="$1"
  local pid="$2"
  local log_file="$3"
  local status_url="$4"
  local deadline="$((SECONDS + WSS_READY_TIMEOUT_SECS))"
  local body
  local status
  local total_rows
  local wss_update_count

  while (( SECONDS <= deadline )); do
    if ! is_alive "${pid}"; then
      echo "error: ${name} exited before readiness" >&2
      tail_log_on_error "${log_file}"
      return 1
    fi
    if body="$(curl -fsS --max-time 2 "${status_url}" 2>/dev/null)"; then
      status="$(printf '%s\n' "${body}" | jq -r '.status // "unknown"' 2>/dev/null || printf 'unknown')"
      total_rows="$(printf '%s\n' "${body}" | jq -r '.total_rows // 0' 2>/dev/null || printf '0')"
      wss_update_count="$(printf '%s\n' "${body}" | jq -r '.wss_update_count // 0' 2>/dev/null || printf '0')"
      if [[ "${status}" == "streaming" && "${total_rows}" =~ ^[0-9]+$ && "${total_rows}" -gt 0 && "${wss_update_count}" =~ ^[0-9]+$ && "${wss_update_count}" -gt 0 ]]; then
        echo "wss_ready name=${name} status_url=${status_url} rows=${total_rows} wss_updates=${wss_update_count}"
        return 0
      fi
    fi
    sleep 1
  done

  echo "error: ${name} did not become healthy within ${WSS_READY_TIMEOUT_SECS}s: ${status_url}" >&2
  tail_log_on_error "${log_file}"
  return 1
}

stop_wss_monitors() {
  [[ -s "${WSS_PID_FILE}" ]] || return 0
  local pid
  local name
  local log_file
  local status_url
  while IFS=$'\t' read -r pid name log_file status_url; do
    if is_alive "${pid}"; then
      echo "TERM WSS pid=${pid} name=${name}"
      kill -TERM "${pid}" 2>/dev/null || true
    fi
  done < "${WSS_PID_FILE}"
}

start_portfolio_dashboard() {
  local log_file="${LOG_DIR}/portfolio-dashboard.log"
  local -a args
  local pid

  args=(
    "${RUNTIME_BIN}" portfolio-dashboard
    --bind "${PORTFOLIO_BIND}"
    --resident-root "${RUN_ROOT}"
  )
  [[ -n "${ARB_RUNTIME_PORTFOLIO_ACCOUNT_SNAPSHOT:-}" ]] && args+=(--account-snapshot "${ARB_RUNTIME_PORTFOLIO_ACCOUNT_SNAPSHOT}")
  [[ -n "${ARB_RUNTIME_PORTFOLIO_ACCOUNT_RAW_SNAPSHOT:-}" ]] && args+=(--account-raw-snapshot "${ARB_RUNTIME_PORTFOLIO_ACCOUNT_RAW_SNAPSHOT}")
  [[ -n "${ARB_RUNTIME_PORTFOLIO_POSITION_SNAPSHOT:-}" ]] && args+=(--position-snapshot "${ARB_RUNTIME_PORTFOLIO_POSITION_SNAPSHOT}")
  [[ -n "${ARB_RUNTIME_PORTFOLIO_POSITION_RAW_SNAPSHOT:-}" ]] && args+=(--position-raw-snapshot "${ARB_RUNTIME_PORTFOLIO_POSITION_RAW_SNAPSHOT}")
  [[ -n "${ARB_RUNTIME_PORTFOLIO_FUNDING_SNAPSHOT:-}" ]] && args+=(--funding-snapshot "${ARB_RUNTIME_PORTFOLIO_FUNDING_SNAPSHOT}")

  echo "starting portfolio dashboard: http://${PORTFOLIO_BIND}/dashboard"
  nohup env ARB_RUNTIME_ENABLE_LEGACY_COMMANDS=1 "${args[@]}" >> "${log_file}" 2>&1 &
  pid="$!"
  printf '%s\n' "${pid}" > "${PORTFOLIO_PID_FILE}"
  echo "  pid=${pid} log=${log_file}"
}

stop_portfolio_dashboard() {
  [[ -s "${PORTFOLIO_PID_FILE}" ]] || return 0
  local pid
  pid="$(sed -n '1p' "${PORTFOLIO_PID_FILE}")"
  if is_alive "${pid}"; then
    echo "TERM portfolio-dashboard pid=${pid}"
    kill -TERM "${pid}" 2>/dev/null || true
  fi
}

if [[ "${DETACH}" != "1" ]]; then
  trap 'stop_portfolio_dashboard; stop_wss_monitors' EXIT
fi

start_portfolio_dashboard

start_wss_monitor binance-spot binance-wss-book-ticker "${BINANCE_SPOT_WSS_BIND}" "${BINANCE_WSS_SYMBOL}" spot /api/binance-wss-book-ticker
start_wss_monitor binance-perp binance-wss-book-ticker "${BINANCE_PERP_WSS_BIND}" "${BINANCE_WSS_SYMBOL}" usdm-perp /api/binance-wss-book-ticker
start_wss_monitor bybit-spot bybit-wss-book-ticker "${BYBIT_SPOT_WSS_BIND}" "${BYBIT_WSS_SYMBOL}" spot /api/bybit-wss-book-ticker
start_wss_monitor bybit-perp bybit-wss-book-ticker "${BYBIT_PERP_WSS_BIND}" "${BYBIT_WSS_SYMBOL}" linear-perp /api/bybit-wss-book-ticker
start_wss_monitor okx-spot okx-wss-book-ticker "${OKX_SPOT_WSS_BIND}" "${OKX_WSS_SYMBOL}" spot /api/okx-wss-book-ticker
start_wss_monitor okx-perp okx-wss-book-ticker "${OKX_PERP_WSS_BIND}" "${OKX_WSS_SYMBOL}" swap /api/okx-wss-book-ticker
start_wss_monitor bitget-spot bitget-wss-book-ticker "${BITGET_SPOT_WSS_BIND}" "${BITGET_WSS_SYMBOL}" spot /api/bitget-wss-book-ticker
start_wss_monitor bitget-perp bitget-wss-book-ticker "${BITGET_PERP_WSS_BIND}" "${BITGET_WSS_SYMBOL}" usdt-futures /api/bitget-wss-book-ticker
start_wss_monitor aster-perp aster-wss-book-ticker "${ASTER_PERP_WSS_BIND}" "${ASTER_WSS_SYMBOL}" usdt-futures /api/aster-wss-book-ticker
start_wss_monitor hyperliquid-perp hyperliquid-wss-book-ticker "${HYPERLIQUID_PERP_WSS_BIND}" "${HYPERLIQUID_WSS_SYMBOL}" perp /api/hyperliquid-wss-book-ticker

while IFS=$'\t' read -r pid name log_file status_url; do
  wait_for_wss_monitor "${name}" "${pid}" "${log_file}" "${status_url}"
done < "${WSS_PID_FILE}"

print_dashboards() {
  cat <<EOF

正式实盘 dashboard:
  系统导航:          http://${PORTFOLIO_BIND}/nav
  总组合看板:        http://${PORTFOLIO_BIND}/dashboard
  Binance basis:      http://127.0.0.1:8796/dashboard
  Bybit basis:        http://127.0.0.1:8797/dashboard
  OKX basis:          http://127.0.0.1:8798/dashboard
  Bitget basis:       http://127.0.0.1:8803/dashboard
  Aster basis:        http://127.0.0.1:8800/dashboard
  Hyperliquid basis:  http://127.0.0.1:8799/dashboard
  Funding arb:        http://127.0.0.1:8804/dashboard

WSS 前置 dashboard:
  Binance spot:       http://${BINANCE_SPOT_WSS_BIND}/dashboard
  Binance perp:       http://${BINANCE_PERP_WSS_BIND}/dashboard
  Bybit spot:         http://${BYBIT_SPOT_WSS_BIND}/dashboard
  Bybit perp:         http://${BYBIT_PERP_WSS_BIND}/dashboard
  OKX spot:           http://${OKX_SPOT_WSS_BIND}/dashboard
  OKX perp:           http://${OKX_PERP_WSS_BIND}/dashboard
  Bitget spot:        http://${BITGET_SPOT_WSS_BIND}/dashboard
  Bitget perp:        http://${BITGET_PERP_WSS_BIND}/dashboard
  Aster perp:         http://${ASTER_PERP_WSS_BIND}/dashboard
  Hyperliquid perp:   http://${HYPERLIQUID_PERP_WSS_BIND}/dashboard

实时日志:
  tail -f ${RUN_ROOT}/logs/realtime-feedback.log
  tail -f ${PREREQ_ROOT}/logs/arb-runtime-live.log

spot-perp-basis 常驻产物:
  ${RUN_ROOT}/resident-live/spot-perp-basis

cross-exchange-funding-arb 常驻产物:
  ${RUN_ROOT}/resident-live/cross-exchange-funding-arb

停止:
  ARB_RUNTIME_LIVE_ROOT=${RUN_ROOT} ARB_RUNTIME_LIVE_PREREQ_ROOT=${PREREQ_ROOT} scripts/stop-arb-runtime-live.sh
EOF
}

print_dashboards

LIVE_ENV=(
  BASIS_OBSERVER_ROOT="${RUN_ROOT}"
  BASIS_OBSERVER_LIVE_ACK=1
  BASIS_OBSERVER_FUNDING_ARB_MODE="${BASIS_OBSERVER_FUNDING_ARB_MODE:-resident}"
  BASIS_OBSERVER_FUNDING_ARB_RESIDENT_INTERVAL_SECS="${BASIS_OBSERVER_FUNDING_ARB_RESIDENT_INTERVAL_SECS:-60}"
  BASIS_OBSERVER_FUNDING_ARB_RESIDENT_MAX_LIVE_ENTRIES="${BASIS_OBSERVER_FUNDING_ARB_RESIDENT_MAX_LIVE_ENTRIES:-1}"
  BASIS_OBSERVER_FUNDING_ARB_RESIDENT_MAX_CYCLES="${BASIS_OBSERVER_FUNDING_ARB_RESIDENT_MAX_CYCLES:-}"
  BASIS_OBSERVER_FUNDING_SETTLEMENT_LEDGER="${BASIS_OBSERVER_FUNDING_SETTLEMENT_LEDGER:-${FUNDING_SETTLEMENT_LEDGER:-}}"
  BASIS_OBSERVER_FUNDING_SETTLEMENT_RAW_SNAPSHOT="${BASIS_OBSERVER_FUNDING_SETTLEMENT_RAW_SNAPSHOT:-${FUNDING_SETTLEMENT_RAW_SNAPSHOT:-}}"
  BINANCE_SPOT_WSS_MONITOR_URL="http://${BINANCE_SPOT_WSS_BIND}/api/binance-wss-book-ticker/status"
  BINANCE_PERP_WSS_MONITOR_URL="http://${BINANCE_PERP_WSS_BIND}/api/binance-wss-book-ticker/status"
  BYBIT_SPOT_WSS_MONITOR_URL="http://${BYBIT_SPOT_WSS_BIND}/api/bybit-wss-book-ticker/status"
  BYBIT_PERP_WSS_MONITOR_URL="http://${BYBIT_PERP_WSS_BIND}/api/bybit-wss-book-ticker/status"
  OKX_SPOT_WSS_MONITOR_URL="http://${OKX_SPOT_WSS_BIND}/api/okx-wss-book-ticker/status"
  OKX_PERP_WSS_MONITOR_URL="http://${OKX_PERP_WSS_BIND}/api/okx-wss-book-ticker/status"
  BITGET_SPOT_WSS_MONITOR_URL="http://${BITGET_SPOT_WSS_BIND}/api/bitget-wss-book-ticker/status"
  BITGET_PERP_WSS_MONITOR_URL="http://${BITGET_PERP_WSS_BIND}/api/bitget-wss-book-ticker/status"
  ASTER_PERP_WSS_MONITOR_URL="http://${ASTER_PERP_WSS_BIND}/api/aster-wss-book-ticker/status"
  HYPERLIQUID_PERP_WSS_MONITOR_URL="http://${HYPERLIQUID_PERP_WSS_BIND}/api/hyperliquid-wss-book-ticker/status"
)

if [[ "${DETACH}" == "1" ]]; then
  live_log="${LOG_DIR}/arb-runtime-live.log"
  nohup env "${LIVE_ENV[@]}" "${RUNTIME_BIN}" live --i-understand-live-orders >> "${live_log}" 2>&1 &
  live_pid="$!"
  printf '%s\n' "${live_pid}" > "${LIVE_PID_FILE}"
  echo
  echo "arb-runtime live started in background: pid=${live_pid} log=${live_log}"
  exit 0
fi

echo
echo "starting arb-runtime live in foreground; press Ctrl-C to stop."
env "${LIVE_ENV[@]}" "${RUNTIME_BIN}" live --i-understand-live-orders
