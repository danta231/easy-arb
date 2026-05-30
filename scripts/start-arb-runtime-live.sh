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
  2. 按策略自动解析需要订阅的 symbol 后，启动对应交易所 WSS monitor。
  3. 等待 WSS monitor 进入 streaming，且已收到真实 WSS 更新。
  4. 启动 arb-runtime live --i-understand-live-orders。
  5. arb-runtime live 默认只启动 cross-exchange-funding-arb 常驻 runner；
     spot-perp-basis 需要显式加入 ARB_RUNTIME_LIVE_STRATEGIES，避免无关策略限流拖垮 funding-arb。
  6. 打印所有实时 JSON API、日志和停止命令。

常用环境变量:
  ARB_RUNTIME_LIVE_ROOT=target/arb-runtime/live # 实盘主运行目录，保存 resident 状态、机会和报告。
  ARB_RUNTIME_LIVE_PREREQ_ROOT=target/arb-runtime/live-prereq # WSS 前置 monitor 的日志和 pid 状态目录。
  ARB_RUNTIME_LIVE_ENV_FILE=.env.local # 可选 env 文件路径；等价于命令行 --env-file。
  ARB_RUNTIME_LIVE_DETACH=0 # 是否后台运行 arb-runtime live；1 表示 detach。
  ARB_RUNTIME_LIVE_PRECHECK_LOG_ENABLED=1 # 是否把启动脚本自身输出写入 live-prereq/logs/arb-runtime-live-precheck.log。
  ARB_RUNTIME_LIVE_WSS_READY_TIMEOUT_SECS=120 # 等待 WSS monitor 就绪的最长秒数。
  ARB_RUNTIME_LIVE_RECLAIM_STALE_MONITOR_PORTS=1 # 启动前是否回收本仓库 arb-runtime 残留只读 API/WSS 端口；0 表示只报错。
  ARB_RUNTIME_LIVE_BUILD=1 # 启动前是否构建 arb-runtime 和 arb-wallet-signer。
  ARB_RUNTIME_LIVE_CONFIG=templates/personal_guarded_live.preflight.yaml # arb-runtime live 使用的风控和执行配置。
  ARB_RUNTIME_LIVE_INTERVAL_SECS=5 # observer 公开 monitor 轮询间隔秒数。
  ARB_RUNTIME_LIVE_MIN_NET_BPS=5 # 最小净收益阈值，单位 bps。
  ARB_RUNTIME_LOCAL_TZ=Asia/Shanghai # 面向人读的日志展示时区；默认 UTC+8。
  ARB_RUNTIME_LIVE_DERISK_ONLY=0 # 事故处理模式；1 表示只启动 funding arb resident 一轮恢复/减仓，不启动 spot-perp 实盘 resident。
  ARB_RUNTIME_LIVE_STRATEGIES=cross-exchange-funding-arb # live 默认只启用 funding-arb；需要 spot-perp 时显式改为 spot-perp-basis,cross-exchange-funding-arb。
  ARB_RUNTIME_LIVE_SPOT_PERP_BASIS_MODE=auto-once # spot-perp-basis 默认不跑常驻 resident；需要实盘常驻时显式设为 resident。
  ARB_RUNTIME_LIVE_CEX_WSS_SCOPE=auto # WSS 覆盖范围；auto 表示按策略解析 symbol，all 表示全部 USDT，target 表示只订阅 resident 实盘目标 symbol，custom 表示使用下面各交易所自定义值。
  ARB_RUNTIME_OKX_FUNDING_RATE_CACHE_TTL_SECS=60 # OKX funding-rate 全市场逐合约请求缓存秒数；0 表示每轮都重新请求。
  ARB_RUNTIME_BYBIT_LINEAR_INSTRUMENT_CACHE_TTL_SECS=300 # Bybit linear instruments-info 元数据缓存秒数；0 表示每轮都重新请求。
  ARB_RUNTIME_ASTER_SPOT_PERP_SPOT_SCAN_ENABLED=0 # Aster spot-perp 不可执行时默认跳过 spot/depth REST；1 表示恢复 spot 扫描。
  ARB_RUNTIME_HYPERLIQUID_SPOT_PERP_SPOT_SCAN_ENABLED=0 # Hyperliquid spot-perp 不可执行时默认跳过 spot context；1 表示恢复 spot 扫描。
  ARB_RUNTIME_FUNDING_ARB_DIRECT_PUBLIC_SOURCES_ENABLED=0 # funding-arb 直接读取 perp/funding 公开源；0 表示复用 basis monitor status。
  ARB_RUNTIME_LIVE_TARGET_WSS_ENABLED=auto # 是否额外启动实盘 guard 专用 target WSS；auto 表示 spot-perp resident 启用时启动并作为 readiness gate。
  ARB_RUNTIME_LIVE_KEEP_PREREQ_ON_LIVE_FAILURE=1 # foreground live 失败时是否保留只读 API/WSS 便于排查；停止用 scripts/stop-arb-runtime-live.sh。
  ARB_RUNTIME_LIVE_BINANCE_WSS_SYMBOL=BTCUSDT # CEX_WSS_SCOPE=custom 时的 Binance spot/perp WSS 订阅 symbol，多个用逗号分隔。
  ARB_RUNTIME_LIVE_BYBIT_WSS_SYMBOL=BTCUSDT # CEX_WSS_SCOPE=custom 时的 Bybit spot/perp WSS 订阅 symbol，多个用逗号分隔。
  ARB_RUNTIME_LIVE_OKX_WSS_SYMBOL=BTC-USDT # CEX_WSS_SCOPE=custom 时的 OKX spot/swap WSS 订阅 symbol，多个用逗号分隔。
  ARB_RUNTIME_LIVE_BITGET_WSS_SYMBOL=BTCUSDT # CEX_WSS_SCOPE=custom 时的 Bitget spot/perp WSS 订阅 symbol，多个用逗号分隔。
  ARB_RUNTIME_LIVE_ASTER_WSS_SYMBOL=ALL_USDT # all/custom/target 时的 Aster perp WSS monitor 订阅范围；auto 会覆盖为策略解析结果。
  ARB_RUNTIME_LIVE_HYPERLIQUID_WSS_SYMBOL=ALL_USDT # all/custom/target 时的 Hyperliquid perp WSS monitor 订阅范围；auto 会覆盖为策略解析结果。
  ARB_RUNTIME_LIVE_PORTFOLIO_BIND=127.0.0.1:8805 # portfolio JSON API 监听地址。
  ARB_RUNTIME_PORTFOLIO_PRIVATE_READONLY_ENABLED=1 # 是否启动全账户私有只读采集器；只读仓位和余额，不下单、不撤单、不转账。
  ARB_RUNTIME_PORTFOLIO_PRIVATE_READONLY_INTERVAL_SECS=60 # 全账户私有只读采集间隔秒数。
  ARB_RUNTIME_PORTFOLIO_PRIVATE_READONLY_VENUES= # 可选覆盖采集交易所，逗号分隔；留空表示所有支持 funding-arb 的交易所。
  BASIS_OBSERVER_BASIS_RESIDENT_INTERVAL_SECS=60 # spot-perp-basis 常驻 runner 扫描间隔秒数。
  BASIS_OBSERVER_BASIS_RESIDENT_MAX_CONCURRENT_POSITIONS=1 # spot-perp-basis 最多同时持有的未平仓 position 数。
  BASIS_OBSERVER_BASIS_RESIDENT_MAX_TOTAL_NOTIONAL_USDT=10.00 # spot-perp-basis 总名义本金上限，单位 USDT。
  BASIS_OBSERVER_PERP_TARGET_LEVERAGE=1 # 所有永续交易所默认目标杠杆；实盘非 reduce-only 下单前会先设置该杠杆。
  BASIS_OBSERVER_BINANCE_USDM_LEVERAGE=1 # 可选覆盖 Binance USD-M 永续目标杠杆。
  BASIS_OBSERVER_BYBIT_LINEAR_LEVERAGE=1 # 可选覆盖 Bybit linear 永续目标杠杆。
  BASIS_OBSERVER_OKX_SWAP_LEVERAGE=1 # 可选覆盖 OKX swap 永续目标杠杆。
  BASIS_OBSERVER_BITGET_USDT_FUTURES_LEVERAGE=1 # 可选覆盖 Bitget USDT-FUTURES 目标杠杆。
  BASIS_OBSERVER_ASTER_PERP_LEVERAGE=1 # 可选覆盖 Aster USDT perp 目标杠杆。
  BASIS_OBSERVER_HYPERLIQUID_PERP_LEVERAGE=1 # 可选覆盖 Hyperliquid perp 目标杠杆。
  BASIS_OBSERVER_FUNDING_ARB_RESIDENT_INTERVAL_SECS=60 # cross-exchange-funding-arb 常驻 runner 扫描间隔秒数。
  BASIS_OBSERVER_FUNDING_ARB_RESIDENT_MAX_CYCLES= # cross-exchange-funding-arb 最大循环次数；留空表示长期运行。
  BASIS_OBSERVER_FUNDING_ARB_AUTO_RESIDUAL_DE_RISK=1 # funding-arb 实盘发现历史 unknown/残腿仓位时自动进入 reduce-only 降风险恢复。
  BASIS_OBSERVER_FUNDING_SETTLEMENT_LEDGER= # 稳定结算账本输入路径；启用 raw snapshot 时必须留空。
  BASIS_OBSERVER_FUNDING_SETTLEMENT_RAW_SNAPSHOT= # 资金费率结算原始只读快照输出路径。
  ARB_RUNTIME_PORTFOLIO_ACCOUNT_SNAPSHOT=target/account_snapshot.json # portfolio JSON API 可选账户快照覆盖输入路径；留空时从 resident root 自动发现。
  ARB_RUNTIME_PORTFOLIO_POSITION_SNAPSHOT=target/position_snapshot.json # portfolio JSON API 可选仓位快照覆盖输入路径；留空时从 resident root 自动汇总。
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

listening_pids_for_port() {
  local port="$1"

  if command -v lsof >/dev/null 2>&1; then
    lsof -nP -iTCP:"${port}" -sTCP:LISTEN -t 2>/dev/null || true
    return 0
  fi
  if command -v ss >/dev/null 2>&1; then
    ss -H -ltnp "sport = :${port}" 2>/dev/null \
      | sed -n 's/.*pid=\([0-9][0-9]*\).*/\1/p' \
      | sort -u
    return 0
  fi
  if command -v fuser >/dev/null 2>&1; then
    fuser -n tcp "${port}" 2>/dev/null \
      | tr ' ' '\n' \
      | sed -n '/^[0-9][0-9]*$/p' \
      | sort -u
    return 0
  fi
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
  local exe_path
  local cmdline
  local open_files

  if [[ -e "/proc/${pid}/exe" ]]; then
    exe_path="$(readlink "/proc/${pid}/exe" 2>/dev/null || true)"
    if [[ -n "${RUNTIME_BIN:-}" && "${exe_path}" == "${RUNTIME_BIN}" ]] || [[ "${exe_path}" == "${REPO_ROOT}/target/"*"arb-runtime"* ]]; then
      return 0
    fi
  fi
  if [[ -r "/proc/${pid}/cmdline" ]]; then
    cmdline="$(tr '\0' ' ' < "/proc/${pid}/cmdline" 2>/dev/null || true)"
    if [[ -n "${RUNTIME_BIN:-}" && "${cmdline}" == *"${RUNTIME_BIN}"* ]] || [[ "${cmdline}" == *"${REPO_ROOT}/target/"*"arb-runtime"* ]]; then
      return 0
    fi
  fi
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
  done < <(listening_pids_for_port "${port}")
}

archive_pid_file_for_restart() {
  local pid_file="$1"
  [[ -e "${pid_file}" ]] || return 0
  mv "${pid_file}" "${pid_file}.stopped.$(date -u +%Y%m%dT%H%M%SZ)" 2>/dev/null || true
}

stop_recorded_single_process_for_restart() {
  local pid_file="$1"
  local label="$2"
  local pid

  [[ -s "${pid_file}" ]] || return 0
  pid="$(sed -n '1p' "${pid_file}" 2>/dev/null || true)"
  if is_alive "${pid}"; then
    [[ "${RECLAIM_STALE_MONITOR_PORTS:-1}" == "1" ]] || die "${label} already running: pid=${pid}; stop first with scripts/stop-arb-runtime-live.sh"
    echo "stopping previous read-only ${label}: pid=${pid}"
    kill -TERM "${pid}" 2>/dev/null || true
    if ! wait_for_pid_exit "${pid}" 5; then
      echo "previous read-only ${label} did not exit after TERM; sending KILL: pid=${pid}"
      kill -KILL "${pid}" 2>/dev/null || true
      wait_for_pid_exit "${pid}" 2 || true
    fi
  fi
  archive_pid_file_for_restart "${pid_file}"
}

stop_recorded_wss_monitors_for_restart() {
  local pid
  local name
  local _log
  local _url
  local _ready_symbol
  local _required
  local alive_count=0

  [[ -s "${WSS_PID_FILE}" ]] || return 0
  while IFS=$'\t' read -r pid name _log _url _ready_symbol _required; do
    if is_alive "${pid}"; then
      [[ "${RECLAIM_STALE_MONITOR_PORTS:-1}" == "1" ]] || die "WSS monitor already running: ${name} pid=${pid}; stop first with scripts/stop-arb-runtime-live.sh"
      alive_count="$((alive_count + 1))"
      echo "stopping previous read-only WSS monitor: name=${name} pid=${pid}"
      kill -TERM "${pid}" 2>/dev/null || true
    fi
  done < "${WSS_PID_FILE}"

  if (( alive_count > 0 )); then
    sleep 1
  fi

  while IFS=$'\t' read -r pid name _log _url _ready_symbol _required; do
    if is_alive "${pid}"; then
      echo "previous read-only WSS monitor did not exit after TERM; sending KILL: name=${name} pid=${pid}"
      kill -KILL "${pid}" 2>/dev/null || true
      wait_for_pid_exit "${pid}" 2 || true
    fi
  done < "${WSS_PID_FILE}"
  archive_pid_file_for_restart "${WSS_PID_FILE}"
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
KEEP_PREREQ_ON_LIVE_FAILURE="${ARB_RUNTIME_LIVE_KEEP_PREREQ_ON_LIVE_FAILURE:-1}"
case "${DETACH}" in
  0|1) ;;
  *) die "ARB_RUNTIME_LIVE_DETACH must be 0 or 1" ;;
esac
case "${BUILD}" in
  0|1) ;;
  *) die "ARB_RUNTIME_LIVE_BUILD must be 0 or 1" ;;
esac
case "${KEEP_PREREQ_ON_LIVE_FAILURE}" in
  0|1) ;;
  *) die "ARB_RUNTIME_LIVE_KEEP_PREREQ_ON_LIVE_FAILURE must be 0 or 1" ;;
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
LIVE_OBSERVER_PID_FILE="${RUN_ROOT}/state/basis-observer.pids"
PORTFOLIO_PID_FILE="${STATE_DIR}/portfolio-dashboard.pid"
PORTFOLIO_PRIVATE_READONLY_PID_FILE="${STATE_DIR}/portfolio-private-readonly.pid"
PRECHECK_LOG="${LOG_DIR}/arb-runtime-live-precheck.log"
RUNTIME_BIN="${ARB_RUNTIME_LIVE_BIN:-${REPO_ROOT}/target/debug/arb-runtime}"
WSS_READY_TIMEOUT_SECS="${ARB_RUNTIME_LIVE_WSS_READY_TIMEOUT_SECS:-120}"
WSS_RECONNECT_DELAY_SECS="${ARB_RUNTIME_LIVE_WSS_RECONNECT_DELAY_SECS:-2}"
RECLAIM_STALE_MONITOR_PORTS="${ARB_RUNTIME_LIVE_RECLAIM_STALE_MONITOR_PORTS:-1}"
PORTFOLIO_BIND="${ARB_RUNTIME_LIVE_PORTFOLIO_BIND:-127.0.0.1:8805}"
PORTFOLIO_PRIVATE_READONLY_ENABLED="${ARB_RUNTIME_PORTFOLIO_PRIVATE_READONLY_ENABLED:-1}"
PORTFOLIO_PRIVATE_READONLY_OUT="${ARB_RUNTIME_PORTFOLIO_PRIVATE_READONLY_OUT:-${RUN_ROOT}/portfolio-private-readonly}"
PORTFOLIO_PRIVATE_READONLY_INTERVAL_SECS="${ARB_RUNTIME_PORTFOLIO_PRIVATE_READONLY_INTERVAL_SECS:-60}"
PORTFOLIO_PRIVATE_READONLY_VENUES="${ARB_RUNTIME_PORTFOLIO_PRIVATE_READONLY_VENUES:-}"
LIVE_CONFIG="${ARB_RUNTIME_LIVE_CONFIG:-${BASIS_OBSERVER_CONFIG:-templates/personal_guarded_live.preflight.yaml}}"
LIVE_INTERVAL_SECS="${ARB_RUNTIME_LIVE_INTERVAL_SECS:-${BASIS_OBSERVER_INTERVAL_SECS:-5}}"
LIVE_MIN_NET_BPS="${ARB_RUNTIME_LIVE_MIN_NET_BPS:-${BASIS_OBSERVER_MIN_NET_BPS:-5}}"
LIVE_DERISK_ONLY="${ARB_RUNTIME_LIVE_DERISK_ONLY:-0}"
LIVE_STRATEGIES="${ARB_RUNTIME_LIVE_STRATEGIES:-cross-exchange-funding-arb}"
LIVE_SPOT_PERP_BASIS_MODE="${ARB_RUNTIME_LIVE_SPOT_PERP_BASIS_MODE:-auto-once}"
LIVE_FUNDING_ARB_MODE="resident"
LIVE_FUNDING_ARB_RESIDENT_MAX_CYCLES="${BASIS_OBSERVER_FUNDING_ARB_RESIDENT_MAX_CYCLES:-}"

if [[ -n "${BASIS_OBSERVER_STRATEGIES:-}" && -z "${ARB_RUNTIME_LIVE_STRATEGIES:-}" ]]; then
  echo "ignoring BASIS_OBSERVER_STRATEGIES for start-arb-runtime-live.sh default; set ARB_RUNTIME_LIVE_STRATEGIES to override live strategies"
fi
if [[ -n "${BASIS_OBSERVER_SPOT_PERP_BASIS_MODE:-}" && -z "${ARB_RUNTIME_LIVE_SPOT_PERP_BASIS_MODE:-}" ]]; then
  echo "ignoring BASIS_OBSERVER_SPOT_PERP_BASIS_MODE for start-arb-runtime-live.sh default; set ARB_RUNTIME_LIVE_SPOT_PERP_BASIS_MODE to override spot-perp live mode"
fi

case "${LIVE_DERISK_ONLY}" in
  0|1) ;;
  *) die "ARB_RUNTIME_LIVE_DERISK_ONLY must be 0 or 1" ;;
esac
case "${PORTFOLIO_PRIVATE_READONLY_ENABLED}" in
  0|1) ;;
  *) die "ARB_RUNTIME_PORTFOLIO_PRIVATE_READONLY_ENABLED must be 0 or 1" ;;
esac
if ! [[ "${PORTFOLIO_PRIVATE_READONLY_INTERVAL_SECS}" =~ ^[0-9]+$ ]] || (( PORTFOLIO_PRIVATE_READONLY_INTERVAL_SECS < 1 )); then
  die "ARB_RUNTIME_PORTFOLIO_PRIVATE_READONLY_INTERVAL_SECS must be a positive integer"
fi
if [[ "${LIVE_DERISK_ONLY}" == "1" ]]; then
  LIVE_STRATEGIES="cross-exchange-funding-arb"
  LIVE_SPOT_PERP_BASIS_MODE="auto-once"
  LIVE_FUNDING_ARB_MODE="resident"
  LIVE_FUNDING_ARB_RESIDENT_MAX_CYCLES="1"
fi

LIVE_STRATEGIES_COMPACT="${LIVE_STRATEGIES//[[:space:]]/}"
SPOT_PERP_RESIDENT_ENABLED=0
if [[ ",${LIVE_STRATEGIES_COMPACT}," == *",spot-perp-basis,"* && "${LIVE_SPOT_PERP_BASIS_MODE}" == "resident" ]]; then
  SPOT_PERP_RESIDENT_ENABLED=1
fi
FUNDING_ARB_DIRECT_PUBLIC_SOURCES_ENABLED="${ARB_RUNTIME_FUNDING_ARB_DIRECT_PUBLIC_SOURCES_ENABLED:-0}"
case "${FUNDING_ARB_DIRECT_PUBLIC_SOURCES_ENABLED}" in
  0|1) ;;
  *) die "ARB_RUNTIME_FUNDING_ARB_DIRECT_PUBLIC_SOURCES_ENABLED must be 0 or 1" ;;
esac

LIVE_MONITORS="${BASIS_OBSERVER_MONITORS:-binance bybit okx bitget}"
if [[ -z "${BASIS_OBSERVER_MONITORS:-}" ]]; then
  if [[ "${ARB_RUNTIME_ASTER_SPOT_PERP_SPOT_SCAN_ENABLED:-0}" == "1" || "${FUNDING_ARB_DIRECT_PUBLIC_SOURCES_ENABLED}" != "1" ]]; then
    LIVE_MONITORS="${LIVE_MONITORS} aster"
  fi
  if [[ "${ARB_RUNTIME_HYPERLIQUID_SPOT_PERP_SPOT_SCAN_ENABLED:-0}" == "1" || "${FUNDING_ARB_DIRECT_PUBLIC_SOURCES_ENABLED}" != "1" ]]; then
    LIVE_MONITORS="${LIVE_MONITORS} hyperliquid"
  fi
fi

if [[ -e "${LIVE_PAUSE_FILE}" && "${ARB_RUNTIME_LIVE_IGNORE_PAUSE:-0}" != "1" ]]; then
  die "live trading is paused by ${LIVE_PAUSE_FILE}; remove the file only after exchange-side risk is flat"
fi

BINANCE_SPOT_WSS_BIND="${ARB_RUNTIME_LIVE_BINANCE_SPOT_WSS_BIND:-127.0.0.1:8786}"
BINANCE_PERP_WSS_BIND="${ARB_RUNTIME_LIVE_BINANCE_PERP_WSS_BIND:-127.0.0.1:8806}"
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

CEX_WSS_SCOPE="${ARB_RUNTIME_LIVE_CEX_WSS_SCOPE:-auto}"
TARGET_WSS_ENABLED="${ARB_RUNTIME_LIVE_TARGET_WSS_ENABLED:-${SPOT_PERP_RESIDENT_ENABLED}}"
if [[ "${TARGET_WSS_ENABLED}" == "auto" ]]; then
  TARGET_WSS_ENABLED="${SPOT_PERP_RESIDENT_ENABLED}"
fi
SHARED_CEX_WSS_REQUIRED="${ARB_RUNTIME_LIVE_SHARED_CEX_WSS_REQUIRED:-${SPOT_PERP_RESIDENT_ENABLED}}"
case "${CEX_WSS_SCOPE}" in
  auto)
    BINANCE_WSS_SYMBOL=""
    BYBIT_WSS_SYMBOL=""
    OKX_WSS_SYMBOL=""
    BITGET_WSS_SYMBOL=""
    ;;
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
    die "ARB_RUNTIME_LIVE_CEX_WSS_SCOPE must be auto, target, all, or custom"
    ;;
esac
case "${TARGET_WSS_ENABLED}" in
  0|1) ;;
  *) die "ARB_RUNTIME_LIVE_TARGET_WSS_ENABLED must be 0, 1, or auto" ;;
esac
case "${SHARED_CEX_WSS_REQUIRED}" in
  0|1) ;;
  *) die "ARB_RUNTIME_LIVE_SHARED_CEX_WSS_REQUIRED must be 0 or 1" ;;
esac
ASTER_WSS_SYMBOL="${ARB_RUNTIME_LIVE_ASTER_WSS_SYMBOL:-ALL_USDT}"
HYPERLIQUID_WSS_SYMBOL="${ARB_RUNTIME_LIVE_HYPERLIQUID_WSS_SYMBOL:-ALL_USDT}"
BINANCE_SPOT_WSS_SYMBOL="${BINANCE_WSS_SYMBOL}"
BINANCE_PERP_WSS_SYMBOL="${BINANCE_WSS_SYMBOL}"
BYBIT_SPOT_WSS_SYMBOL="${BYBIT_WSS_SYMBOL}"
BYBIT_PERP_WSS_SYMBOL="${BYBIT_WSS_SYMBOL}"
OKX_SPOT_WSS_SYMBOL="${OKX_WSS_SYMBOL}"
OKX_PERP_WSS_SYMBOL="${OKX_WSS_SYMBOL}"
BITGET_SPOT_WSS_SYMBOL="${BITGET_WSS_SYMBOL}"
BITGET_PERP_WSS_SYMBOL="${BITGET_WSS_SYMBOL}"

mkdir -p "${LOG_DIR}" "${STATE_DIR}"

if [[ "${ARB_RUNTIME_LIVE_PRECHECK_LOG_ENABLED:-1}" != "0" ]]; then
  {
    echo
    echo "=== arb-runtime live precheck $(local_log_timestamp) ==="
    echo "repo_root=${REPO_ROOT}"
    echo "run_root=${RUN_ROOT}"
    echo "prereq_root=${PREREQ_ROOT}"
    echo "detach=${DETACH} build=${BUILD} config=${LIVE_CONFIG} interval_secs=${LIVE_INTERVAL_SECS} min_net_bps=${LIVE_MIN_NET_BPS} strategies=${LIVE_STRATEGIES} monitors=${LIVE_MONITORS} spot_perp_basis_mode=${LIVE_SPOT_PERP_BASIS_MODE} funding_arb_mode=${LIVE_FUNDING_ARB_MODE} funding_arb_direct_public_sources=${FUNDING_ARB_DIRECT_PUBLIC_SOURCES_ENABLED} derisk_only=${LIVE_DERISK_ONLY} cex_wss_scope=${CEX_WSS_SCOPE} shared_cex_wss_required=${SHARED_CEX_WSS_REQUIRED} target_wss_enabled=${TARGET_WSS_ENABLED}"
  } >> "${PRECHECK_LOG}"
  exec > >(tee -a "${PRECHECK_LOG}") 2>&1
fi

if [[ -s "${LIVE_PID_FILE}" ]]; then
  live_pid="$(sed -n '1p' "${LIVE_PID_FILE}" 2>/dev/null || true)"
  if is_alive "${live_pid}"; then
    die "arb-runtime live already running: pid=${live_pid}; stop first with scripts/stop-arb-runtime-live.sh"
  fi
fi
if [[ -s "${LIVE_OBSERVER_PID_FILE}" ]]; then
  while IFS=$'\t' read -r observer_pid observer_name _observer_log; do
    if is_alive "${observer_pid}"; then
      die "arb-runtime live observer already running: ${observer_name} pid=${observer_pid}; stop first with scripts/stop-arb-runtime-live.sh"
    fi
  done < "${LIVE_OBSERVER_PID_FILE}"
fi

stop_recorded_wss_monitors_for_restart
: > "${WSS_PID_FILE}"

stop_recorded_single_process_for_restart "${PORTFOLIO_PRIVATE_READONLY_PID_FILE}" "portfolio-private-readonly"
stop_recorded_single_process_for_restart "${PORTFOLIO_PID_FILE}" "portfolio-dashboard"

if [[ "${BUILD}" == "1" ]]; then
  echo "building arb-runtime with live-exec feature..."
  cargo build -p arb-runtime --features live-exec --manifest-path "${REPO_ROOT}/Cargo.toml"
  echo "building arb-wallet-signer..."
  cargo build -p arb-wallet-signer --manifest-path "${REPO_ROOT}/Cargo.toml"
fi

resolve_auto_wss_symbols() {
  local resolver_output
  local resolver_env_file="${STATE_DIR}/live-wss-symbols.env"

  echo "resolving live WSS symbol scopes from public venue metadata..."
  if ! resolver_output="$(
    env ARB_RUNTIME_ENABLE_LEGACY_COMMANDS=1 "${RUNTIME_BIN}" resolve-live-wss-symbols \
      --strategies "${LIVE_STRATEGIES}" \
      --monitors "${LIVE_MONITORS}" \
      --format shell
  )"; then
    printf '%s\n' "${resolver_output}"
    die "failed to resolve live WSS symbol scopes"
  fi
  printf '%s\n' "${resolver_output}" > "${resolver_env_file}"
  # shellcheck disable=SC1090
  source "${resolver_env_file}"
  echo "wss_symbol_resolver mode=auto env=${resolver_env_file} spot_perp_symbols=${LIVE_WSS_RESOLVER_SPOT_PERP_SYMBOL_COUNT:-0} funding_bases=${LIVE_WSS_RESOLVER_FUNDING_ARB_BASE_COUNT:-0}"
  echo "wss_symbols binance_spot=${BINANCE_SPOT_WSS_SYMBOL:-skip} binance_perp=${BINANCE_PERP_WSS_SYMBOL:-skip} bybit_spot=${BYBIT_SPOT_WSS_SYMBOL:-skip} bybit_perp=${BYBIT_PERP_WSS_SYMBOL:-skip} okx_spot=${OKX_SPOT_WSS_SYMBOL:-skip} okx_perp=${OKX_PERP_WSS_SYMBOL:-skip} bitget_spot=${BITGET_SPOT_WSS_SYMBOL:-skip} bitget_perp=${BITGET_PERP_WSS_SYMBOL:-skip} aster_perp=${ASTER_WSS_SYMBOL:-skip} hyperliquid_perp=${HYPERLIQUID_WSS_SYMBOL:-skip}"
}

if [[ "${CEX_WSS_SCOPE}" == "auto" ]]; then
  resolve_auto_wss_symbols
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
  echo "starting ${name}: ${status_url}"
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

start_wss_monitor_if_configured() {
  local name="$1"
  local command_name="$2"
  local bind_addr="$3"
  local symbol="$4"
  local market="$5"
  local api_prefix="$6"
  local ready_symbol="${7:-}"
  local required="${8:-1}"

  if [[ -z "${symbol}" ]]; then
    echo "skipping ${name}: no symbols resolved for current live strategies"
    return 0
  fi
  start_wss_monitor "${name}" "${command_name}" "${bind_addr}" "${symbol}" "${market}" "${api_prefix}" "${ready_symbol}" "${required}"
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
  echo "starting portfolio JSON API: http://${PORTFOLIO_BIND}/api/portfolio/status"
  echo "starting error logs JSON API: http://${PORTFOLIO_BIND}/api/errors/logs"
  nohup "${args[@]}" >> "${log_file}" 2>&1 &
  pid="$!"
  printf '%s\n' "${pid}" > "${PORTFOLIO_PID_FILE}"
  echo "  pid=${pid} log=${log_file}"
  sleep 1
  if ! is_alive "${pid}"; then
    echo "portfolio JSON API failed to stay running; last log lines:"
    tail -n 40 "${log_file}" 2>/dev/null || true
    die "portfolio JSON API failed to start"
  fi
}

start_portfolio_private_readonly_snapshot() {
  [[ "${PORTFOLIO_PRIVATE_READONLY_ENABLED}" == "1" ]] || return 0
  local log_file="${LOG_DIR}/portfolio-private-readonly.log"
  local pid
  local -a args
  local venue
  local -a venues

  args=(
    "${RUNTIME_BIN}" portfolio-private-readonly-snapshot
    --config "${LIVE_CONFIG}"
    --out "${PORTFOLIO_PRIVATE_READONLY_OUT}"
    --interval-secs "${PORTFOLIO_PRIVATE_READONLY_INTERVAL_SECS}"
  )
  if [[ -n "${PORTFOLIO_PRIVATE_READONLY_VENUES}" ]]; then
    args+=(--clear-venues)
    IFS=',' read -r -a venues <<< "${PORTFOLIO_PRIVATE_READONLY_VENUES}"
    for venue in "${venues[@]}"; do
      venue="${venue//[[:space:]]/}"
      [[ -n "${venue}" ]] || continue
      args+=(--venue "${venue}")
    done
  fi
  [[ -n "${HYPERLIQUID_USER:-}" ]] && args+=(--hyperliquid-user "${HYPERLIQUID_USER}")
  [[ -n "${ASTER_USER:-}" ]] && args+=(--aster-user "${ASTER_USER}")
  [[ -n "${ASTER_SIGNER:-}" ]] && args+=(--aster-signer "${ASTER_SIGNER}")
  [[ -n "${ASTER_SIGNER_CMD_ENV:-}" ]] && args+=(--aster-signer-cmd-env "${ASTER_SIGNER_CMD_ENV}")

  mkdir -p "${PORTFOLIO_PRIVATE_READONLY_OUT}"
  echo "starting portfolio private read-only snapshotter: out=${PORTFOLIO_PRIVATE_READONLY_OUT}"
  nohup env ARB_RUNTIME_ENABLE_LEGACY_COMMANDS=1 "${args[@]}" >> "${log_file}" 2>&1 &
  pid="$!"
  printf '%s\n' "${pid}" > "${PORTFOLIO_PRIVATE_READONLY_PID_FILE}"
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

stop_portfolio_private_readonly_snapshot() {
  [[ -s "${PORTFOLIO_PRIVATE_READONLY_PID_FILE}" ]] || return 0
  local pid
  pid="$(sed -n '1p' "${PORTFOLIO_PRIVATE_READONLY_PID_FILE}")"
  if is_alive "${pid}"; then
    echo "TERM portfolio-private-readonly pid=${pid}"
    kill -TERM "${pid}" 2>/dev/null || true
  fi
}

cleanup_prereq_processes() {
  stop_portfolio_dashboard
  stop_portfolio_private_readonly_snapshot
  stop_wss_monitors
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
  local reason="stopped by start-arb-runtime-live.sh"
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
  local state_path="$3"
  local events_path="$4"
  local process_name="$5"
  local label="$6"
  local archived_lock

  [[ -e "${lock_path}" ]] || return 0
  if resident_process_alive "${process_name}"; then
    echo "resident_lock_kept label=${label} reason=process_alive lock=${lock_path}"
    return 0
  fi
  archived_lock="${lock_path}.stale.$(date -u +%Y%m%dT%H%M%SZ).$$"
  if safe_resident_lock_artifacts "${summary_path}" "${events_path}"; then
    mark_running_resident_artifacts_stopped "${state_path}" "${summary_path}" "${label}"
    echo "resident_lock_archived label=${label} reason=stale_safe_state lock=${lock_path} archived=${archived_lock}"
    mv "${lock_path}" "${archived_lock}"
    return 0
  fi
  mark_running_resident_artifacts_stopped "${state_path}" "${summary_path}" "${label}"
  echo "resident_lock_archived label=${label} reason=stale_state_requires_resident_recovery lock=${lock_path} archived=${archived_lock}"
  mv "${lock_path}" "${archived_lock}"
}

cleanup_safe_stale_resident_locks() {
  cleanup_safe_stale_resident_lock \
    "${RUN_ROOT}/resident-live/spot-perp-basis/multi_venue_resident_live.lock" \
    "${RUN_ROOT}/resident-live/spot-perp-basis/multi_venue_resident_live_summary.json" \
    "${RUN_ROOT}/resident-live/spot-perp-basis/multi_venue_resident_live_state.json" \
    "${RUN_ROOT}/resident-live/spot-perp-basis/multi_venue_resident_live_events.jsonl" \
    "spot-perp-basis-resident-live" \
    "spot-perp-basis"
  cleanup_safe_stale_resident_lock \
    "${RUN_ROOT}/resident-live/cross-exchange-funding-arb/funding_arb_resident_live.lock" \
    "${RUN_ROOT}/resident-live/cross-exchange-funding-arb/funding_arb_resident_live_summary.json" \
    "${RUN_ROOT}/resident-live/cross-exchange-funding-arb/funding_arb_resident_live_state.json" \
    "${RUN_ROOT}/resident-live/cross-exchange-funding-arb/funding_arb_resident_live_events.jsonl" \
    "funding-arb-resident-live" \
    "cross-exchange-funding-arb"
}

cleanup_safe_stale_resident_locks

if [[ "${DETACH}" != "1" ]]; then
  trap 'cleanup_prereq_processes' EXIT
  trap 'cleanup_prereq_processes; exit 130' INT
  trap 'cleanup_prereq_processes; exit 143' TERM
fi

start_portfolio_private_readonly_snapshot
start_portfolio_dashboard

start_wss_monitor_if_configured binance-spot binance-wss-book-ticker "${BINANCE_SPOT_WSS_BIND}" "${BINANCE_SPOT_WSS_SYMBOL}" spot /api/binance-wss-book-ticker "" "${SHARED_CEX_WSS_REQUIRED}"
start_wss_monitor_if_configured binance-perp binance-wss-book-ticker "${BINANCE_PERP_WSS_BIND}" "${BINANCE_PERP_WSS_SYMBOL}" usdm-perp /api/binance-wss-book-ticker "" "${SHARED_CEX_WSS_REQUIRED}"
start_wss_monitor_if_configured bybit-spot bybit-wss-book-ticker "${BYBIT_SPOT_WSS_BIND}" "${BYBIT_SPOT_WSS_SYMBOL}" spot /api/bybit-wss-book-ticker "" "${SHARED_CEX_WSS_REQUIRED}"
start_wss_monitor_if_configured bybit-perp bybit-wss-book-ticker "${BYBIT_PERP_WSS_BIND}" "${BYBIT_PERP_WSS_SYMBOL}" linear-perp /api/bybit-wss-book-ticker "" "${SHARED_CEX_WSS_REQUIRED}"
start_wss_monitor_if_configured okx-spot okx-wss-book-ticker "${OKX_SPOT_WSS_BIND}" "${OKX_SPOT_WSS_SYMBOL}" spot /api/okx-wss-book-ticker "" "${SHARED_CEX_WSS_REQUIRED}"
start_wss_monitor_if_configured okx-perp okx-wss-book-ticker "${OKX_PERP_WSS_BIND}" "${OKX_PERP_WSS_SYMBOL}" swap /api/okx-wss-book-ticker "" "${SHARED_CEX_WSS_REQUIRED}"
start_wss_monitor_if_configured bitget-spot bitget-wss-book-ticker "${BITGET_SPOT_WSS_BIND}" "${BITGET_SPOT_WSS_SYMBOL}" spot /api/bitget-wss-book-ticker "" "${SHARED_CEX_WSS_REQUIRED}"
start_wss_monitor_if_configured bitget-perp bitget-wss-book-ticker "${BITGET_PERP_WSS_BIND}" "${BITGET_PERP_WSS_SYMBOL}" usdt-futures /api/bitget-wss-book-ticker "" "${SHARED_CEX_WSS_REQUIRED}"
start_wss_monitor_if_configured aster-perp aster-wss-book-ticker "${ASTER_PERP_WSS_BIND}" "${ASTER_WSS_SYMBOL}" usdt-futures /api/aster-wss-book-ticker "" 0
start_wss_monitor_if_configured hyperliquid-perp hyperliquid-wss-book-ticker "${HYPERLIQUID_PERP_WSS_BIND}" "${HYPERLIQUID_WSS_SYMBOL}" perp /api/hyperliquid-wss-book-ticker "" 0

if [[ "${TARGET_WSS_ENABLED}" == "1" ]]; then
  start_wss_monitor target-binance-spot binance-wss-book-ticker "${TARGET_BINANCE_SPOT_WSS_BIND}" "${BINANCE_BASIS_READY_SYMBOL}" spot /api/binance-wss-book-ticker "${BINANCE_BASIS_READY_SYMBOL}" 1
  start_wss_monitor target-binance-perp binance-wss-book-ticker "${TARGET_BINANCE_PERP_WSS_BIND}" "${BINANCE_BASIS_READY_SYMBOL}" usdm-perp /api/binance-wss-book-ticker "${BINANCE_BASIS_READY_SYMBOL}" 1
  start_wss_monitor target-bybit-spot bybit-wss-book-ticker "${TARGET_BYBIT_SPOT_WSS_BIND}" "${BYBIT_BASIS_READY_SYMBOL}" spot /api/bybit-wss-book-ticker "${BYBIT_BASIS_READY_SYMBOL}" 1
  start_wss_monitor target-bybit-perp bybit-wss-book-ticker "${TARGET_BYBIT_PERP_WSS_BIND}" "${BYBIT_BASIS_READY_SYMBOL}" linear-perp /api/bybit-wss-book-ticker "${BYBIT_BASIS_READY_SYMBOL}" 1
  start_wss_monitor target-okx-spot okx-wss-book-ticker "${TARGET_OKX_SPOT_WSS_BIND}" "${OKX_BASIS_READY_SYMBOL}" spot /api/okx-wss-book-ticker "${OKX_BASIS_READY_SYMBOL}" 1
  start_wss_monitor target-okx-perp okx-wss-book-ticker "${TARGET_OKX_PERP_WSS_BIND}" "${OKX_BASIS_READY_SYMBOL}" swap /api/okx-wss-book-ticker "${OKX_BASIS_READY_SYMBOL}" 1
  start_wss_monitor target-bitget-spot bitget-wss-book-ticker "${TARGET_BITGET_SPOT_WSS_BIND}" "${BITGET_BASIS_READY_SYMBOL}" spot /api/bitget-wss-book-ticker "${BITGET_BASIS_READY_SYMBOL}" 1
  start_wss_monitor target-bitget-perp bitget-wss-book-ticker "${TARGET_BITGET_PERP_WSS_BIND}" "${BITGET_BASIS_READY_SYMBOL}" usdt-futures /api/bitget-wss-book-ticker "${BITGET_BASIS_READY_SYMBOL}" 1
fi

while IFS=$'\t' read -r pid name log_file status_url ready_symbol required; do
  [[ "${ready_symbol:-}" == "__none__" ]] && ready_symbol=""
  wait_for_wss_monitor "${name}" "${pid}" "${log_file}" "${status_url}" "${ready_symbol:-}" "${required:-1}"
done < "${WSS_PID_FILE}"

print_readonly_apis() {
  cat <<EOF

正式实盘只读 JSON API:
  系统导航:          http://${PORTFOLIO_BIND}/api/navigation/pages
  总组合状态:        http://${PORTFOLIO_BIND}/api/portfolio/status
  错误日志:          http://${PORTFOLIO_BIND}/api/errors/logs
  Binance basis:      http://127.0.0.1:8796/api/basis/status
  Bybit basis:        http://127.0.0.1:8797/api/bybit-basis/status
  OKX basis:          http://127.0.0.1:8798/api/okx-basis/status
  Bitget basis:       http://127.0.0.1:8803/api/bitget-basis/status
  Aster basis:        http://127.0.0.1:8800/api/aster-basis/status
  Hyperliquid basis:  http://127.0.0.1:8799/api/hyperliquid-basis/status
  Funding arb:        http://127.0.0.1:8804/api/funding-arb/status

策略 WSS 覆盖 JSON API（auto 模式下未解析到 symbol 的 spot/perp monitor 不会启动）:
  Binance spot:       http://${BINANCE_SPOT_WSS_BIND}/api/binance-wss-book-ticker/status
  Binance perp:       http://${BINANCE_PERP_WSS_BIND}/api/binance-wss-book-ticker/status
  Bybit spot:         http://${BYBIT_SPOT_WSS_BIND}/api/bybit-wss-book-ticker/status
  Bybit perp:         http://${BYBIT_PERP_WSS_BIND}/api/bybit-wss-book-ticker/status
  OKX spot:           http://${OKX_SPOT_WSS_BIND}/api/okx-wss-book-ticker/status
  OKX perp:           http://${OKX_PERP_WSS_BIND}/api/okx-wss-book-ticker/status
  Bitget spot:        http://${BITGET_SPOT_WSS_BIND}/api/bitget-wss-book-ticker/status
  Bitget perp:        http://${BITGET_PERP_WSS_BIND}/api/bitget-wss-book-ticker/status
  Aster perp:         http://${ASTER_PERP_WSS_BIND}/api/aster-wss-book-ticker/status
  Hyperliquid perp:   http://${HYPERLIQUID_PERP_WSS_BIND}/api/hyperliquid-wss-book-ticker/status

实盘 guard target WSS JSON API:
  Binance spot:       http://${TARGET_BINANCE_SPOT_WSS_BIND}/api/binance-wss-book-ticker/status
  Binance perp:       http://${TARGET_BINANCE_PERP_WSS_BIND}/api/binance-wss-book-ticker/status
  Bybit spot:         http://${TARGET_BYBIT_SPOT_WSS_BIND}/api/bybit-wss-book-ticker/status
  Bybit perp:         http://${TARGET_BYBIT_PERP_WSS_BIND}/api/bybit-wss-book-ticker/status
  OKX spot:           http://${TARGET_OKX_SPOT_WSS_BIND}/api/okx-wss-book-ticker/status
  OKX perp:           http://${TARGET_OKX_PERP_WSS_BIND}/api/okx-wss-book-ticker/status
  Bitget spot:        http://${TARGET_BITGET_SPOT_WSS_BIND}/api/bitget-wss-book-ticker/status
  Bitget perp:        http://${TARGET_BITGET_PERP_WSS_BIND}/api/bitget-wss-book-ticker/status

实时日志:
  tail -f ${PREREQ_ROOT}/logs/arb-runtime-live-precheck.log
  tail -f ${PREREQ_ROOT}/logs/portfolio-private-readonly.log
  tail -f ${RUN_ROOT}/logs/realtime-feedback.log

全账户私有只读快照:
  ${PORTFOLIO_PRIVATE_READONLY_OUT}

spot-perp-basis 常驻产物（仅显式启用 resident 时）:
  ${RUN_ROOT}/resident-live/spot-perp-basis

cross-exchange-funding-arb 常驻产物:
  ${RUN_ROOT}/resident-live/cross-exchange-funding-arb

停止:
  ARB_RUNTIME_LIVE_ROOT=${RUN_ROOT} ARB_RUNTIME_LIVE_PREREQ_ROOT=${PREREQ_ROOT} scripts/stop-arb-runtime-live.sh
EOF
}

print_readonly_apis

BASIS_BINANCE_SPOT_WSS_BIND="${BINANCE_SPOT_WSS_BIND}"
BASIS_BINANCE_PERP_WSS_BIND="${BINANCE_PERP_WSS_BIND}"
BASIS_BYBIT_SPOT_WSS_BIND="${BYBIT_SPOT_WSS_BIND}"
BASIS_BYBIT_PERP_WSS_BIND="${BYBIT_PERP_WSS_BIND}"
BASIS_OKX_SPOT_WSS_BIND="${OKX_SPOT_WSS_BIND}"
BASIS_OKX_PERP_WSS_BIND="${OKX_PERP_WSS_BIND}"
BASIS_BITGET_SPOT_WSS_BIND="${BITGET_SPOT_WSS_BIND}"
BASIS_BITGET_PERP_WSS_BIND="${BITGET_PERP_WSS_BIND}"

LIVE_ENV=(
  BASIS_OBSERVER_ROOT="${RUN_ROOT}"
  BASIS_OBSERVER_LIVE_ACK=1
  BASIS_OBSERVER_STRATEGIES="${LIVE_STRATEGIES}"
  BASIS_OBSERVER_MONITORS="${LIVE_MONITORS}"
  BASIS_OBSERVER_SPOT_PERP_BASIS_MODE="${LIVE_SPOT_PERP_BASIS_MODE}"
  BASIS_OBSERVER_FUNDING_ARB_MODE="${LIVE_FUNDING_ARB_MODE}"
  BASIS_OBSERVER_FUNDING_ARB_RESIDENT_INTERVAL_SECS="${BASIS_OBSERVER_FUNDING_ARB_RESIDENT_INTERVAL_SECS:-60}"
  BASIS_OBSERVER_FUNDING_ARB_RESIDENT_MAX_CYCLES="${LIVE_FUNDING_ARB_RESIDENT_MAX_CYCLES}"
  BASIS_OBSERVER_FUNDING_ARB_AUTO_RESIDUAL_DE_RISK="${BASIS_OBSERVER_FUNDING_ARB_AUTO_RESIDUAL_DE_RISK:-1}"
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
  BASIS_OBSERVER_DYNAMIC_TARGET_WSS=0
  BASIS_OBSERVER_TARGET_WSS_ROOT="${RUN_ROOT}/target-wss"
  BASIS_OBSERVER_TARGET_WSS_BASE_PORT="${BASIS_OBSERVER_TARGET_WSS_BASE_PORT:-8830}"
  BASIS_OBSERVER_TARGET_WSS_READY_TIMEOUT_SECS="${BASIS_OBSERVER_TARGET_WSS_READY_TIMEOUT_SECS:-60}"
  BASIS_OBSERVER_TARGET_WSS_RECONNECT_DELAY_SECS="${BASIS_OBSERVER_TARGET_WSS_RECONNECT_DELAY_SECS:-${WSS_RECONNECT_DELAY_SECS}}"
  ARB_RUNTIME_FUNDING_ARB_DIRECT_PUBLIC_SOURCES_ENABLED="${FUNDING_ARB_DIRECT_PUBLIC_SOURCES_ENABLED}"
  ARB_RUNTIME_ASTER_SPOT_PERP_SPOT_SCAN_ENABLED="${ARB_RUNTIME_ASTER_SPOT_PERP_SPOT_SCAN_ENABLED:-0}"
  ARB_RUNTIME_HYPERLIQUID_SPOT_PERP_SPOT_SCAN_ENABLED="${ARB_RUNTIME_HYPERLIQUID_SPOT_PERP_SPOT_SCAN_ENABLED:-0}"
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
  --i-understand-live-orders
)

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
set +e
env "${LIVE_ENV[@]}" "${RUNTIME_BIN}" "${LIVE_ARGS[@]}"
live_status="$?"
set -e
if [[ "${live_status}" != "0" && "${KEEP_PREREQ_ON_LIVE_FAILURE}" == "1" ]]; then
  trap - EXIT INT TERM
  echo
  echo "arb-runtime live exited with status=${live_status}; keeping read-only API/WSS processes running for inspection."
  echo "stop them with:"
  echo "  ARB_RUNTIME_LIVE_ROOT=${RUN_ROOT} ARB_RUNTIME_LIVE_PREREQ_ROOT=${PREREQ_ROOT} scripts/stop-arb-runtime-live.sh"
  exit "${live_status}"
fi
exit "${live_status}"
