#!/usr/bin/env bash
set -euo pipefail

# 中文说明：该脚本只启动公开行情 WSS bookTicker monitor；不读取私有账户、不下单、不撤单、不转账、不签名。
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
RUNTIME_BIN="${REPO_ROOT}/target/debug/arb-runtime"

LOG_DIR="${WSS_BOOK_TICKER_LOG_DIR:-${REPO_ROOT}/target/wss-book-ticker-logs}"
PID_FILE="${LOG_DIR}/wss-book-ticker-monitors.pids"
RECONNECT_DELAY_SECS="${WSS_BOOK_TICKER_RECONNECT_DELAY_SECS:-2}"
BINANCE_BIND="${BINANCE_WSS_BOOK_TICKER_BIND:-127.0.0.1:8801}"
BINANCE_SYMBOL="${BINANCE_WSS_BOOK_TICKER_SYMBOL:-ALL_USDT}"
BINANCE_MARKET="${BINANCE_WSS_BOOK_TICKER_MARKET:-spot}"
DETACH="${WSS_BOOK_TICKER_DETACH:-0}"

PIDS=()
NAMES=()

usage() {
  cat <<'USAGE'
用法:
  scripts/start-wss-book-ticker-monitors.sh [binance]

默认不传参数时启动 binance。后续新增交易所时，沿用同一脚本增加交易所名称和绑定端口。

常用环境变量:
  BINANCE_WSS_BOOK_TICKER_BIND=127.0.0.1:8801
  BINANCE_WSS_BOOK_TICKER_SYMBOL=ALL_USDT
  BINANCE_WSS_BOOK_TICKER_MARKET=spot
  WSS_BOOK_TICKER_RECONNECT_DELAY_SECS=2
  WSS_BOOK_TICKER_LOG_DIR=target/wss-book-ticker-logs
  WSS_BOOK_TICKER_DETACH=1

示例:
  scripts/start-wss-book-ticker-monitors.sh
  scripts/start-wss-book-ticker-monitors.sh binance
  BINANCE_WSS_BOOK_TICKER_SYMBOL=ETHUSDT scripts/start-wss-book-ticker-monitors.sh
  WSS_BOOK_TICKER_DETACH=1 scripts/start-wss-book-ticker-monitors.sh
USAGE
}

cleanup() {
  if [[ "${DETACH}" == "1" || "${#PIDS[@]}" -eq 0 ]]; then
    return
  fi
  echo
  echo "stopping WSS bookTicker monitors..."
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
  MONITORS=("binance")
else
  MONITORS=("$@")
fi

for monitor in "${MONITORS[@]}"; do
  case "${monitor}" in
    binance) ;;
    *)
      echo "unknown WSS bookTicker monitor: ${monitor}" >&2
      usage >&2
      exit 2
      ;;
  esac
done

mkdir -p "${LOG_DIR}"
: > "${PID_FILE}"

echo "building arb-runtime..."
cargo build -p arb-runtime --manifest-path "${REPO_ROOT}/Cargo.toml"

start_monitor() {
  local name="$1"
  local command="$2"
  local bind_addr="$3"
  local symbol="$4"
  local market="$5"
  local log_file="${LOG_DIR}/${name}.log"

  echo "starting ${name}: http://${bind_addr}/dashboard"
  "${RUNTIME_BIN}" "${command}" \
    --bind "${bind_addr}" \
    --symbol "${symbol}" \
    --market "${market}" \
    --reconnect-delay-secs "${RECONNECT_DELAY_SECS}" \
    >>"${log_file}" 2>&1 &
  local pid="$!"
  PIDS+=("${pid}")
  NAMES+=("${name}")
  echo "${pid} ${name} ${log_file}" >> "${PID_FILE}"
  echo "  pid=${pid} log=${log_file}"
}

for monitor in "${MONITORS[@]}"; do
  case "${monitor}" in
    binance)
      start_monitor "binance-wss-book-ticker" "binance-wss-book-ticker" "${BINANCE_BIND}" "${BINANCE_SYMBOL}" "${BINANCE_MARKET}"
      ;;
  esac
done

echo
echo "started ${#PIDS[@]} WSS bookTicker monitor(s). pid file: ${PID_FILE}"
echo "中文说明：这些进程只访问公开 WSS/REST 行情；日志中不应出现密钥或私有账户字段。"

if [[ "${DETACH}" == "1" ]]; then
  echo "detached mode enabled; monitors keep running in background."
  exit 0
fi

echo "press Ctrl-C to stop all monitors."
wait "${PIDS[@]}"
