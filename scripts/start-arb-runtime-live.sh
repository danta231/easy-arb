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
  ARB_RUNTIME_LIVE_ROOT=target/arb-runtime/live # 实盘主运行目录，保存 resident 状态、机会和报告。
  ARB_RUNTIME_LIVE_PREREQ_ROOT=target/arb-runtime/live-prereq # WSS 前置 monitor 的日志和 pid 状态目录。
  ARB_RUNTIME_LIVE_ENV_FILE=.env.local # 可选 env 文件路径；等价于命令行 --env-file。
  ARB_RUNTIME_LIVE_DETACH=0 # 是否后台运行 arb-runtime live；1 表示 detach。
  ARB_RUNTIME_LIVE_PRECHECK_LOG_ENABLED=1 # 是否把启动脚本自身输出写入 live-prereq/logs/arb-runtime-live-precheck.log。
  ARB_RUNTIME_LIVE_WSS_READY_TIMEOUT_SECS=120 # 等待 WSS monitor 就绪的最长秒数。
  ARB_RUNTIME_LIVE_RECLAIM_STALE_MONITOR_PORTS=1 # 启动前是否回收本仓库 arb-runtime 残留 dashboard/WSS 端口；0 表示只报错。
  ARB_RUNTIME_LIVE_BUILD=1 # 启动前是否构建 arb-runtime 和 arb-wallet-signer。
  ARB_RUNTIME_LIVE_CONFIG=templates/personal_guarded_live.preflight.yaml # arb-runtime live 使用的风控和执行配置。
  ARB_RUNTIME_LIVE_INTERVAL_SECS=5 # observer 公开 monitor 轮询间隔秒数。
  ARB_RUNTIME_LIVE_MIN_NET_BPS=5 # 最小净收益阈值，单位 bps。
  ARB_RUNTIME_LOCAL_TZ=Asia/Shanghai # 面向人读的日志展示时区；默认 UTC+8。
  ARB_RUNTIME_LIVE_AUTO_ONCE_COOLDOWN_SECS=60 # 同一候选 auto-once 验证冷却秒数。
  ARB_RUNTIME_LIVE_VALIDATE_AUTO_ONCE=1 # 是否运行候选 auto-once 验证。
  ARB_RUNTIME_LIVE_DERISK_ONLY=0 # 事故处理模式；1 表示只启动 funding arb resident 一轮恢复/减仓，不启动 spot-perp 实盘 resident。
  ARB_RUNTIME_LIVE_CEX_WSS_SCOPE=all # Binance/Bybit/OKX/Bitget 全市场 WSS 覆盖范围；all 表示全部 USDT，target 表示只订阅 resident 实盘目标 symbol，custom 表示使用下面各交易所自定义值。
  ARB_RUNTIME_LIVE_TARGET_WSS_ENABLED=1 # 是否额外启动实盘 guard 专用 target WSS；启动不阻塞，真实下单前仍强制校验 Fresh quote。
  ARB_RUNTIME_LIVE_BINANCE_WSS_SYMBOL=BTCUSDT # CEX_WSS_SCOPE=custom 时的 Binance WSS monitor 订阅 symbol。
  ARB_RUNTIME_LIVE_BYBIT_WSS_SYMBOL=BTCUSDT # CEX_WSS_SCOPE=custom 时的 Bybit WSS monitor 订阅 symbol。
  ARB_RUNTIME_LIVE_OKX_WSS_SYMBOL=BTC-USDT # CEX_WSS_SCOPE=custom 时的 OKX WSS monitor 订阅 symbol。
  ARB_RUNTIME_LIVE_BITGET_WSS_SYMBOL=BTCUSDT # CEX_WSS_SCOPE=custom 时的 Bitget WSS monitor 订阅 symbol。
  ARB_RUNTIME_LIVE_ASTER_WSS_SYMBOL=ALL_USDT # Aster perp WSS monitor 订阅范围；ALL_USDT 表示全部 USDT 合约；启动不阻塞，策略侧按数据新鲜度 fail-closed。
  ARB_RUNTIME_LIVE_HYPERLIQUID_WSS_SYMBOL=ALL_USDT # Hyperliquid perp WSS monitor 订阅范围；ALL_USDT 表示全部永续合约；启动不阻塞，策略侧按数据新鲜度 fail-closed。
  ARB_RUNTIME_LIVE_PORTFOLIO_BIND=127.0.0.1:8805 # portfolio dashboard 监听地址。
  BASIS_OBSERVER_BASIS_RESIDENT_INTERVAL_SECS=60 # spot-perp-basis 常驻 runner 扫描间隔秒数。
  BASIS_OBSERVER_BASIS_RESIDENT_MAX_LIVE_ENTRIES=1 # spot-perp-basis 单轮最多新开实盘 entry 数。
  BASIS_OBSERVER_BASIS_RESIDENT_MAX_CONCURRENT_POSITIONS=1 # spot-perp-basis 最多同时持有的未平仓 position 数。
  BASIS_OBSERVER_BASIS_RESIDENT_MAX_TOTAL_NOTIONAL_USDT=10.00 # spot-perp-basis 总名义本金上限，单位 USDT。
  BASIS_OBSERVER_PERP_TARGET_LEVERAGE=1 # 所有永续交易所默认目标杠杆；实盘非 reduce-only 下单前会先设置该杠杆。
  BASIS_OBSERVER_BINANCE_USDM_LEVERAGE=1 # 可选覆盖 Binance USD-M 永续目标杠杆。
  BASIS_OBSERVER_BYBIT_LINEAR_LEVERAGE=1 # 可选覆盖 Bybit linear 永续目标杠杆。
  BASIS_OBSERVER_OKX_SWAP_LEVERAGE=1 # 可选覆盖 OKX swap 永续目标杠杆。
  BASIS_OBSERVER_BITGET_USDT_FUTURES_LEVERAGE=1 # 可选覆盖 Bitget USDT-FUTURES 目标杠杆。
  BASIS_OBSERVER_ASTER_PERP_LEVERAGE=1 # 可选覆盖 Aster USDT perp 目标杠杆。
  BASIS_OBSERVER_HYPERLIQUID_PERP_LEVERAGE=1 # 可选覆盖 Hyperliquid perp 目标杠杆。
  BASIS_OBSERVER_FUNDING_ARB_MODE=resident # cross-exchange-funding-arb 运行模式；resident 表示常驻运行。
  BASIS_OBSERVER_FUNDING_ARB_RESIDENT_INTERVAL_SECS=60 # cross-exchange-funding-arb 常驻 runner 扫描间隔秒数。
  BASIS_OBSERVER_FUNDING_ARB_RESIDENT_MAX_LIVE_ENTRIES=1 # cross-exchange-funding-arb 单轮最多新开实盘 entry 数。
  BASIS_OBSERVER_FUNDING_ARB_RESIDENT_MAX_CYCLES= # cross-exchange-funding-arb 最大循环次数；留空表示长期运行。
  BASIS_OBSERVER_FUNDING_SETTLEMENT_LEDGER= # 稳定结算账本输入路径；启用 raw snapshot 时必须留空。
  BASIS_OBSERVER_FUNDING_SETTLEMENT_RAW_SNAPSHOT= # 资金费率结算原始只读快照输出路径。
  ARB_RUNTIME_PORTFOLIO_ACCOUNT_SNAPSHOT=target/account_snapshot.json # portfolio dashboard 可选账户快照覆盖输入路径；留空时从 resident root 自动发现。
  ARB_RUNTIME_PORTFOLIO_POSITION_SNAPSHOT=target/position_snapshot.json # portfolio dashboard 可选仓位快照覆盖输入路径；留空时从 resident root 自动汇总。
  ASTER_USER=0x... # Aster 账户/user 地址，用于账户归属、查询和订单归属。
  ASTER_SIGNER=0x... # Aster 实际签名/API 地址，必须与 signer 私钥匹配。
  ASTER_SIGNER_PRIVATE=<local-secret> # Aster signer/API 地址对应私钥，只放本机 env。
  HYPERLIQUID_USER=0x... # Hyperliquid 账户/user 地址，用于账户归属、查询和订单归属。
  HYPERLIQUID_SIGNER=0x... # Hyperliquid 实际签名/API/agent 地址，必须与 signer 私钥匹配。
  HYPERLIQUID_SIGNER_PRIVATE=<local-secret> # Hyperliquid signer/API/agent 地址对应私钥，只放本机 env。

Aster 默认按 USDT 结算；Hyperliquid 默认按 USDC 结算，并默认从 Hyperliquid public meta 自动解析 asset id。
如果 user 地址和 signer/API 地址相同，也可以用 ASTER_ADDRESS + ASTER_PRIVATE_KEY、HYPERLIQUID_ADDRESS + HYPERLIQUID_PRIVATE_KEY。

正式实盘凭证请放在 shell 环境或 --env-file 指向的本地文件中，不要写入命令行。
USAGE
}

die() {
  echo "error: $*" >&2
  exit 1
}

local_log_timestamp() {
  local timestamp
  timestamp="$(TZ="${ARB_RUNTIME_LOCAL_TZ:-Asia/Shanghai}" date +%Y-%m-%dT%H:%M:%S%z)"
  printf '%s:%s\n' "${timestamp%??}" "${timestamp: -2}"
}

require_command() {
  command -v "$1" >/dev/null 2>&1 || die "missing required command: $1"
}

is_alive() {
  local pid="$1"
  [[ "${pid}" =~ ^[0-9]+$ ]] && kill -0 "${pid}" 2>/dev/null
}

bind_port() {
  local bind_addr="$1"
  local port="${bind_addr##*:}"
  [[ "${port}" =~ ^[0-9]+$ ]] || return 1
  printf '%s\n' "${port}"
}

pid_is_recorded_by_live_prereq() {
  local needle="$1"
  local pid
  local _name
  local _log_file
  local _status_url
  local _ready_symbol
  local _required

  if [[ -s "${PORTFOLIO_PID_FILE:-}" ]]; then
    pid="$(sed -n '1p' "${PORTFOLIO_PID_FILE}" 2>/dev/null || true)"
    [[ "${pid}" == "${needle}" ]] && return 0
  fi
  [[ -s "${WSS_PID_FILE:-}" ]] || return 1
  while IFS=$'\t' read -r pid _name _log_file _status_url _ready_symbol _required; do
    [[ "${pid}" == "${needle}" ]] && return 0
  done < "${WSS_PID_FILE}"
  return 1
}

pid_looks_like_repo_arb_runtime() {
  local pid="$1"
  local open_files

  command -v lsof >/dev/null 2>&1 || return 1
  open_files="$(lsof -nP -p "${pid}" 2>/dev/null || true)"
  [[ -n "${open_files}" ]] || return 1
  if [[ "${open_files}" == *"${REPO_ROOT}/target/"*"arb-runtime"* ]]; then
    return 0
  fi
  [[ -n "${RUNTIME_BIN:-}" && "${open_files}" == *"${RUNTIME_BIN}"* ]]
}

wait_for_pid_exit() {
  local pid="$1"
  local timeout_secs="$2"
  local deadline="$((SECONDS + timeout_secs))"

  while (( SECONDS <= deadline )); do
    if ! is_alive "${pid}"; then
      return 0
    fi
    sleep 1
  done
  return 1
}

reclaim_stale_runtime_port() {
  local label="$1"
  local bind_addr="$2"
  local port
  local pid

  [[ "${RECLAIM_STALE_MONITOR_PORTS:-1}" == "1" ]] || return 0
  command -v lsof >/dev/null 2>&1 || return 0
  port="$(bind_port "${bind_addr}")" || return 0

  while IFS= read -r pid; do
    [[ "${pid}" =~ ^[0-9]+$ ]] || continue
    pid_is_recorded_by_live_prereq "${pid}" && continue
    if pid_looks_like_repo_arb_runtime "${pid}"; then
      echo "reclaiming stale runtime port: label=${label} bind=${bind_addr} pid=${pid}"
      kill -TERM "${pid}" 2>/dev/null || true
      if ! wait_for_pid_exit "${pid}" 3; then
        echo "stale runtime process did not exit after TERM; sending KILL: label=${label} bind=${bind_addr} pid=${pid}"
        kill -KILL "${pid}" 2>/dev/null || true
      fi
    else
      die "cannot bind ${label} on ${bind_addr}: address already in use by pid=${pid}; not a managed ${REPO_ROOT}/target arb-runtime process"
    fi
  done < <(lsof -nP -iTCP:"${port}" -sTCP:LISTEN -t 2>/dev/null || true)
}

tail_log_on_error() {
  local log_file="$1"
  [[ -f "${log_file}" ]] && tail -n 40 "${log_file}" >&2 || true
}

DETACH="${ARB_RUNTIME_LIVE_DETACH:-0}"
ENV_FILE="${ARB_RUNTIME_LIVE_ENV_FILE:-}"
BUILD="${ARB_RUNTIME_LIVE_BUILD:-1}"
DETACH_FROM_CLI=0
BUILD_FROM_CLI=0

while [[ "$#" -gt 0 ]]; do
  case "$1" in
    --detach)
      DETACH="1"
      DETACH_FROM_CLI=1
      shift
      ;;
    --env-file)
      [[ "$#" -ge 2 ]] || die "--env-file requires a path"
      ENV_FILE="$2"
      shift 2
      ;;
    --no-build)
      BUILD="0"
      BUILD_FROM_CLI=1
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

if [[ "${DETACH_FROM_CLI}" != "1" ]]; then
  DETACH="${ARB_RUNTIME_LIVE_DETACH:-${DETACH}}"
fi
if [[ "${BUILD_FROM_CLI}" != "1" ]]; then
  BUILD="${ARB_RUNTIME_LIVE_BUILD:-${BUILD}}"
fi
case "${DETACH}" in
  0|1) ;;
  *) die "ARB_RUNTIME_LIVE_DETACH must be 0 or 1" ;;
esac
case "${BUILD}" in
  0|1) ;;
  *) die "ARB_RUNTIME_LIVE_BUILD must be 0 or 1" ;;
esac

set_default_env() {
  local name="$1"
  local value="${2:-}"
  if [[ -n "${value}" && -z "${!name-}" ]]; then
    printf -v "${name}" '%s' "${value}"
    export "${name}"
  fi
}

apply_simplified_wallet_env_aliases() {
  local aster_user_address="${ASTER_USER_ADDRESS:-${ASTER_ACCOUNT_ADDRESS:-${ASTER_ADDRESS:-}}}"
  local aster_signer_address="${ASTER_SIGNER_ADDRESS:-${ASTER_API_ADDRESS:-${ASTER_ADDRESS:-}}}"
  local aster_signer_private="${ASTER_SIGNER_PRIVATE_KEY:-${ASTER_SIGNER_PRIVATE:-${ASTER_PRIVATE_KEY:-}}}"
  set_default_env ASTER_USER "${BASIS_OBSERVER_ASTER_USER:-${aster_user_address}}"
  set_default_env ASTER_SIGNER "${BASIS_OBSERVER_ASTER_SIGNER:-${aster_signer_address}}"
  set_default_env BASIS_OBSERVER_ASTER_USER "${ASTER_USER:-}"
  set_default_env BASIS_OBSERVER_ASTER_SIGNER "${ASTER_SIGNER:-}"
  set_default_env ASTER_SIGNER_PRIVATE_KEY "${aster_signer_private}"

  local hyperliquid_user_address="${HYPERLIQUID_USER_ADDRESS:-${HYPERLIQUID_ACCOUNT_ADDRESS:-${HYPERLIQUID_ADDRESS:-}}}"
  local hyperliquid_signer_address="${HYPERLIQUID_SIGNER_ADDRESS:-${HYPERLIQUID_API_ADDRESS:-${HYPERLIQUID_SIGNER:-}}}"
  local hyperliquid_signer_private="${HYPERLIQUID_SIGNER_PRIVATE_KEY:-${HYPERLIQUID_SIGNER_PRIVATE:-${HYPERLIQUID_PRIVATE_KEY:-}}}"
  set_default_env HYPERLIQUID_USER "${BASIS_OBSERVER_HYPERLIQUID_USER:-${hyperliquid_user_address}}"
  set_default_env BASIS_OBSERVER_HYPERLIQUID_USER "${HYPERLIQUID_USER:-}"
  set_default_env HYPERLIQUID_AGENT "${hyperliquid_signer_address}"
  set_default_env HYPERLIQUID_AGENT_PRIVATE_KEY "${hyperliquid_signer_private}"
}

apply_simplified_wallet_env_aliases

apply_default_perp_leverage_env() {
  set_default_env BASIS_OBSERVER_PERP_TARGET_LEVERAGE "1"
  set_default_env ARB_RUNTIME_PERP_TARGET_LEVERAGE "${BASIS_OBSERVER_PERP_TARGET_LEVERAGE}"
  set_default_env BASIS_OBSERVER_BINANCE_USDM_LEVERAGE "${BASIS_OBSERVER_PERP_TARGET_LEVERAGE}"
  set_default_env BASIS_OBSERVER_BYBIT_LINEAR_LEVERAGE "${BASIS_OBSERVER_PERP_TARGET_LEVERAGE}"
  set_default_env BASIS_OBSERVER_OKX_SWAP_LEVERAGE "${BASIS_OBSERVER_PERP_TARGET_LEVERAGE}"
  set_default_env BASIS_OBSERVER_BITGET_USDT_FUTURES_LEVERAGE "${BASIS_OBSERVER_PERP_TARGET_LEVERAGE}"
  set_default_env BASIS_OBSERVER_ASTER_PERP_LEVERAGE "${BASIS_OBSERVER_PERP_TARGET_LEVERAGE}"
  set_default_env BASIS_OBSERVER_HYPERLIQUID_PERP_LEVERAGE "${BASIS_OBSERVER_PERP_TARGET_LEVERAGE}"
}

apply_default_perp_leverage_env

require_command cargo
require_command curl
require_command jq

RUN_ROOT="${ARB_RUNTIME_LIVE_ROOT:-${REPO_ROOT}/target/arb-runtime/live}"
PREREQ_ROOT="${ARB_RUNTIME_LIVE_PREREQ_ROOT:-${REPO_ROOT}/target/arb-runtime/live-prereq}"
LIVE_PAUSE_FILE="${ARB_RUNTIME_LIVE_PAUSE_FILE:-${RUN_ROOT}/LIVE_TRADING_PAUSED}"
LOG_DIR="${PREREQ_ROOT}/logs"
STATE_DIR="${PREREQ_ROOT}/state"
WSS_PID_FILE="${STATE_DIR}/wss-book-ticker.pids"
LIVE_PID_FILE="${STATE_DIR}/arb-runtime-live.pid"
PORTFOLIO_PID_FILE="${STATE_DIR}/portfolio-dashboard.pid"
PRECHECK_LOG="${LOG_DIR}/arb-runtime-live-precheck.log"
RUNTIME_BIN="${ARB_RUNTIME_LIVE_BIN:-${REPO_ROOT}/target/debug/arb-runtime}"
WSS_READY_TIMEOUT_SECS="${ARB_RUNTIME_LIVE_WSS_READY_TIMEOUT_SECS:-120}"
WSS_RECONNECT_DELAY_SECS="${ARB_RUNTIME_LIVE_WSS_RECONNECT_DELAY_SECS:-2}"
RECLAIM_STALE_MONITOR_PORTS="${ARB_RUNTIME_LIVE_RECLAIM_STALE_MONITOR_PORTS:-1}"
PORTFOLIO_BIND="${ARB_RUNTIME_LIVE_PORTFOLIO_BIND:-127.0.0.1:8805}"
LIVE_CONFIG="${ARB_RUNTIME_LIVE_CONFIG:-${BASIS_OBSERVER_CONFIG:-templates/personal_guarded_live.preflight.yaml}}"
LIVE_INTERVAL_SECS="${ARB_RUNTIME_LIVE_INTERVAL_SECS:-${BASIS_OBSERVER_INTERVAL_SECS:-5}}"
LIVE_MIN_NET_BPS="${ARB_RUNTIME_LIVE_MIN_NET_BPS:-${BASIS_OBSERVER_MIN_NET_BPS:-5}}"
LIVE_AUTO_ONCE_COOLDOWN_SECS="${ARB_RUNTIME_LIVE_AUTO_ONCE_COOLDOWN_SECS:-${BASIS_OBSERVER_AUTO_ONCE_COOLDOWN_SECS:-60}}"
LIVE_VALIDATE_AUTO_ONCE="${ARB_RUNTIME_LIVE_VALIDATE_AUTO_ONCE:-${BASIS_OBSERVER_VALIDATE_AUTO_ONCE:-1}}"
LIVE_DERISK_ONLY="${ARB_RUNTIME_LIVE_DERISK_ONLY:-0}"
LIVE_STRATEGIES="${BASIS_OBSERVER_STRATEGIES:-spot-perp-basis,cross-exchange-funding-arb}"
LIVE_SPOT_PERP_BASIS_MODE="${BASIS_OBSERVER_SPOT_PERP_BASIS_MODE:-resident}"
LIVE_FUNDING_ARB_MODE="${BASIS_OBSERVER_FUNDING_ARB_MODE:-resident}"
LIVE_FUNDING_ARB_RESIDENT_MAX_CYCLES="${BASIS_OBSERVER_FUNDING_ARB_RESIDENT_MAX_CYCLES:-}"

case "${LIVE_DERISK_ONLY}" in
  0|1) ;;
  *) die "ARB_RUNTIME_LIVE_DERISK_ONLY must be 0 or 1" ;;
esac
if [[ "${LIVE_DERISK_ONLY}" == "1" ]]; then
  LIVE_STRATEGIES="cross-exchange-funding-arb"
  LIVE_SPOT_PERP_BASIS_MODE="auto-once"
  LIVE_FUNDING_ARB_MODE="resident"
  LIVE_FUNDING_ARB_RESIDENT_MAX_CYCLES="1"
fi

if [[ -e "${LIVE_PAUSE_FILE}" && "${ARB_RUNTIME_LIVE_IGNORE_PAUSE:-0}" != "1" ]]; then
  die "live trading is paused by ${LIVE_PAUSE_FILE}; remove the file only after exchange-side risk is flat"
fi

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
TARGET_BINANCE_SPOT_WSS_BIND="${ARB_RUNTIME_LIVE_TARGET_BINANCE_SPOT_WSS_BIND:-127.0.0.1:8816}"
TARGET_BINANCE_PERP_WSS_BIND="${ARB_RUNTIME_LIVE_TARGET_BINANCE_PERP_WSS_BIND:-127.0.0.1:8817}"
TARGET_BYBIT_SPOT_WSS_BIND="${ARB_RUNTIME_LIVE_TARGET_BYBIT_SPOT_WSS_BIND:-127.0.0.1:8818}"
TARGET_BYBIT_PERP_WSS_BIND="${ARB_RUNTIME_LIVE_TARGET_BYBIT_PERP_WSS_BIND:-127.0.0.1:8819}"
TARGET_OKX_SPOT_WSS_BIND="${ARB_RUNTIME_LIVE_TARGET_OKX_SPOT_WSS_BIND:-127.0.0.1:8820}"
TARGET_OKX_PERP_WSS_BIND="${ARB_RUNTIME_LIVE_TARGET_OKX_PERP_WSS_BIND:-127.0.0.1:8821}"
TARGET_BITGET_SPOT_WSS_BIND="${ARB_RUNTIME_LIVE_TARGET_BITGET_SPOT_WSS_BIND:-127.0.0.1:8822}"
TARGET_BITGET_PERP_WSS_BIND="${ARB_RUNTIME_LIVE_TARGET_BITGET_PERP_WSS_BIND:-127.0.0.1:8823}"

BINANCE_BASIS_READY_SYMBOL="BTCUSDT"
BYBIT_BASIS_READY_SYMBOL="BTCUSDT"
OKX_BASIS_READY_SYMBOL="BTC-USDT"
BITGET_BASIS_READY_SYMBOL="BTCUSDT"

CEX_WSS_SCOPE="${ARB_RUNTIME_LIVE_CEX_WSS_SCOPE:-all}"
TARGET_WSS_ENABLED="${ARB_RUNTIME_LIVE_TARGET_WSS_ENABLED:-1}"
case "${CEX_WSS_SCOPE}" in
  target)
    BINANCE_WSS_SYMBOL="${BINANCE_BASIS_READY_SYMBOL}"
    BYBIT_WSS_SYMBOL="${BYBIT_BASIS_READY_SYMBOL}"
    OKX_WSS_SYMBOL="${OKX_BASIS_READY_SYMBOL}"
    BITGET_WSS_SYMBOL="${BITGET_BASIS_READY_SYMBOL}"
    ;;
  all)
    BINANCE_WSS_SYMBOL="${ARB_RUNTIME_LIVE_BINANCE_WSS_SYMBOL:-ALL_USDT}"
    BYBIT_WSS_SYMBOL="${ARB_RUNTIME_LIVE_BYBIT_WSS_SYMBOL:-ALL_USDT}"
    OKX_WSS_SYMBOL="${ARB_RUNTIME_LIVE_OKX_WSS_SYMBOL:-ALL_USDT}"
    BITGET_WSS_SYMBOL="${ARB_RUNTIME_LIVE_BITGET_WSS_SYMBOL:-ALL_USDT}"
    ;;
  custom)
    BINANCE_WSS_SYMBOL="${ARB_RUNTIME_LIVE_BINANCE_WSS_SYMBOL:-${BINANCE_BASIS_READY_SYMBOL}}"
    BYBIT_WSS_SYMBOL="${ARB_RUNTIME_LIVE_BYBIT_WSS_SYMBOL:-${BYBIT_BASIS_READY_SYMBOL}}"
    OKX_WSS_SYMBOL="${ARB_RUNTIME_LIVE_OKX_WSS_SYMBOL:-${OKX_BASIS_READY_SYMBOL}}"
    BITGET_WSS_SYMBOL="${ARB_RUNTIME_LIVE_BITGET_WSS_SYMBOL:-${BITGET_BASIS_READY_SYMBOL}}"
    ;;
  *)
    die "ARB_RUNTIME_LIVE_CEX_WSS_SCOPE must be target, all, or custom"
    ;;
esac
case "${TARGET_WSS_ENABLED}" in
  0|1) ;;
  *) die "ARB_RUNTIME_LIVE_TARGET_WSS_ENABLED must be 0 or 1" ;;
esac
case "${LIVE_VALIDATE_AUTO_ONCE}" in
  0|1) ;;
  *) die "ARB_RUNTIME_LIVE_VALIDATE_AUTO_ONCE must be 0 or 1" ;;
esac
ASTER_WSS_SYMBOL="${ARB_RUNTIME_LIVE_ASTER_WSS_SYMBOL:-ALL_USDT}"
HYPERLIQUID_WSS_SYMBOL="${ARB_RUNTIME_LIVE_HYPERLIQUID_WSS_SYMBOL:-ALL_USDT}"

mkdir -p "${LOG_DIR}" "${STATE_DIR}"

if [[ "${ARB_RUNTIME_LIVE_PRECHECK_LOG_ENABLED:-1}" != "0" ]]; then
  {
    echo
    echo "=== arb-runtime live precheck $(local_log_timestamp) ==="
    echo "repo_root=${REPO_ROOT}"
    echo "run_root=${RUN_ROOT}"
    echo "prereq_root=${PREREQ_ROOT}"
    echo "detach=${DETACH} build=${BUILD} config=${LIVE_CONFIG} interval_secs=${LIVE_INTERVAL_SECS} min_net_bps=${LIVE_MIN_NET_BPS} strategies=${LIVE_STRATEGIES} derisk_only=${LIVE_DERISK_ONLY} cex_wss_scope=${CEX_WSS_SCOPE} target_wss_enabled=${TARGET_WSS_ENABLED}"
  } >> "${PRECHECK_LOG}"
  exec > >(tee -a "${PRECHECK_LOG}") 2>&1
fi

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
  echo "building arb-wallet-signer..."
  cargo build -p arb-wallet-signer --manifest-path "${REPO_ROOT}/Cargo.toml"
fi

start_wss_monitor() {
  local name="$1"
  local command_name="$2"
  local bind_addr="$3"
  local symbol="$4"
  local market="$5"
  local api_prefix="$6"
  local ready_symbol="${7:-}"
  local required="${8:-1}"
  local ready_symbol_field
  local log_file="${LOG_DIR}/${name}.log"
  local status_url="http://${bind_addr}${api_prefix}/status"
  local pid

  reclaim_stale_runtime_port "${name}" "${bind_addr}"
  echo "starting ${name}: http://${bind_addr}/dashboard"
  nohup env ARB_RUNTIME_ENABLE_LEGACY_COMMANDS=1 "${RUNTIME_BIN}" "${command_name}" \
    --bind "${bind_addr}" \
    --symbol "${symbol}" \
    --market "${market}" \
    --reconnect-delay-secs "${WSS_RECONNECT_DELAY_SECS}" \
    >> "${log_file}" 2>&1 &
  pid="$!"
  ready_symbol_field="${ready_symbol:-__none__}"
  printf '%s\t%s\t%s\t%s\t%s\t%s\n' "${pid}" "${name}" "${log_file}" "${status_url}" "${ready_symbol_field}" "${required}" >> "${WSS_PID_FILE}"
  echo "  pid=${pid} log=${log_file}"
}

wss_quote_ready_for_symbol() {
  local body="$1"
  local symbol="$2"

  [[ -z "${symbol}" ]] && return 0
  printf '%s\n' "${body}" | jq -e --arg symbol "${symbol}" '
    def non_empty_string: type == "string" and length > 0;
    any((.rows // [])[]?;
      .symbol == $symbol
      and .freshness_status == "Fresh"
      and ((.source_event_id // "") | test(":wss-book(-ticker|Ticker):"))
      and ((.source_sequence // "") | tostring | test("^[0-9]+$"))
      and ((.best_bid // "") | non_empty_string)
      and ((.best_ask // "") | non_empty_string)
      and ((.bid_size // "") | non_empty_string)
      and ((.ask_size // "") | non_empty_string)
    )
  ' >/dev/null 2>&1
}

wait_for_wss_monitor() {
  local name="$1"
  local pid="$2"
  local log_file="$3"
  local status_url="$4"
  local ready_symbol="${5:-}"
  local required="${6:-1}"
  local deadline="$((SECONDS + WSS_READY_TIMEOUT_SECS))"
  local body
  local status
  local total_rows
  local wss_update_count

  if [[ "${required}" != "1" ]]; then
    echo "wss_best_effort name=${name} status_url=${status_url} ready_symbol=${ready_symbol:-none}"
    return 0
  fi

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
        if wss_quote_ready_for_symbol "${body}" "${ready_symbol}"; then
          echo "wss_ready name=${name} status_url=${status_url} rows=${total_rows} wss_updates=${wss_update_count} ready_symbol=${ready_symbol:-none}"
          return 0
        fi
      fi
    fi
    sleep 1
  done

  echo "error: ${name} did not become healthy within ${WSS_READY_TIMEOUT_SECS}s: ${status_url}; ready_symbol=${ready_symbol:-none}" >&2
  tail_log_on_error "${log_file}"
  return 1
}

stop_wss_monitors() {
  [[ -s "${WSS_PID_FILE}" ]] || return 0
  local pid
  local name
  local log_file
  local status_url
  local ready_symbol
  local required
  while IFS=$'\t' read -r pid name log_file status_url ready_symbol required; do
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

  reclaim_stale_runtime_port "portfolio-dashboard" "${PORTFOLIO_BIND}"
  echo "starting portfolio dashboard: http://${PORTFOLIO_BIND}/dashboard"
  echo "starting error logs dashboard: http://${PORTFOLIO_BIND}/errors"
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

resident_process_alive() {
  local expected_name="$1"
  local basis_pid_file="${RUN_ROOT}/state/basis-observer.pids"
  local pid
  local name
  local _log_file

  [[ -s "${basis_pid_file}" ]] || return 1
  while IFS=$'\t' read -r pid name _log_file; do
    if [[ "${name}" == "${expected_name}" ]] && is_alive "${pid}"; then
      return 0
    fi
  done < "${basis_pid_file}"
  return 1
}

safe_resident_lock_artifacts() {
  local summary_path="$1"
  local events_path="$2"
  local output_root
  local non_empty_private_artifact

  output_root="$(dirname "${events_path}")"
  non_empty_private_artifact="$(find "${output_root}" -type f \( \
    -name 'mutable_receipts.jsonl' -o \
    -name 'private_confirmations.jsonl' -o \
    -name 'execution_reports.jsonl' -o \
    -name '*position_state*.json' -o \
    -name '*positions.jsonl' \
  \) -size +0c -print -quit 2>/dev/null || true)"
  [[ -n "${non_empty_private_artifact}" ]] && return 1

  if [[ -s "${events_path}" ]] && jq -s -e '
    def as_count:
      if type == "number" then .
      elif type == "string" then (tonumber? // 0)
      elif type == "array" then length
      else 0 end;
    any(.[]; (
      (((.submitted_receipt_count // .submitted_receipts // 0) | as_count) > 0)
      or (((.private_confirmation_count // .private_confirmations // 0) | as_count) > 0)
      or ((.residual_risk // null) != null)
      or ((.event_type // "") == "position_opened")
      or ((.event_type // "") == "position_unknown")
    ))
  ' "${events_path}" >/dev/null 2>&1; then
    return 1
  fi

  if [[ -s "${summary_path}" ]] && jq -e '
    def as_count:
      if type == "number" then .
      elif type == "string" then (tonumber? // 0)
      elif type == "array" then length
      else 0 end;
    (((.open_position_count // 0) | as_count) != 0)
    or (((.unknown_position_count // 0) | as_count) != 0)
    or (((.live_entry_count // 0) | as_count) != 0)
  ' "${summary_path}" >/dev/null 2>&1; then
    return 1
  fi

  [[ -s "${summary_path}" || -s "${events_path}" ]]
}

cleanup_safe_stale_resident_lock() {
  local lock_path="$1"
  local summary_path="$2"
  local events_path="$3"
  local process_name="$4"
  local label="$5"
  local archived_lock

  [[ -e "${lock_path}" ]] || return 0
  if resident_process_alive "${process_name}"; then
    echo "resident_lock_kept label=${label} reason=process_alive lock=${lock_path}"
    return 0
  fi
  archived_lock="${lock_path}.stale.$(date -u +%Y%m%dT%H%M%SZ).$$"
  if safe_resident_lock_artifacts "${summary_path}" "${events_path}"; then
    echo "resident_lock_archived label=${label} reason=stale_safe_state lock=${lock_path} archived=${archived_lock}"
    mv "${lock_path}" "${archived_lock}"
    return 0
  fi
  echo "resident_lock_archived label=${label} reason=stale_state_requires_resident_recovery lock=${lock_path} archived=${archived_lock}"
  mv "${lock_path}" "${archived_lock}"
}

cleanup_safe_stale_resident_locks() {
  cleanup_safe_stale_resident_lock \
    "${RUN_ROOT}/resident-live/spot-perp-basis/multi_venue_resident_live.lock" \
    "${RUN_ROOT}/resident-live/spot-perp-basis/multi_venue_resident_live_summary.json" \
    "${RUN_ROOT}/resident-live/spot-perp-basis/multi_venue_resident_live_events.jsonl" \
    "spot-perp-basis-resident-live" \
    "spot-perp-basis"
  cleanup_safe_stale_resident_lock \
    "${RUN_ROOT}/resident-live/cross-exchange-funding-arb/funding_arb_resident_live.lock" \
    "${RUN_ROOT}/resident-live/cross-exchange-funding-arb/funding_arb_resident_live_summary.json" \
    "${RUN_ROOT}/resident-live/cross-exchange-funding-arb/funding_arb_resident_live_events.jsonl" \
    "funding-arb-resident-live" \
    "cross-exchange-funding-arb"
}

cleanup_safe_stale_resident_locks

if [[ "${DETACH}" != "1" ]]; then
  trap 'stop_portfolio_dashboard; stop_wss_monitors' EXIT
fi

start_portfolio_dashboard

start_wss_monitor binance-spot binance-wss-book-ticker "${BINANCE_SPOT_WSS_BIND}" "${BINANCE_WSS_SYMBOL}" spot /api/binance-wss-book-ticker "" 0
start_wss_monitor binance-perp binance-wss-book-ticker "${BINANCE_PERP_WSS_BIND}" "${BINANCE_WSS_SYMBOL}" usdm-perp /api/binance-wss-book-ticker "" 0
start_wss_monitor bybit-spot bybit-wss-book-ticker "${BYBIT_SPOT_WSS_BIND}" "${BYBIT_WSS_SYMBOL}" spot /api/bybit-wss-book-ticker "" 0
start_wss_monitor bybit-perp bybit-wss-book-ticker "${BYBIT_PERP_WSS_BIND}" "${BYBIT_WSS_SYMBOL}" linear-perp /api/bybit-wss-book-ticker "" 0
start_wss_monitor okx-spot okx-wss-book-ticker "${OKX_SPOT_WSS_BIND}" "${OKX_WSS_SYMBOL}" spot /api/okx-wss-book-ticker "" 0
start_wss_monitor okx-perp okx-wss-book-ticker "${OKX_PERP_WSS_BIND}" "${OKX_WSS_SYMBOL}" swap /api/okx-wss-book-ticker "" 0
start_wss_monitor bitget-spot bitget-wss-book-ticker "${BITGET_SPOT_WSS_BIND}" "${BITGET_WSS_SYMBOL}" spot /api/bitget-wss-book-ticker "" 0
start_wss_monitor bitget-perp bitget-wss-book-ticker "${BITGET_PERP_WSS_BIND}" "${BITGET_WSS_SYMBOL}" usdt-futures /api/bitget-wss-book-ticker "" 0
start_wss_monitor aster-perp aster-wss-book-ticker "${ASTER_PERP_WSS_BIND}" "${ASTER_WSS_SYMBOL}" usdt-futures /api/aster-wss-book-ticker "" 0
start_wss_monitor hyperliquid-perp hyperliquid-wss-book-ticker "${HYPERLIQUID_PERP_WSS_BIND}" "${HYPERLIQUID_WSS_SYMBOL}" perp /api/hyperliquid-wss-book-ticker "" 0

if [[ "${TARGET_WSS_ENABLED}" == "1" ]]; then
  start_wss_monitor target-binance-spot binance-wss-book-ticker "${TARGET_BINANCE_SPOT_WSS_BIND}" "${BINANCE_BASIS_READY_SYMBOL}" spot /api/binance-wss-book-ticker "${BINANCE_BASIS_READY_SYMBOL}" 0
  start_wss_monitor target-binance-perp binance-wss-book-ticker "${TARGET_BINANCE_PERP_WSS_BIND}" "${BINANCE_BASIS_READY_SYMBOL}" usdm-perp /api/binance-wss-book-ticker "${BINANCE_BASIS_READY_SYMBOL}" 0
  start_wss_monitor target-bybit-spot bybit-wss-book-ticker "${TARGET_BYBIT_SPOT_WSS_BIND}" "${BYBIT_BASIS_READY_SYMBOL}" spot /api/bybit-wss-book-ticker "${BYBIT_BASIS_READY_SYMBOL}" 0
  start_wss_monitor target-bybit-perp bybit-wss-book-ticker "${TARGET_BYBIT_PERP_WSS_BIND}" "${BYBIT_BASIS_READY_SYMBOL}" linear-perp /api/bybit-wss-book-ticker "${BYBIT_BASIS_READY_SYMBOL}" 0
  start_wss_monitor target-okx-spot okx-wss-book-ticker "${TARGET_OKX_SPOT_WSS_BIND}" "${OKX_BASIS_READY_SYMBOL}" spot /api/okx-wss-book-ticker "${OKX_BASIS_READY_SYMBOL}" 0
  start_wss_monitor target-okx-perp okx-wss-book-ticker "${TARGET_OKX_PERP_WSS_BIND}" "${OKX_BASIS_READY_SYMBOL}" swap /api/okx-wss-book-ticker "${OKX_BASIS_READY_SYMBOL}" 0
  start_wss_monitor target-bitget-spot bitget-wss-book-ticker "${TARGET_BITGET_SPOT_WSS_BIND}" "${BITGET_BASIS_READY_SYMBOL}" spot /api/bitget-wss-book-ticker "${BITGET_BASIS_READY_SYMBOL}" 0
  start_wss_monitor target-bitget-perp bitget-wss-book-ticker "${TARGET_BITGET_PERP_WSS_BIND}" "${BITGET_BASIS_READY_SYMBOL}" usdt-futures /api/bitget-wss-book-ticker "${BITGET_BASIS_READY_SYMBOL}" 0
fi

while IFS=$'\t' read -r pid name log_file status_url ready_symbol required; do
  [[ "${ready_symbol:-}" == "__none__" ]] && ready_symbol=""
  wait_for_wss_monitor "${name}" "${pid}" "${log_file}" "${status_url}" "${ready_symbol:-}" "${required:-1}"
done < "${WSS_PID_FILE}"

print_dashboards() {
  cat <<EOF

正式实盘 dashboard:
  系统导航:          http://${PORTFOLIO_BIND}/nav
  总组合看板:        http://${PORTFOLIO_BIND}/dashboard
  错误日志:          http://${PORTFOLIO_BIND}/errors
  Binance basis:      http://127.0.0.1:8796/dashboard
  Bybit basis:        http://127.0.0.1:8797/dashboard
  OKX basis:          http://127.0.0.1:8798/dashboard
  Bitget basis:       http://127.0.0.1:8803/dashboard
  Aster basis:        http://127.0.0.1:8800/dashboard
  Hyperliquid basis:  http://127.0.0.1:8799/dashboard
  Funding arb:        http://127.0.0.1:8804/dashboard

全市场 WSS 覆盖 dashboard:
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

实盘 guard target WSS dashboard:
  Binance spot:       http://${TARGET_BINANCE_SPOT_WSS_BIND}/dashboard
  Binance perp:       http://${TARGET_BINANCE_PERP_WSS_BIND}/dashboard
  Bybit spot:         http://${TARGET_BYBIT_SPOT_WSS_BIND}/dashboard
  Bybit perp:         http://${TARGET_BYBIT_PERP_WSS_BIND}/dashboard
  OKX spot:           http://${TARGET_OKX_SPOT_WSS_BIND}/dashboard
  OKX perp:           http://${TARGET_OKX_PERP_WSS_BIND}/dashboard
  Bitget spot:        http://${TARGET_BITGET_SPOT_WSS_BIND}/dashboard
  Bitget perp:        http://${TARGET_BITGET_PERP_WSS_BIND}/dashboard

实时日志:
  tail -f ${PREREQ_ROOT}/logs/arb-runtime-live-precheck.log
  tail -f ${RUN_ROOT}/logs/realtime-feedback.log

spot-perp-basis 常驻产物:
  ${RUN_ROOT}/resident-live/spot-perp-basis

cross-exchange-funding-arb 常驻产物:
  ${RUN_ROOT}/resident-live/cross-exchange-funding-arb

停止:
  ARB_RUNTIME_LIVE_ROOT=${RUN_ROOT} ARB_RUNTIME_LIVE_PREREQ_ROOT=${PREREQ_ROOT} scripts/stop-arb-runtime-live.sh
EOF
}

print_dashboards

BASIS_BINANCE_SPOT_WSS_BIND="${TARGET_BINANCE_SPOT_WSS_BIND}"
BASIS_BINANCE_PERP_WSS_BIND="${TARGET_BINANCE_PERP_WSS_BIND}"
BASIS_BYBIT_SPOT_WSS_BIND="${TARGET_BYBIT_SPOT_WSS_BIND}"
BASIS_BYBIT_PERP_WSS_BIND="${TARGET_BYBIT_PERP_WSS_BIND}"
BASIS_OKX_SPOT_WSS_BIND="${TARGET_OKX_SPOT_WSS_BIND}"
BASIS_OKX_PERP_WSS_BIND="${TARGET_OKX_PERP_WSS_BIND}"
BASIS_BITGET_SPOT_WSS_BIND="${TARGET_BITGET_SPOT_WSS_BIND}"
BASIS_BITGET_PERP_WSS_BIND="${TARGET_BITGET_PERP_WSS_BIND}"
if [[ "${TARGET_WSS_ENABLED}" != "1" ]]; then
  BASIS_BINANCE_SPOT_WSS_BIND="${BINANCE_SPOT_WSS_BIND}"
  BASIS_BINANCE_PERP_WSS_BIND="${BINANCE_PERP_WSS_BIND}"
  BASIS_BYBIT_SPOT_WSS_BIND="${BYBIT_SPOT_WSS_BIND}"
  BASIS_BYBIT_PERP_WSS_BIND="${BYBIT_PERP_WSS_BIND}"
  BASIS_OKX_SPOT_WSS_BIND="${OKX_SPOT_WSS_BIND}"
  BASIS_OKX_PERP_WSS_BIND="${OKX_PERP_WSS_BIND}"
  BASIS_BITGET_SPOT_WSS_BIND="${BITGET_SPOT_WSS_BIND}"
  BASIS_BITGET_PERP_WSS_BIND="${BITGET_PERP_WSS_BIND}"
fi

LIVE_ENV=(
  BASIS_OBSERVER_ROOT="${RUN_ROOT}"
  BASIS_OBSERVER_LIVE_ACK=1
  BASIS_OBSERVER_STRATEGIES="${LIVE_STRATEGIES}"
  BASIS_OBSERVER_SPOT_PERP_BASIS_MODE="${LIVE_SPOT_PERP_BASIS_MODE}"
  BASIS_OBSERVER_FUNDING_ARB_MODE="${LIVE_FUNDING_ARB_MODE}"
  BASIS_OBSERVER_FUNDING_ARB_RESIDENT_INTERVAL_SECS="${BASIS_OBSERVER_FUNDING_ARB_RESIDENT_INTERVAL_SECS:-60}"
  BASIS_OBSERVER_FUNDING_ARB_RESIDENT_MAX_LIVE_ENTRIES="${BASIS_OBSERVER_FUNDING_ARB_RESIDENT_MAX_LIVE_ENTRIES:-1}"
  BASIS_OBSERVER_FUNDING_ARB_RESIDENT_MAX_CYCLES="${LIVE_FUNDING_ARB_RESIDENT_MAX_CYCLES}"
  BASIS_OBSERVER_FUNDING_SETTLEMENT_LEDGER="${BASIS_OBSERVER_FUNDING_SETTLEMENT_LEDGER:-${FUNDING_SETTLEMENT_LEDGER:-}}"
  BASIS_OBSERVER_FUNDING_SETTLEMENT_RAW_SNAPSHOT="${BASIS_OBSERVER_FUNDING_SETTLEMENT_RAW_SNAPSHOT:-${FUNDING_SETTLEMENT_RAW_SNAPSHOT:-}}"
  BASIS_OBSERVER_PERP_TARGET_LEVERAGE="${BASIS_OBSERVER_PERP_TARGET_LEVERAGE}"
  ARB_RUNTIME_PERP_TARGET_LEVERAGE="${ARB_RUNTIME_PERP_TARGET_LEVERAGE}"
  BASIS_OBSERVER_BINANCE_USDM_LEVERAGE="${BASIS_OBSERVER_BINANCE_USDM_LEVERAGE}"
  BASIS_OBSERVER_BYBIT_LINEAR_LEVERAGE="${BASIS_OBSERVER_BYBIT_LINEAR_LEVERAGE}"
  BASIS_OBSERVER_OKX_SWAP_LEVERAGE="${BASIS_OBSERVER_OKX_SWAP_LEVERAGE}"
  BASIS_OBSERVER_BITGET_USDT_FUTURES_LEVERAGE="${BASIS_OBSERVER_BITGET_USDT_FUTURES_LEVERAGE}"
  BASIS_OBSERVER_ASTER_PERP_LEVERAGE="${BASIS_OBSERVER_ASTER_PERP_LEVERAGE}"
  BASIS_OBSERVER_HYPERLIQUID_PERP_LEVERAGE="${BASIS_OBSERVER_HYPERLIQUID_PERP_LEVERAGE}"
  BASIS_OBSERVER_DYNAMIC_TARGET_WSS=1
  BASIS_OBSERVER_TARGET_WSS_ROOT="${RUN_ROOT}/target-wss"
  BASIS_OBSERVER_TARGET_WSS_BASE_PORT="${BASIS_OBSERVER_TARGET_WSS_BASE_PORT:-8830}"
  BASIS_OBSERVER_TARGET_WSS_READY_TIMEOUT_SECS="${BASIS_OBSERVER_TARGET_WSS_READY_TIMEOUT_SECS:-60}"
  BASIS_OBSERVER_TARGET_WSS_RECONNECT_DELAY_SECS="${BASIS_OBSERVER_TARGET_WSS_RECONNECT_DELAY_SECS:-${WSS_RECONNECT_DELAY_SECS}}"
  BINANCE_SPOT_WSS_MONITOR_URL="http://${BASIS_BINANCE_SPOT_WSS_BIND}/api/binance-wss-book-ticker/status"
  BINANCE_PERP_WSS_MONITOR_URL="http://${BASIS_BINANCE_PERP_WSS_BIND}/api/binance-wss-book-ticker/status"
  BYBIT_SPOT_WSS_MONITOR_URL="http://${BASIS_BYBIT_SPOT_WSS_BIND}/api/bybit-wss-book-ticker/status"
  BYBIT_PERP_WSS_MONITOR_URL="http://${BASIS_BYBIT_PERP_WSS_BIND}/api/bybit-wss-book-ticker/status"
  OKX_SPOT_WSS_MONITOR_URL="http://${BASIS_OKX_SPOT_WSS_BIND}/api/okx-wss-book-ticker/status"
  OKX_PERP_WSS_MONITOR_URL="http://${BASIS_OKX_PERP_WSS_BIND}/api/okx-wss-book-ticker/status"
  BITGET_SPOT_WSS_MONITOR_URL="http://${BASIS_BITGET_SPOT_WSS_BIND}/api/bitget-wss-book-ticker/status"
  BITGET_PERP_WSS_MONITOR_URL="http://${BASIS_BITGET_PERP_WSS_BIND}/api/bitget-wss-book-ticker/status"
  ASTER_PERP_WSS_MONITOR_URL="http://${ASTER_PERP_WSS_BIND}/api/aster-wss-book-ticker/status"
  HYPERLIQUID_PERP_WSS_MONITOR_URL="http://${HYPERLIQUID_PERP_WSS_BIND}/api/hyperliquid-wss-book-ticker/status"
)

LIVE_ARGS=(
  live
  --config "${LIVE_CONFIG}"
  --out "${RUN_ROOT}"
  --interval-secs "${LIVE_INTERVAL_SECS}"
  --min-net-bps "${LIVE_MIN_NET_BPS}"
  --auto-once-cooldown-secs "${LIVE_AUTO_ONCE_COOLDOWN_SECS}"
  --i-understand-live-orders
)
if [[ "${LIVE_VALIDATE_AUTO_ONCE}" == "0" ]]; then
  LIVE_ARGS+=(--no-auto-validate)
fi

if [[ "${DETACH}" == "1" ]]; then
  live_log="${LOG_DIR}/arb-runtime-live.log"
  nohup env "${LIVE_ENV[@]}" "${RUNTIME_BIN}" "${LIVE_ARGS[@]}" >> "${live_log}" 2>&1 &
  live_pid="$!"
  printf '%s\n' "${live_pid}" > "${LIVE_PID_FILE}"
  echo
  echo "arb-runtime live started in background: pid=${live_pid} log=${live_log}"
  echo "实时 live 日志:"
  echo "  tail -f ${live_log}"
  exit 0
fi

echo
echo "starting arb-runtime live in foreground; press Ctrl-C to stop."
env "${LIVE_ENV[@]}" "${RUNTIME_BIN}" "${LIVE_ARGS[@]}"
