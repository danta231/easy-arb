#!/usr/bin/env bash
set -euo pipefail

# 中文说明：该脚本只启动公开行情 basis monitor，不读取私有账户、不下单、不撤单、不转账、不签名。
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
RUNTIME_BIN="${REPO_ROOT}/target/debug/arb-runtime"

LOG_DIR="${BASIS_MONITOR_LOG_DIR:-${REPO_ROOT}/target/basis-monitor-logs}"
PID_FILE="${LOG_DIR}/basis-monitors.pids"
INTERVAL_SECS="${BASIS_MONITOR_INTERVAL_SECS:-5}"
MIN_ABS_FUNDING_RATE="${BASIS_MONITOR_MIN_ABS_FUNDING_RATE:-0}"
MIN_NET_BPS="${BASIS_MONITOR_MIN_NET_BPS:-5}"
NOTIONAL_USD="${BASIS_MONITOR_NOTIONAL_USD:-100.00}"
SPOT_FEE_BPS="${BASIS_MONITOR_SPOT_FEE_BPS:-10}"
PERP_FEE_BPS="${BASIS_MONITOR_PERP_FEE_BPS:-5}"
SLIPPAGE_BPS="${BASIS_MONITOR_SLIPPAGE_BPS:-5}"
BINANCE_BIND="${BINANCE_BASIS_BIND:-127.0.0.1:8796}"
BYBIT_BIND="${BYBIT_BASIS_BIND:-127.0.0.1:8797}"
OKX_BIND="${OKX_BASIS_BIND:-127.0.0.1:8798}"
HYPERLIQUID_BIND="${HYPERLIQUID_BASIS_BIND:-127.0.0.1:8799}"
ASTER_BIND="${ASTER_BASIS_BIND:-127.0.0.1:8800}"
BITGET_BIND="${BITGET_BASIS_BIND:-127.0.0.1:8803}"
DETACH="${BASIS_MONITOR_DETACH:-0}"

PIDS=()
NAMES=()

usage() {
  cat <<'USAGE'
用法:
  scripts/start-basis-monitors.sh [binance] [bybit] [okx] [bitget] [hyperliquid] [aster]

默认不传参数时同时启动 binance、bybit、okx、bitget、hyperliquid 和 aster。

常用环境变量:
  BINANCE_BASIS_BIND=127.0.0.1:8796
  BYBIT_BASIS_BIND=127.0.0.1:8797
  OKX_BASIS_BIND=127.0.0.1:8798
  BITGET_BASIS_BIND=127.0.0.1:8803
  HYPERLIQUID_BASIS_BIND=127.0.0.1:8799
  ASTER_BASIS_BIND=127.0.0.1:8800
  BASIS_MONITOR_INTERVAL_SECS=5
  BASIS_MONITOR_MIN_ABS_FUNDING_RATE=0
  BASIS_MONITOR_MIN_NET_BPS=5
  BASIS_MONITOR_NOTIONAL_USD=100.00
  BASIS_MONITOR_LOG_DIR=target/basis-monitor-logs
  BASIS_MONITOR_DETACH=1

示例:
  scripts/start-basis-monitors.sh
  scripts/start-basis-monitors.sh bybit
  scripts/start-basis-monitors.sh okx
  scripts/start-basis-monitors.sh bitget
  scripts/start-basis-monitors.sh hyperliquid
  scripts/start-basis-monitors.sh aster
  BASIS_MONITOR_DETACH=1 scripts/start-basis-monitors.sh
USAGE
}

cleanup() {
  if [[ "${DETACH}" == "1" || "${#PIDS[@]}" -eq 0 ]]; then
    return
  fi
  echo
  echo "stopping basis monitors..."
  for pid in "${PIDS[@]}"; do
    kill "${pid}" 2>/dev/null || true
  done
  for pid in "${PIDS[@]}"; do
    wait "${pid}" 2>/dev/null || true
  done
}

trap cleanup INT TERM EXIT

if [[ "${1:-}" == "-h" || "${1:-}" == "--help" ]]; then
  usage
  exit 0
fi

if [[ "$#" -eq 0 ]]; then
  MONITORS=("binance" "bybit" "okx" "bitget" "hyperliquid" "aster")
else
  MONITORS=("$@")
fi

for monitor in "${MONITORS[@]}"; do
  case "${monitor}" in
    binance|bybit|okx|bitget|hyperliquid|aster) ;;
    *)
      echo "unknown monitor: ${monitor}" >&2
      usage >&2
      exit 2
      ;;
  esac
done

mkdir -p "${LOG_DIR}"
: > "${PID_FILE}"

echo "building arb-runtime..."
cargo build -p arb-runtime --manifest-path "${REPO_ROOT}/Cargo.toml"

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
  local name="$1"
  local command="$2"
  local bind_addr="$3"
  local status_path="$4"
  local log_file="${LOG_DIR}/${name}.log"

  echo "starting ${name}: http://${bind_addr}${status_path}"
  "${RUNTIME_BIN}" "${command}" --bind "${bind_addr}" "${COMMON_ARGS[@]}" >>"${log_file}" 2>&1 &
  local pid="$!"
  PIDS+=("${pid}")
  NAMES+=("${name}")
  echo "${pid} ${name} ${log_file}" >> "${PID_FILE}"
  echo "  pid=${pid} log=${log_file}"
}

for monitor in "${MONITORS[@]}"; do
  case "${monitor}" in
    binance)
      start_monitor "binance-basis-monitor" "binance-basis-monitor" "${BINANCE_BIND}" /api/basis/status
      ;;
    bybit)
      start_monitor "bybit-basis-monitor" "bybit-basis-monitor" "${BYBIT_BIND}" /api/bybit-basis/status
      ;;
    okx)
      start_monitor "okx-basis-monitor" "okx-basis-monitor" "${OKX_BIND}" /api/okx-basis/status
      ;;
    bitget)
      start_monitor "bitget-basis-monitor" "bitget-basis-monitor" "${BITGET_BIND}" /api/bitget-basis/status
      ;;
    hyperliquid)
      start_monitor "hyperliquid-basis-monitor" "hyperliquid-basis-monitor" "${HYPERLIQUID_BIND}" /api/hyperliquid-basis/status
      ;;
    aster)
      start_monitor "aster-basis-monitor" "aster-basis-monitor" "${ASTER_BIND}" /api/aster-basis/status
      ;;
  esac
done

echo
echo "started ${#PIDS[@]} monitor(s). pid file: ${PID_FILE}"
echo "中文说明：这些进程只访问公开市场数据；日志中不应出现密钥或私有账户字段。"

if [[ "${DETACH}" == "1" ]]; then
  echo "detached mode enabled; monitors keep running in background."
  exit 0
fi

echo "press Ctrl-C to stop all monitors."
wait "${PIDS[@]}"
