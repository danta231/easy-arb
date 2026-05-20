#!/usr/bin/env bash
set -euo pipefail

# 中文说明：启动六交易所套利机会观察链路。
# 默认运行测试盘：公开行情监控 + 模拟下单验证，不提交订单、不撤单、不转账。
# 只有 BASIS_OBSERVER_EXECUTE_LIVE=1 且 BASIS_OBSERVER_LIVE_ACK=1 时才进入正式实盘，
# 并向底层 guarded live 命令传递真实下单确认参数。
# 当前会主动轮询 spot-perp-basis monitor，并默认启动 spot-perp-basis 常驻
# live runner；同时启动专用 funding-arb-monitor 聚合本机 basis status 快照，
# 生成 cross-exchange-funding-arb 机会文件。

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
export ARB_RUNTIME_ENABLE_LEGACY_COMMANDS="${ARB_RUNTIME_ENABLE_LEGACY_COMMANDS:-1}"

usage() {
  cat <<'USAGE'
用法:
  scripts/start-basis-opportunity-observer.sh [binance] [bybit] [okx] [bitget] [aster] [hyperliquid]
  scripts/start-basis-opportunity-observer.sh --venues binance,bybit,okx,bitget,aster,hyperliquid --strategies spot-perp-basis,cross-exchange-funding-arb

默认启动 binance、bybit、okx、bitget、aster、hyperliquid 六条公开行情链路。

核心行为:
  1. 启动六条只读 basis monitor，持续刷新公开行情。
  2. 轮询 /opportunities，实时记录 candidate_count > 0 的 spot-perp-basis 机会。
  3. 默认启动 multi-venue-basis-resident-live 常驻 runner 管理
     Binance/Bybit/OKX/Bitget 的 spot-perp-basis 开仓和平仓。
  4. 如果启用 cross-exchange-funding-arb，启动专用 funding-arb-monitor，
     聚合本机 basis /status 快照并记录真实候选，不伪造机会。
  5. 测试盘默认模拟下单；正式实盘必须显式设置 BASIS_OBSERVER_EXECUTE_LIVE=1 和 BASIS_OBSERVER_LIVE_ACK=1。

常用环境变量:
  BASIS_OBSERVER_ROOT=target/arb-opportunity-observer # observer 主运行目录，保存日志、快照、机会和报告。
  BASIS_OBSERVER_STRATEGIES=spot-perp-basis,cross-exchange-funding-arb # 启用策略列表。
  BASIS_OBSERVER_MONITORS="binance bybit okx bitget aster hyperliquid" # 启用的交易所 monitor 列表。
  BASIS_OBSERVER_INTERVAL_SECS=5 # observer 轮询公开 monitor 的间隔秒数。
  BASIS_OBSERVER_MIN_NET_BPS=5 # 最小净收益阈值，单位 bps。
  BASIS_OBSERVER_MIN_ABS_FUNDING_RATE=0 # 最小绝对资金费率过滤阈值；0 表示不过滤。
  BASIS_OBSERVER_NOTIONAL_USD=100.00 # 单次候选机会用于计算和下单的目标名义本金，单位美元。
  BASIS_OBSERVER_SPOT_FEE_BPS=10 # spot 腿手续费估算，单位 bps。
  BASIS_OBSERVER_PERP_FEE_BPS=5 # perp 腿手续费估算，单位 bps。
  BASIS_OBSERVER_SLIPPAGE_BPS=5 # 滑点估算，单位 bps。
  BASIS_OBSERVER_CONFIG=templates/personal_guarded_live.preflight.yaml # 风控和执行配置文件路径。
  BASIS_OBSERVER_SPOT_PERP_BASIS_MODE=resident # spot-perp-basis 运行模式；resident 表示常驻运行。
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
  BASIS_OBSERVER_BASIS_RESIDENT_MAX_CYCLES= # spot-perp-basis 最大循环次数；留空表示长期运行。
  BASIS_OBSERVER_FUNDING_ARB_MODE=resident # cross-exchange-funding-arb 运行模式；resident 表示常驻运行。
  BASIS_OBSERVER_FUNDING_ARB_RESIDENT_INTERVAL_SECS=60 # cross-exchange-funding-arb 常驻 runner 扫描间隔秒数。
  BASIS_OBSERVER_FUNDING_ARB_RESIDENT_MAX_LIVE_ENTRIES=1 # cross-exchange-funding-arb 单轮最多新开实盘 entry 数。
  BASIS_OBSERVER_FUNDING_ARB_RESIDENT_MAX_CYCLES= # cross-exchange-funding-arb 最大循环次数；留空表示长期运行。
  BASIS_OBSERVER_FUNDING_SETTLEMENT_LEDGER= # 稳定结算账本输入路径；启用 raw snapshot 时必须留空。
  BASIS_OBSERVER_FUNDING_SETTLEMENT_RAW_SNAPSHOT= # 资金费率结算原始只读快照输出路径。
  BASIS_OBSERVER_DYNAMIC_TARGET_WSS=0 # candidate 触发 auto-once 验证时，是否按 symbol 动态启动专用 target WSS。
  BASIS_OBSERVER_TARGET_WSS_ROOT=target/arb-opportunity-observer/target-wss # 动态 target WSS 的日志和状态目录。
  BASIS_OBSERVER_TARGET_WSS_BASE_PORT=8830 # 动态 target WSS 从该本地端口开始分配。
  BASIS_OBSERVER_TARGET_WSS_READY_TIMEOUT_SECS=60 # 动态 target WSS 等待真实 WSS quote 就绪的最长秒数。
  BASIS_OBSERVER_TARGET_WSS_RECONNECT_DELAY_SECS=2 # 动态 target WSS 断线重连间隔秒数。
  BASIS_OBSERVER_VALIDATE_AUTO_ONCE=1 # auto-once 模式是否执行候选验证。
  BASIS_OBSERVER_AUTO_ONCE_COOLDOWN_SECS=60 # auto-once 两次触发之间的冷却秒数。
  BASIS_OBSERVER_EXECUTE_LIVE=0 # 是否允许正式实盘下单；1 表示允许。
  BASIS_OBSERVER_LIVE_ACK=0 # 正式实盘确认开关；进入 live 必须为 1。
  BASIS_OBSERVER_AUTO_PRICE_GUARD_BPS=2 # 自动价格保护缓冲，单位 bps。
  BASIS_OBSERVER_CURL_RETRIES=3 # 拉取本地/公开 HTTP 端点的重试次数。
  BASIS_OBSERVER_CURL_RETRY_SLEEP_SECS=1 # HTTP 重试间隔秒数。
  BASIS_OBSERVER_CURL_TIMEOUT_SECS=10 # 单次 HTTP 请求超时秒数。
  BASIS_OBSERVER_STARTUP_CHECK=1 # 启动后是否等待 monitor 健康检查通过。
  BASIS_OBSERVER_STARTUP_WAIT_SECS=180 # 启动健康检查最长等待秒数。
  BASIS_OBSERVER_STOP_DRAIN_SECS=15 # 停止时等待子进程自然收尾的秒数。
  BASIS_OBSERVER_STOP_GRACE_SECS=3 # 停止时发送终止信号后的宽限秒数。
  BASIS_OBSERVER_FOREGROUND=0 # 是否前台运行 observer；1 表示前台。
  BINANCE_BASIS_BIND=127.0.0.1:8796 # Binance basis monitor 监听地址。
  BYBIT_BASIS_BIND=127.0.0.1:8797 # Bybit basis monitor 监听地址。
  OKX_BASIS_BIND=127.0.0.1:8798 # OKX basis monitor 监听地址。
  BITGET_BASIS_BIND=127.0.0.1:8803 # Bitget basis monitor 监听地址。
  ASTER_BASIS_BIND=127.0.0.1:8800 # Aster basis monitor 监听地址。
  HYPERLIQUID_BASIS_BIND=127.0.0.1:8799 # Hyperliquid basis monitor 监听地址。
  FUNDING_ARB_BIND=127.0.0.1:8804 # funding-arb 聚合 monitor 监听地址。
  FUNDING_ARB_MAX_ENTRY_PRICE_DIVERGENCE_BPS=20 # funding-arb 入场时允许的最大价格偏离，单位 bps。
  ASTER_USER=0x... # Aster 账户/user 地址，用于账户归属、查询和订单归属。
  ASTER_SIGNER=0x... # Aster 实际签名/API 地址，必须与 signer 私钥匹配。
  ASTER_SIGNER_PRIVATE=<local-secret> # Aster signer/API 地址对应私钥，只放本机 env。
  HYPERLIQUID_USER=0x... # Hyperliquid 账户/user 地址，用于账户归属、查询和订单归属。
  HYPERLIQUID_SIGNER=0x... # Hyperliquid 实际签名/API/agent 地址，必须与 signer 私钥匹配。
  HYPERLIQUID_SIGNER_PRIVATE=<local-secret> # Hyperliquid signer/API/agent 地址对应私钥，只放本机 env。

Aster 默认按 USDT 结算；Hyperliquid 默认按 USDC 结算，并默认从 Hyperliquid public meta 自动解析 asset id。
如果 user 地址和 signer/API 地址相同，也可以用 ASTER_ADDRESS + ASTER_PRIVATE_KEY、HYPERLIQUID_ADDRESS + HYPERLIQUID_PRIVATE_KEY。

可选 WSS monitor URL:
  BINANCE_SPOT_WSS_MONITOR_URL=http://127.0.0.1:8786/api/binance-wss-book-ticker/status # Binance spot WSS monitor 状态接口。
  BINANCE_PERP_WSS_MONITOR_URL=http://127.0.0.1:8787/api/binance-wss-book-ticker/status # Binance USDM perp WSS monitor 状态接口。
  BYBIT_SPOT_WSS_MONITOR_URL=http://127.0.0.1:8788/api/bybit-wss-book-ticker/status # Bybit spot WSS monitor 状态接口。
  BYBIT_PERP_WSS_MONITOR_URL=http://127.0.0.1:8789/api/bybit-wss-book-ticker/status # Bybit linear perp WSS monitor 状态接口。
  OKX_SPOT_WSS_MONITOR_URL=http://127.0.0.1:8790/api/okx-wss-book-ticker/status # OKX spot WSS monitor 状态接口。
  OKX_PERP_WSS_MONITOR_URL=http://127.0.0.1:8791/api/okx-wss-book-ticker/status # OKX swap WSS monitor 状态接口。
  BITGET_SPOT_WSS_MONITOR_URL=http://127.0.0.1:8792/api/bitget-wss-book-ticker/status # Bitget spot WSS monitor 状态接口。
  BITGET_PERP_WSS_MONITOR_URL=http://127.0.0.1:8793/api/bitget-wss-book-ticker/status # Bitget USDT-FUTURES WSS monitor 状态接口。
  ASTER_PERP_WSS_MONITOR_URL=http://127.0.0.1:8794/api/aster-wss-book-ticker/status # Aster USDT perp WSS monitor 状态接口。
  HYPERLIQUID_PERP_WSS_MONITOR_URL=http://127.0.0.1:8795/api/hyperliquid-wss-book-ticker/status # Hyperliquid perp WSS monitor 状态接口。

输出:
  target/arb-opportunity-observer/logs/realtime-feedback.log
  target/arb-opportunity-observer/opportunities/spot-perp-basis.jsonl
  target/arb-opportunity-observer/opportunities/cross-exchange-funding-arb.jsonl
  target/arb-opportunity-observer/dry-run/dry-run-reports.jsonl
  target/arb-opportunity-observer/live/live-reports.jsonl
  target/arb-opportunity-observer/dry-run/<run-id>/
  target/arb-opportunity-observer/resident-live/spot-perp-basis/
  target/arb-opportunity-observer/resident-live/cross-exchange-funding-arb/
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

supports_auto_once_validation() {
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

supports_basis_resident_live() {
  case "$1" in
    binance|bybit|okx|bitget) return 0 ;;
    aster|hyperliquid) return 1 ;;
    *) return 1 ;;
  esac
}

basis_resident_venues_csv() {
  local monitor
  local joined=""
  for monitor in "${MONITORS[@]}"; do
    if supports_basis_resident_live "${monitor}"; then
      if [[ -n "${joined}" ]]; then
        joined+=","
      fi
      joined+="${monitor}"
    fi
  done
  printf '%s\n' "${joined}"
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

append_basis_monitor_wss_args() {
  local venue="$1"
  case "${venue}" in
    aster)
      if [[ "${EXECUTE_LIVE}" == "1" && -z "${ASTER_PERP_WSS_MONITOR_URL:-}" && "${STRATEGIES}" == *cross-exchange-funding-arb* ]]; then
        die "cross-exchange-funding-arb live requires ASTER_PERP_WSS_MONITOR_URL for aster"
      fi
      [[ -n "${ASTER_PERP_WSS_MONITOR_URL:-}" ]] && MONITOR_ARGS+=(--perp-wss-monitor-url "${ASTER_PERP_WSS_MONITOR_URL}")
      ;;
    hyperliquid)
      if [[ "${EXECUTE_LIVE}" == "1" && -z "${HYPERLIQUID_PERP_WSS_MONITOR_URL:-}" && "${STRATEGIES}" == *cross-exchange-funding-arb* ]]; then
        die "cross-exchange-funding-arb live requires HYPERLIQUID_PERP_WSS_MONITOR_URL for hyperliquid"
      fi
      [[ -n "${HYPERLIQUID_PERP_WSS_MONITOR_URL:-}" ]] && MONITOR_ARGS+=(--perp-wss-monitor-url "${HYPERLIQUID_PERP_WSS_MONITOR_URL}")
      ;;
    *)
      ;;
  esac
}

append_basis_resident_wss_args() {
  local venue="$1"
  local spot_url=""
  local perp_url=""
  local spot_option=""
  local perp_option=""

  case "${venue}" in
    binance)
      spot_url="${BINANCE_SPOT_WSS_MONITOR_URL:-}"
      perp_url="${BINANCE_PERP_WSS_MONITOR_URL:-}"
      spot_option="--binance-spot-wss-monitor-url"
      perp_option="--binance-perp-wss-monitor-url"
      ;;
    bybit)
      spot_url="${BYBIT_SPOT_WSS_MONITOR_URL:-}"
      perp_url="${BYBIT_PERP_WSS_MONITOR_URL:-}"
      spot_option="--bybit-spot-wss-monitor-url"
      perp_option="--bybit-perp-wss-monitor-url"
      ;;
    okx)
      spot_url="${OKX_SPOT_WSS_MONITOR_URL:-}"
      perp_url="${OKX_PERP_WSS_MONITOR_URL:-}"
      spot_option="--okx-spot-wss-monitor-url"
      perp_option="--okx-perp-wss-monitor-url"
      ;;
    bitget)
      spot_url="${BITGET_SPOT_WSS_MONITOR_URL:-}"
      perp_url="${BITGET_PERP_WSS_MONITOR_URL:-}"
      spot_option="--bitget-spot-wss-monitor-url"
      perp_option="--bitget-perp-wss-monitor-url"
      ;;
    *)
      return 0
      ;;
  esac

  if [[ "${EXECUTE_LIVE}" == "1" && ( -z "${spot_url}" || -z "${perp_url}" ) ]]; then
    die "resident spot-perp-basis live requires ${venue} spot/perp WSS monitor URLs"
  fi
  if [[ -n "${spot_url}" || -n "${perp_url}" ]]; then
    if [[ -z "${spot_url}" || -z "${perp_url}" ]]; then
      die "${venue} resident WSS URL must provide both spot and perp/swap monitor URLs"
    fi
    BASIS_RESIDENT_ARGS+=("${spot_option}" "${spot_url}" "${perp_option}" "${perp_url}")
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

target_wss_api_prefix() {
  case "$1" in
    binance) printf '/api/binance-wss-book-ticker' ;;
    bybit) printf '/api/bybit-wss-book-ticker' ;;
    okx) printf '/api/okx-wss-book-ticker' ;;
    bitget) printf '/api/bitget-wss-book-ticker' ;;
    *) return 1 ;;
  esac
}

target_wss_command_market() {
  local venue="$1"
  local leg="$2"
  case "${venue}:${leg}" in
    binance:spot) printf '%s\t%s\n' "binance-wss-book-ticker" "spot" ;;
    binance:perp) printf '%s\t%s\n' "binance-wss-book-ticker" "usdm-perp" ;;
    bybit:spot) printf '%s\t%s\n' "bybit-wss-book-ticker" "spot" ;;
    bybit:perp) printf '%s\t%s\n' "bybit-wss-book-ticker" "linear-perp" ;;
    okx:spot) printf '%s\t%s\n' "okx-wss-book-ticker" "spot" ;;
    okx:perp) printf '%s\t%s\n' "okx-wss-book-ticker" "swap" ;;
    bitget:spot) printf '%s\t%s\n' "bitget-wss-book-ticker" "spot" ;;
    bitget:perp) printf '%s\t%s\n' "bitget-wss-book-ticker" "usdt-futures" ;;
    *) return 1 ;;
  esac
}

target_wss_quote_ready_for_symbol() {
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
  ' >/dev/null 2>> "${LOG_DIR}/jq-errors.log"
}

allocate_target_wss_port() {
  mkdir -p "${TARGET_WSS_STATE_DIR}"
  local lock_dir="${TARGET_WSS_STATE_DIR}/port.lock"
  local next_file="${TARGET_WSS_STATE_DIR}/next-port"
  local next_port="${TARGET_WSS_BASE_PORT}"
  local waited=0

  while ! mkdir "${lock_dir}" 2>/dev/null; do
    sleep 0.1
    waited="$((waited + 1))"
    if (( waited > 300 )); then
      echo "target WSS port allocation lock timed out: ${lock_dir}" >&2
      return 1
    fi
  done

  if [[ -s "${next_file}" ]]; then
    next_port="$(sed -n '1p' "${next_file}")"
  fi
  if ! [[ "${next_port}" =~ ^[0-9]+$ ]]; then
    next_port="${TARGET_WSS_BASE_PORT}"
  fi
  printf '%s\n' "$((next_port + 1))" > "${next_file}"
  rmdir "${lock_dir}" 2>/dev/null || true
  printf '%s\n' "${next_port}"
}

wait_target_wss_monitor_ready() {
  local name="$1"
  local pid="$2"
  local log_file="$3"
  local status_url="$4"
  local ready_symbol="$5"
  local deadline="$((SECONDS + TARGET_WSS_READY_TIMEOUT_SECS))"
  local body
  local status
  local total_rows
  local wss_update_count

  while (( SECONDS <= deadline )); do
    if ! is_alive "${pid}"; then
      echo "target_wss_exited_before_ready name=${name} pid=${pid}" >&2
      [[ -f "${log_file}" ]] && tail -n 40 "${log_file}" >&2 || true
      return 1
    fi
    if body="$(curl -fsS --max-time 2 "${status_url}" 2>> "${LOG_DIR}/curl-errors.log")"; then
      status="$(printf '%s\n' "${body}" | jq -r '.status // "unknown"' 2>> "${LOG_DIR}/jq-errors.log" || printf 'unknown')"
      total_rows="$(printf '%s\n' "${body}" | jq -r '.total_rows // 0' 2>> "${LOG_DIR}/jq-errors.log" || printf '0')"
      wss_update_count="$(printf '%s\n' "${body}" | jq -r '.wss_update_count // 0' 2>> "${LOG_DIR}/jq-errors.log" || printf '0')"
      if [[ "${status}" == "streaming" && "${total_rows}" =~ ^[0-9]+$ && "${total_rows}" -gt 0 && "${wss_update_count}" =~ ^[0-9]+$ && "${wss_update_count}" -gt 0 ]]; then
        if target_wss_quote_ready_for_symbol "${body}" "${ready_symbol}"; then
          echo "target_wss_ready name=${name} status_url=${status_url} symbol=${ready_symbol} rows=${total_rows} wss_updates=${wss_update_count}" >&2
          return 0
        fi
      fi
    fi
    sleep 1
  done

  echo "target_wss_ready_timeout name=${name} status_url=${status_url} symbol=${ready_symbol} timeout_secs=${TARGET_WSS_READY_TIMEOUT_SECS}" >&2
  [[ -f "${log_file}" ]] && tail -n 40 "${log_file}" >&2 || true
  return 1
}

ensure_target_wss_monitor() {
  local venue="$1"
  local symbol="$2"
  local leg="$3"
  local slug
  local key
  local name
  local meta_file
  local pid=""
  local meta_name=""
  local log_file=""
  local status_url=""
  local ready_symbol=""
  local command_market
  local command_name
  local market
  local api_prefix
  local port
  local bind_addr

  [[ "${DYNAMIC_TARGET_WSS}" == "1" ]] || return 1
  mkdir -p "${TARGET_WSS_LOG_DIR}" "${TARGET_WSS_STATE_DIR}"

  slug="$(symbol_slug "${symbol}")"
  key="${venue}-${leg}-${slug}"
  name="target-wss-${key}"
  meta_file="${TARGET_WSS_STATE_DIR}/${key}.tsv"

  if [[ -s "${meta_file}" ]]; then
    while IFS=$'\t' read -r pid meta_name log_file status_url ready_symbol; do
      if [[ "${meta_name}" == "${name}" && "${ready_symbol}" == "${symbol}" ]] && is_alive "${pid}"; then
        if ! wait_target_wss_monitor_ready "${name}" "${pid}" "${log_file}" "${status_url}" "${symbol}"; then
          kill -TERM "${pid}" 2>/dev/null || true
          rm -f "${meta_file}" 2>/dev/null || true
          return 1
        fi
        printf '%s\n' "${status_url}"
        return 0
      fi
    done < "${meta_file}"
  fi

  if ! command_market="$(target_wss_command_market "${venue}" "${leg}")"; then
    echo "target_wss_unsupported venue=${venue} leg=${leg}" >&2
    return 1
  fi
  command_name="$(printf '%s\n' "${command_market}" | cut -f1)"
  market="$(printf '%s\n' "${command_market}" | cut -f2)"
  api_prefix="$(target_wss_api_prefix "${venue}")" || return 1
  port="$(allocate_target_wss_port)" || return 1
  bind_addr="127.0.0.1:${port}"
  status_url="http://${bind_addr}${api_prefix}/status"
  log_file="${TARGET_WSS_LOG_DIR}/${name}.log"

  echo "starting ${name}: symbol=${symbol} leg=${leg} status_url=${status_url}" >&2
  nohup env ARB_RUNTIME_ENABLE_LEGACY_COMMANDS=1 "${RUNTIME_BIN}" "${command_name}" \
    --bind "${bind_addr}" \
    --symbol "${symbol}" \
    --market "${market}" \
    --reconnect-delay-secs "${TARGET_WSS_RECONNECT_DELAY_SECS}" \
    >> "${log_file}" 2>&1 &
  pid="$!"
  printf '%s\t%s\t%s\t%s\t%s\n' "${pid}" "${name}" "${log_file}" "${status_url}" "${symbol}" > "${meta_file}"
  printf '%s\t%s\t%s\n' "${pid}" "${name}" "${log_file}" >> "${PID_FILE}"
  append_json_line "${VALIDATION_EVENTS_JSONL}" \
    --arg venue "${venue}" \
    --arg symbol "${symbol}" \
    --arg leg "${leg}" \
    --arg status_url "${status_url}" \
    --arg log_file "${log_file}" \
    --argjson pid "${pid}" \
    --arg recorded_at "$(date -u +%Y-%m-%dT%H:%M:%SZ)" \
    '{recorded_at:$recorded_at,venue:$venue,symbol:$symbol,leg:$leg,event:"target_wss_started",pid:$pid,status_url:$status_url,log_file:$log_file,mutable_execution_started:false}'

  if ! wait_target_wss_monitor_ready "${name}" "${pid}" "${log_file}" "${status_url}" "${symbol}"; then
    kill -TERM "${pid}" 2>/dev/null || true
    rm -f "${meta_file}" 2>/dev/null || true
    append_json_line "${VALIDATION_EVENTS_JSONL}" \
      --arg venue "${venue}" \
      --arg symbol "${symbol}" \
      --arg leg "${leg}" \
      --arg status_url "${status_url}" \
      --arg recorded_at "$(date -u +%Y-%m-%dT%H:%M:%SZ)" \
      '{recorded_at:$recorded_at,venue:$venue,symbol:$symbol,leg:$leg,event:"target_wss_failed",status_url:$status_url,mutable_execution_started:false}'
    return 1
  fi

  printf '%s\n' "${status_url}"
}

target_wss_args_for_venue_symbol() {
  local venue="$1"
  local symbol="$2"
  local spot_url
  local perp_url

  if [[ "${DYNAMIC_TARGET_WSS}" != "1" ]]; then
    wss_args_for_venue "${venue}"
    return $?
  fi

  case "${venue}" in
    binance|bybit|okx|bitget) ;;
    *) wss_args_for_venue "${venue}"; return $? ;;
  esac

  spot_url="$(ensure_target_wss_monitor "${venue}" "${symbol}" spot)" || return 1
  perp_url="$(ensure_target_wss_monitor "${venue}" "${symbol}" perp)" || return 1
  printf '%s\n%s\n' "${spot_url}" "${perp_url}"
}

target_wss_monitor_alive() {
  local venue="$1"
  local symbol="$2"
  local leg="$3"
  local slug
  local key
  local name
  local meta_file
  local pid
  local meta_name
  local log_file
  local status_url
  local ready_symbol

  slug="$(symbol_slug "${symbol}")"
  key="${venue}-${leg}-${slug}"
  name="target-wss-${key}"
  meta_file="${TARGET_WSS_STATE_DIR}/${key}.tsv"
  [[ -s "${meta_file}" ]] || return 1
  while IFS=$'\t' read -r pid meta_name log_file status_url ready_symbol; do
    if [[ "${meta_name}" == "${name}" && "${ready_symbol}" == "${symbol}" ]] && is_alive "${pid}"; then
      return 0
    fi
  done < "${meta_file}"
  return 1
}

run_target_wss_warmup_job() {
  set +e
  local venue="$1"
  local symbol="$2"
  local recorded_at="$3"
  local status=0
  local finished_at

  if ! ensure_target_wss_monitor "${venue}" "${symbol}" spot >/dev/null; then
    status=1
  fi
  if ! ensure_target_wss_monitor "${venue}" "${symbol}" perp >/dev/null; then
    status=1
  fi
  finished_at="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
  append_json_line "${VALIDATION_EVENTS_JSONL}" \
    --arg venue "${venue}" \
    --arg symbol "${symbol}" \
    --arg recorded_at "${recorded_at}" \
    --arg finished_at "${finished_at}" \
    --argjson exit_status "${status}" \
    '{recorded_at:$recorded_at,finished_at:$finished_at,venue:$venue,symbol:$symbol,event:(if $exit_status == 0 then "target_wss_warmup_ready" else "target_wss_warmup_failed" end),exit_status:$exit_status,mutable_execution_started:false}'
  return 0
}

maybe_start_target_wss_warmup() {
  local venue="$1"
  local symbol="$2"
  local ts="$3"
  local slug
  local inflight_file
  local inflight_pid
  local log_file
  local pid

  [[ "${DYNAMIC_TARGET_WSS}" == "1" ]] || return 0
  case "${venue}" in
    binance|bybit|okx|bitget) ;;
    *) return 0 ;;
  esac
  [[ -n "${symbol}" ]] || return 0

  slug="$(symbol_slug "${symbol}")"
  if target_wss_monitor_alive "${venue}" "${symbol}" spot && target_wss_monitor_alive "${venue}" "${symbol}" perp; then
    return 0
  fi
  inflight_file="${STATE_DIR}/target-wss-warmup-${venue}-${slug}.pid"
  if [[ -s "${inflight_file}" ]]; then
    inflight_pid="$(sed -n '1p' "${inflight_file}")"
    if is_alive "${inflight_pid}"; then
      return 0
    fi
  fi

  log_file="${LOG_DIR}/target-wss-warmup-${venue}-${slug}.log"
  run_target_wss_warmup_job "${venue}" "${symbol}" "${ts}" >> "${log_file}" 2>&1 &
  pid="$!"
  printf '%s\n' "${pid}" > "${inflight_file}"
  printf '%s\t%s\t%s\n' "${pid}" "target-wss-warmup-${venue}-${slug}" "${log_file}" >> "${PID_FILE}"
  append_json_line "${VALIDATION_EVENTS_JSONL}" \
    --arg venue "${venue}" \
    --arg symbol "${symbol}" \
    --arg recorded_at "${ts}" \
    --arg log_file "${log_file}" \
    --argjson pid "${pid}" \
    '{recorded_at:$recorded_at,venue:$venue,symbol:$symbol,event:"target_wss_warmup_started",pid:$pid,log_file:$log_file,mutable_execution_started:false}'
}

start_target_wss_warmups_for_candidates() {
  local venue="$1"
  local body="$2"
  local ts="$3"
  local symbol

  printf '%s\n' "${body}" | jq -r '.rows[]? | select(.is_candidate == true) | .symbol' |
    while IFS= read -r symbol; do
      maybe_start_target_wss_warmup "${venue}" "${symbol}" "${ts}"
    done
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
    *-basis-monitor|funding-arb-monitor|opportunity-recorder|spot-perp-basis-resident-live|funding-arb-resident-live|target-wss-*) return 0 ;;
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
  local reason="stopped by start-basis-opportunity-observer.sh"
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

graceful_stop_started_processes() {
  stop_core_processes
  if ! wait_for_validation_processes; then
    echo "validation drain timed out after ${STOP_DRAIN_SECS:-15}s; terminating remaining validation process(es)."
  fi
  kill_remaining_started_processes
  mark_resident_artifacts_stopped
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
        *-basis-monitor|funding-arb-monitor|opportunity-recorder|spot-perp-basis-resident-live|funding-arb-resident-live|target-wss-*)
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
    printf '%s\n' "${last_body}" | jq -c '{status:(.status // "unknown"),candidate_count:(.candidate_count // 0),updated_at:(.updated_at // "unknown"),last_error:(.last_error // null),rows:((.rows // []) | length)}' >&2 2>> "${LOG_DIR}/jq-errors.log" || true
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
    printf '%s\n' "${last_body}" | jq -c '{status:(.status // "unknown"),candidate_count:(.candidate_count // 0),updated_at:(.updated_at // "unknown"),last_error:(.last_error // null),rows:((.rows // []) | length)}' >&2 2>> "${LOG_DIR}/jq-errors.log" || true
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
      --auto-price-guard-bps "${AUTO_PRICE_GUARD_BPS}"
    )

    if [[ "${EXECUTE_LIVE}" == "1" ]]; then
      args+=(--private-order-events-dir "${PRIVATE_ORDER_EVENTS_DIR}" --execute-live --i-understand-basis-live-orders)
    else
      args+=(--dry-run)
    fi

    if ! wss_values="$(target_wss_args_for_venue_symbol "${venue}" "${symbol}")"; then
      status=1
      finished_at="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
      append_json_line "${EXECUTION_REPORTS_JSONL}" \
        --arg venue "${venue}" \
        --arg symbol "${symbol}" \
        --arg run_id "${run_id}" \
        --arg started_at "${started_at}" \
        --arg finished_at "${finished_at}" \
        --arg out_dir "${out_dir}" \
        --arg execution_mode "${EXECUTION_MODE}" \
        --argjson exit_status "${status}" \
        --argjson mutable_execution_started "${MUTABLE_EXECUTION_STARTED_JSON}" \
        '{execution_mode:$execution_mode,venue:$venue,symbol:$symbol,run_id:$run_id,validation_started_at:$started_at,validation_finished_at:$finished_at,validation_exit_status:$exit_status,validation_result_class:"target_wss_setup_failed",dry_run_output_dir:$out_dir,validation_output_dir:$out_dir,mutable_execution_started:$mutable_execution_started,target_wss_setup_failed:true}'
      echo "[${finished_at}] validation_end venue=${venue} symbol=${symbol} run_id=${run_id} exit_status=${status} result=target_wss_setup_failed out=${out_dir}"
      return 0
    fi
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
        --arg execution_mode "${EXECUTION_MODE}" \
        --argjson exit_status "${status}" \
        --argjson mutable_execution_started "${MUTABLE_EXECUTION_STARTED_JSON}" \
        '. + {
          execution_mode: $execution_mode,
          venue: $venue,
          symbol: $symbol,
          run_id: $run_id,
          validation_started_at: $started_at,
          validation_finished_at: $finished_at,
          validation_exit_status: $exit_status,
          validation_result_class: $validation_result_class,
          dry_run_output_dir: $out_dir,
          validation_output_dir: $out_dir,
          mutable_execution_started: $mutable_execution_started
        }' "${report_file}" >> "${EXECUTION_REPORTS_JSONL}"
    else
      append_json_line "${EXECUTION_REPORTS_JSONL}" \
        --arg venue "${venue}" \
        --arg symbol "${symbol}" \
        --arg run_id "${run_id}" \
        --arg started_at "${started_at}" \
        --arg finished_at "${finished_at}" \
        --arg out_dir "${out_dir}" \
        --arg execution_mode "${EXECUTION_MODE}" \
        --argjson exit_status "${status}" \
        --argjson mutable_execution_started "${MUTABLE_EXECUTION_STARTED_JSON}" \
        '{execution_mode:$execution_mode,venue:$venue,symbol:$symbol,run_id:$run_id,validation_started_at:$started_at,validation_finished_at:$finished_at,validation_exit_status:$exit_status,validation_result_class:"report_missing",dry_run_output_dir:$out_dir,validation_output_dir:$out_dir,mutable_execution_started:$mutable_execution_started,report_missing:true}'
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
  local command_name
  local args
  local asset_id_arg
  local -a hyperliquid_asset_id_args
  local validation_result_class="command_failed"

  if [[ "${EXECUTE_LIVE}" == "1" ]]; then
    command_name="funding-arb-guarded-live-canary-once"
    report_file="${out_dir}/funding_arb_guarded_live_canary_report.json"
  else
    command_name="funding-arb-guarded-dry-run-once"
    report_file="${out_dir}/funding_arb_guarded_dry_run_report.json"
  fi
  started_at="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
  mkdir -p "${out_dir}"

  {
    echo "[${started_at}] funding_validation_start pair_id=${pair_id} symbol=${symbol} run_id=${run_id}"

    args=(
      "${RUNTIME_BIN}" "${command_name}"
      --snapshot "${snapshot_file}" \
      --pair-id "${pair_id}" \
      --config "${CONFIG_PATH}" \
      --out "${out_dir}" \
      --notional-usd "${NOTIONAL_USD}" \
      --taker-fee-bps "${PERP_FEE_BPS}" \
      --slippage-bps "${SLIPPAGE_BPS}" \
      --max-entry-price-divergence-bps "${FUNDING_ARB_MAX_ENTRY_PRICE_DIVERGENCE_BPS}" \
      --min-net-funding-bps "${MIN_NET_BPS}"
    )

    [[ -n "${FUNDING_SETTLEMENT_LEDGER:-}" ]] && args+=(--funding-settlement-ledger "${FUNDING_SETTLEMENT_LEDGER}")
    [[ -n "${FUNDING_SETTLEMENT_RAW_SNAPSHOT:-}" ]] && args+=(--funding-settlement-raw-snapshot "${FUNDING_SETTLEMENT_RAW_SNAPSHOT}")

    if [[ "${EXECUTE_LIVE}" == "1" ]]; then
      args+=(--private-order-events-dir "${PRIVATE_ORDER_EVENTS_DIR}" --execute-live --i-understand-funding-arb-live-orders)
      [[ -n "${HYPERLIQUID_USER:-}" ]] && args+=(--hyperliquid-user "${HYPERLIQUID_USER}")
      [[ -n "${HYPERLIQUID_SOURCE:-}" ]] && args+=(--hyperliquid-source "${HYPERLIQUID_SOURCE}")
      [[ -n "${HYPERLIQUID_VAULT_ADDRESS:-}" ]] && args+=(--hyperliquid-vault-address "${HYPERLIQUID_VAULT_ADDRESS}")
      [[ -n "${HYPERLIQUID_EXPIRES_AFTER_MS:-}" ]] && args+=(--hyperliquid-expires-after-ms "${HYPERLIQUID_EXPIRES_AFTER_MS}")
      if [[ -n "${HYPERLIQUID_ASSET_IDS:-}" ]]; then
        IFS=',' read -r -a hyperliquid_asset_id_args <<< "${HYPERLIQUID_ASSET_IDS}"
        for asset_id_arg in "${hyperliquid_asset_id_args[@]}"; do
          asset_id_arg="${asset_id_arg//[[:space:]]/}"
          [[ -n "${asset_id_arg}" ]] && args+=(--hyperliquid-asset-id "${asset_id_arg}")
        done
      fi
      [[ -n "${ASTER_USER:-}" ]] && args+=(--aster-user "${ASTER_USER}")
      [[ -n "${ASTER_SIGNER:-}" ]] && args+=(--aster-signer "${ASTER_SIGNER}")
      [[ -n "${ASTER_SIGNER_CMD_ENV:-}" ]] && args+=(--aster-signer-cmd-env "${ASTER_SIGNER_CMD_ENV}")
    else
      args+=(--dry-run)
    fi

    "${args[@]}"
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
        --arg execution_mode "${EXECUTION_MODE}" \
        --argjson exit_status "${status}" \
        --argjson mutable_execution_started "${MUTABLE_EXECUTION_STARTED_JSON}" \
        '. + {
          execution_mode: $execution_mode,
          strategy: $strategy,
          pair_id: $pair_id,
          symbol: $symbol,
          run_id: $run_id,
          validation_started_at: $started_at,
          validation_finished_at: $finished_at,
          validation_exit_status: $exit_status,
          validation_result_class: $validation_result_class,
          dry_run_output_dir: $out_dir,
          validation_output_dir: $out_dir,
          mutable_execution_started: $mutable_execution_started
        }' "${report_file}" >> "${EXECUTION_REPORTS_JSONL}"
    else
      append_json_line "${EXECUTION_REPORTS_JSONL}" \
        --arg strategy "cross-exchange-funding-arb" \
        --arg pair_id "${pair_id}" \
        --arg symbol "${symbol}" \
        --arg run_id "${run_id}" \
        --arg started_at "${started_at}" \
        --arg finished_at "${finished_at}" \
        --arg out_dir "${out_dir}" \
        --arg execution_mode "${EXECUTION_MODE}" \
        --argjson exit_status "${status}" \
        --argjson mutable_execution_started "${MUTABLE_EXECUTION_STARTED_JSON}" \
        '{execution_mode:$execution_mode,strategy:$strategy,pair_id:$pair_id,symbol:$symbol,run_id:$run_id,validation_started_at:$started_at,validation_finished_at:$finished_at,validation_exit_status:$exit_status,validation_result_class:"report_missing",dry_run_output_dir:$out_dir,validation_output_dir:$out_dir,mutable_execution_started:$mutable_execution_started,report_missing:true}'
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
  if ! supports_auto_once_validation "${venue}"; then
    append_json_line "${VALIDATION_EVENTS_JSONL}" \
      --arg venue "${venue}" \
      --arg symbol "${symbol}" \
      --arg recorded_at "${ts}" \
      --argjson mutable_execution_started "${MUTABLE_EXECUTION_STARTED_JSON}" \
      '{recorded_at:$recorded_at,venue:$venue,symbol:$symbol,event:"validation_skipped",reason:"auto_once_not_supported",mutable_execution_started:$mutable_execution_started}'
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
  out_dir="${EXECUTION_DIR}/${run_id}"
  job_log="${LOG_DIR}/${venue}-${EXECUTION_MODE}-${run_id}.log"
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
    --arg execution_mode "${EXECUTION_MODE}" \
    --argjson mutable_execution_started "${MUTABLE_EXECUTION_STARTED_JSON}" \
    '{recorded_at:$recorded_at,execution_mode:$execution_mode,venue:$venue,symbol:$symbol,event:"validation_started",run_id:$run_id,pid:$pid,dry_run_output_dir:$out_dir,validation_output_dir:$out_dir,mutable_execution_started:$mutable_execution_started}'
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
  out_dir="${EXECUTION_DIR}/${run_id}"
  job_log="${LOG_DIR}/funding-arb-${EXECUTION_MODE}-${run_id}.log"
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
    --arg execution_mode "${EXECUTION_MODE}" \
    --argjson mutable_execution_started "${MUTABLE_EXECUTION_STARTED_JSON}" \
    '{recorded_at:$recorded_at,execution_mode:$execution_mode,strategy:$strategy,pair_id:$pair_id,symbol:$symbol,event:"validation_started",run_id:$run_id,pid:$pid,dry_run_output_dir:$out_dir,validation_output_dir:$out_dir,mutable_execution_started:$mutable_execution_started}'
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
    start_target_wss_warmups_for_candidates "${venue}" "${body}" "${ts}"
    if [[ "${SPOT_PERP_BASIS_MODE}" == "auto-once" ]]; then
      start_validations_for_candidates "${venue}" "${body}" "${ts}"
    fi
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
    if [[ "${FUNDING_ARB_MODE}" == "auto-once" ]]; then
      start_funding_arb_validations_for_candidates "${body}" "${ts}"
    fi
  fi
}

run_recorder() {
  cd "${REPO_ROOT}"
  trap 'echo "[$(date -u +%Y-%m-%dT%H:%M:%SZ)] recorder_stop" >> "${FEEDBACK_LOG}"; exit 0' INT TERM
  mkdir -p "${LOG_DIR}" "${STATE_DIR}" "${OPPORTUNITY_DIR}" "${EXECUTION_DIR}" "${SNAPSHOT_DIR}" "${PRIVATE_ORDER_EVENTS_DIR}" "${TARGET_WSS_LOG_DIR}" "${TARGET_WSS_STATE_DIR}"
  touch "${OPPORTUNITY_DIR}/all-opportunities.jsonl" "${OPPORTUNITY_DIR}/spot-perp-basis.jsonl" "${OPPORTUNITY_DIR}/cross-exchange-funding-arb.jsonl"
  touch "${VALIDATION_EVENTS_JSONL}" "${EXECUTION_REPORTS_JSONL}"
  IFS=' ' read -r -a RECORDER_MONITORS <<< "${EFFECTIVE_MONITORS}"
  echo "[$(date -u +%Y-%m-%dT%H:%M:%SZ)] recorder_start mode=${EXECUTION_MODE} spot_perp_basis_mode=${SPOT_PERP_BASIS_MODE} funding_arb_mode=${FUNDING_ARB_MODE} monitors=${EFFECTIVE_MONITORS} strategies=${EFFECTIVE_STRATEGIES} interval_secs=${INTERVAL_SECS} min_net_bps=${MIN_NET_BPS}" >> "${FEEDBACK_LOG}"
  append_json_line "${HEALTH_EVENTS_JSONL}" \
    --arg recorded_at "$(date -u +%Y-%m-%dT%H:%M:%SZ)" \
    --arg strategies "${EFFECTIVE_STRATEGIES}" \
    --arg execution_mode "${EXECUTION_MODE}" \
    --arg spot_perp_basis_mode "${SPOT_PERP_BASIS_MODE}" \
    --arg funding_arb_mode "${FUNDING_ARB_MODE}" \
    '{recorded_at:$recorded_at,event:"observer_strategies_configured",execution_mode:$execution_mode,strategies:$strategies,spot_perp_basis_mode:$spot_perp_basis_mode,funding_arb_mode:$funding_arb_mode,mutable_execution_started:false}'
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

if [[ "${1:-}" == "--recorder" ]]; then
  RUN_ROOT="${BASIS_OBSERVER_ROOT:-${REPO_ROOT}/target/arb-opportunity-observer}"
  LOG_DIR="${RUN_ROOT}/logs"
  STATE_DIR="${RUN_ROOT}/state"
  SNAPSHOT_DIR="${RUN_ROOT}/snapshots"
  OPPORTUNITY_DIR="${RUN_ROOT}/opportunities"
  EXECUTE_LIVE="${BASIS_OBSERVER_EXECUTE_LIVE:-0}"
  LIVE_ACK="${BASIS_OBSERVER_LIVE_ACK:-0}"
  if [[ "${EXECUTE_LIVE}" == "1" ]]; then
    [[ "${LIVE_ACK}" == "1" ]] || die "recorder live mode requires BASIS_OBSERVER_LIVE_ACK=1"
    EXECUTION_MODE="live"
    MUTABLE_EXECUTION_STARTED_JSON="true"
    EXECUTION_DIR="${RUN_ROOT}/live"
    EXECUTION_REPORTS_JSONL="${EXECUTION_DIR}/live-reports.jsonl"
  else
    EXECUTION_MODE="paper"
    MUTABLE_EXECUTION_STARTED_JSON="false"
    EXECUTION_DIR="${RUN_ROOT}/dry-run"
    EXECUTION_REPORTS_JSONL="${EXECUTION_DIR}/dry-run-reports.jsonl"
  fi
  DRY_RUN_DIR="${EXECUTION_DIR}"
  PID_FILE="${STATE_DIR}/basis-observer.pids"
  FEEDBACK_LOG="${LOG_DIR}/realtime-feedback.log"
  HEALTH_EVENTS_JSONL="${LOG_DIR}/health-events.jsonl"
  VALIDATION_EVENTS_JSONL="${EXECUTION_DIR}/validation-events.jsonl"
  DRY_RUN_REPORTS_JSONL="${EXECUTION_REPORTS_JSONL}"
  RUNTIME_BIN="${BASIS_OBSERVER_RUNTIME_BIN:-${REPO_ROOT}/target/debug/arb-runtime}"
  CONFIG_PATH="${BASIS_OBSERVER_CONFIG:-templates/personal_guarded_live.preflight.yaml}"
  INTERVAL_SECS="${BASIS_OBSERVER_INTERVAL_SECS:-5}"
  MIN_NET_BPS="${BASIS_OBSERVER_MIN_NET_BPS:-5}"
  NOTIONAL_USD="${BASIS_OBSERVER_NOTIONAL_USD:-100.00}"
  PERP_FEE_BPS="${BASIS_OBSERVER_PERP_FEE_BPS:-5}"
  SLIPPAGE_BPS="${BASIS_OBSERVER_SLIPPAGE_BPS:-5}"
  FUNDING_ARB_MAX_ENTRY_PRICE_DIVERGENCE_BPS="${FUNDING_ARB_MAX_ENTRY_PRICE_DIVERGENCE_BPS:-20}"
  FUNDING_SETTLEMENT_LEDGER="${BASIS_OBSERVER_FUNDING_SETTLEMENT_LEDGER:-${FUNDING_SETTLEMENT_LEDGER:-}}"
  FUNDING_SETTLEMENT_RAW_SNAPSHOT="${BASIS_OBSERVER_FUNDING_SETTLEMENT_RAW_SNAPSHOT:-${FUNDING_SETTLEMENT_RAW_SNAPSHOT:-}}"
  DYNAMIC_TARGET_WSS="${BASIS_OBSERVER_DYNAMIC_TARGET_WSS:-0}"
  TARGET_WSS_ROOT="${BASIS_OBSERVER_TARGET_WSS_ROOT:-${RUN_ROOT}/target-wss}"
  TARGET_WSS_LOG_DIR="${TARGET_WSS_ROOT}/logs"
  TARGET_WSS_STATE_DIR="${TARGET_WSS_ROOT}/state"
  TARGET_WSS_BASE_PORT="${BASIS_OBSERVER_TARGET_WSS_BASE_PORT:-8830}"
  TARGET_WSS_READY_TIMEOUT_SECS="${BASIS_OBSERVER_TARGET_WSS_READY_TIMEOUT_SECS:-60}"
  TARGET_WSS_RECONNECT_DELAY_SECS="${BASIS_OBSERVER_TARGET_WSS_RECONNECT_DELAY_SECS:-2}"
  AUTO_PRICE_GUARD_BPS="${BASIS_OBSERVER_AUTO_PRICE_GUARD_BPS:-2}"
  PRIVATE_ORDER_EVENTS_DIR="${BASIS_OBSERVER_PRIVATE_ORDER_EVENTS_DIR:-${RUN_ROOT}/private-order-events}"
  HYPERLIQUID_USER="${BASIS_OBSERVER_HYPERLIQUID_USER:-${HYPERLIQUID_USER:-}}"
  HYPERLIQUID_SOURCE="${BASIS_OBSERVER_HYPERLIQUID_SOURCE:-${HYPERLIQUID_SOURCE:-}}"
  HYPERLIQUID_VAULT_ADDRESS="${BASIS_OBSERVER_HYPERLIQUID_VAULT_ADDRESS:-${HYPERLIQUID_VAULT_ADDRESS:-}}"
  HYPERLIQUID_EXPIRES_AFTER_MS="${BASIS_OBSERVER_HYPERLIQUID_EXPIRES_AFTER_MS:-${HYPERLIQUID_EXPIRES_AFTER_MS:-}}"
  HYPERLIQUID_ASSET_IDS="${BASIS_OBSERVER_HYPERLIQUID_ASSET_IDS:-${HYPERLIQUID_ASSET_IDS:-}}"
  ASTER_USER="${BASIS_OBSERVER_ASTER_USER:-${ASTER_USER:-}}"
  ASTER_SIGNER="${BASIS_OBSERVER_ASTER_SIGNER:-${ASTER_SIGNER:-}}"
  ASTER_SIGNER_CMD_ENV="${BASIS_OBSERVER_ASTER_SIGNER_CMD_ENV:-${ASTER_SIGNER_CMD_ENV:-}}"
  VALIDATE_AUTO_ONCE="${BASIS_OBSERVER_VALIDATE_AUTO_ONCE:-1}"
  AUTO_ONCE_COOLDOWN_SECS="${BASIS_OBSERVER_AUTO_ONCE_COOLDOWN_SECS:-60}"
  SPOT_PERP_BASIS_MODE="${BASIS_OBSERVER_SPOT_PERP_BASIS_MODE:-resident}"
  FUNDING_ARB_MODE="${BASIS_OBSERVER_FUNDING_ARB_MODE:-resident}"
  if [[ -n "${FUNDING_SETTLEMENT_LEDGER}" && -n "${FUNDING_SETTLEMENT_RAW_SNAPSHOT}" ]]; then
    die "cannot combine BASIS_OBSERVER_FUNDING_SETTLEMENT_LEDGER and BASIS_OBSERVER_FUNDING_SETTLEMENT_RAW_SNAPSHOT"
  fi
  case "${DYNAMIC_TARGET_WSS}" in
    0|1) ;;
    *) die "BASIS_OBSERVER_DYNAMIC_TARGET_WSS must be 0 or 1" ;;
  esac
  [[ "${TARGET_WSS_BASE_PORT}" =~ ^[0-9]+$ ]] || die "BASIS_OBSERVER_TARGET_WSS_BASE_PORT must be numeric"
  [[ "${TARGET_WSS_READY_TIMEOUT_SECS}" =~ ^[0-9]+$ ]] || die "BASIS_OBSERVER_TARGET_WSS_READY_TIMEOUT_SECS must be numeric"
  [[ "${TARGET_WSS_RECONNECT_DELAY_SECS}" =~ ^[0-9]+$ ]] || die "BASIS_OBSERVER_TARGET_WSS_RECONNECT_DELAY_SECS must be numeric"
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
CLI_EXECUTE_LIVE=""
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
      CLI_EXECUTE_LIVE="0"
      shift
      ;;
    --paper)
      CLI_EXECUTE_LIVE="0"
      shift
      ;;
    --live)
      CLI_EXECUTE_LIVE="1"
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
EXECUTE_LIVE="${CLI_EXECUTE_LIVE:-${BASIS_OBSERVER_EXECUTE_LIVE:-0}}"
LIVE_PAUSE_FILE="${BASIS_OBSERVER_LIVE_PAUSE_FILE:-${RUN_ROOT}/LIVE_TRADING_PAUSED}"
LIVE_ACK="${BASIS_OBSERVER_LIVE_ACK:-0}"
if [[ "${EXECUTE_LIVE}" == "1" ]]; then
  if [[ -e "${LIVE_PAUSE_FILE}" && "${BASIS_OBSERVER_IGNORE_LIVE_PAUSE:-0}" != "1" ]]; then
    die "live trading is paused by ${LIVE_PAUSE_FILE}; remove the file only after exchange-side risk is flat"
  fi
  [[ "${LIVE_ACK}" == "1" ]] || die "正式实盘需要设置 BASIS_OBSERVER_LIVE_ACK=1，或改用测试盘 paper"
  EXECUTION_MODE="live"
  MUTABLE_EXECUTION_STARTED_JSON="true"
  EXECUTION_DIR="${RUN_ROOT}/live"
  EXECUTION_REPORTS_JSONL="${EXECUTION_DIR}/live-reports.jsonl"
else
  EXECUTE_LIVE="0"
  EXECUTION_MODE="paper"
  MUTABLE_EXECUTION_STARTED_JSON="false"
  EXECUTION_DIR="${RUN_ROOT}/dry-run"
  EXECUTION_REPORTS_JSONL="${EXECUTION_DIR}/dry-run-reports.jsonl"
fi
DRY_RUN_DIR="${EXECUTION_DIR}"
PID_FILE="${STATE_DIR}/basis-observer.pids"
RUNTIME_BIN="${BASIS_OBSERVER_RUNTIME_BIN:-${REPO_ROOT}/target/debug/arb-runtime}"
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
AUTO_PRICE_GUARD_BPS="${BASIS_OBSERVER_AUTO_PRICE_GUARD_BPS:-2}"
PRIVATE_ORDER_EVENTS_DIR="${BASIS_OBSERVER_PRIVATE_ORDER_EVENTS_DIR:-${RUN_ROOT}/private-order-events}"
SPOT_PERP_BASIS_MODE="${BASIS_OBSERVER_SPOT_PERP_BASIS_MODE:-resident}"
BASIS_RESIDENT_INTERVAL_SECS="${BASIS_OBSERVER_BASIS_RESIDENT_INTERVAL_SECS:-60}"
BASIS_RESIDENT_MAX_LIVE_ENTRIES="${BASIS_OBSERVER_BASIS_RESIDENT_MAX_LIVE_ENTRIES:-1}"
BASIS_RESIDENT_MAX_CONCURRENT_POSITIONS="${BASIS_OBSERVER_BASIS_RESIDENT_MAX_CONCURRENT_POSITIONS:-1}"
BASIS_RESIDENT_MAX_TOTAL_NOTIONAL_USDT="${BASIS_OBSERVER_BASIS_RESIDENT_MAX_TOTAL_NOTIONAL_USDT:-10.00}"
BASIS_RESIDENT_MAX_CYCLES="${BASIS_OBSERVER_BASIS_RESIDENT_MAX_CYCLES:-}"
BASIS_RESIDENT_OUT_DIR="${BASIS_OBSERVER_BASIS_RESIDENT_OUT:-${RUN_ROOT}/resident-live/spot-perp-basis}"
BASIS_RESIDENT_ADL_EVENTS_DIR="${BASIS_OBSERVER_BASIS_RESIDENT_ADL_EVENTS_DIR:-}"
FUNDING_ARB_MODE="${BASIS_OBSERVER_FUNDING_ARB_MODE:-resident}"
FUNDING_ARB_RESIDENT_INTERVAL_SECS="${BASIS_OBSERVER_FUNDING_ARB_RESIDENT_INTERVAL_SECS:-60}"
FUNDING_ARB_RESIDENT_MAX_LIVE_ENTRIES="${BASIS_OBSERVER_FUNDING_ARB_RESIDENT_MAX_LIVE_ENTRIES:-1}"
FUNDING_ARB_RESIDENT_MAX_CYCLES="${BASIS_OBSERVER_FUNDING_ARB_RESIDENT_MAX_CYCLES:-}"
FUNDING_ARB_RESIDENT_OUT_DIR="${BASIS_OBSERVER_FUNDING_ARB_RESIDENT_OUT:-${RUN_ROOT}/resident-live/cross-exchange-funding-arb}"
FUNDING_SETTLEMENT_LEDGER="${BASIS_OBSERVER_FUNDING_SETTLEMENT_LEDGER:-${FUNDING_SETTLEMENT_LEDGER:-}}"
FUNDING_SETTLEMENT_RAW_SNAPSHOT="${BASIS_OBSERVER_FUNDING_SETTLEMENT_RAW_SNAPSHOT:-${FUNDING_SETTLEMENT_RAW_SNAPSHOT:-}}"
DYNAMIC_TARGET_WSS="${BASIS_OBSERVER_DYNAMIC_TARGET_WSS:-0}"
TARGET_WSS_ROOT="${BASIS_OBSERVER_TARGET_WSS_ROOT:-${RUN_ROOT}/target-wss}"
TARGET_WSS_LOG_DIR="${TARGET_WSS_ROOT}/logs"
TARGET_WSS_STATE_DIR="${TARGET_WSS_ROOT}/state"
TARGET_WSS_BASE_PORT="${BASIS_OBSERVER_TARGET_WSS_BASE_PORT:-8830}"
TARGET_WSS_READY_TIMEOUT_SECS="${BASIS_OBSERVER_TARGET_WSS_READY_TIMEOUT_SECS:-60}"
TARGET_WSS_RECONNECT_DELAY_SECS="${BASIS_OBSERVER_TARGET_WSS_RECONNECT_DELAY_SECS:-2}"
HYPERLIQUID_USER="${BASIS_OBSERVER_HYPERLIQUID_USER:-${HYPERLIQUID_USER:-}}"
HYPERLIQUID_SOURCE="${BASIS_OBSERVER_HYPERLIQUID_SOURCE:-${HYPERLIQUID_SOURCE:-}}"
HYPERLIQUID_VAULT_ADDRESS="${BASIS_OBSERVER_HYPERLIQUID_VAULT_ADDRESS:-${HYPERLIQUID_VAULT_ADDRESS:-}}"
HYPERLIQUID_EXPIRES_AFTER_MS="${BASIS_OBSERVER_HYPERLIQUID_EXPIRES_AFTER_MS:-${HYPERLIQUID_EXPIRES_AFTER_MS:-}}"
HYPERLIQUID_ASSET_IDS="${BASIS_OBSERVER_HYPERLIQUID_ASSET_IDS:-${HYPERLIQUID_ASSET_IDS:-}}"
ASTER_USER="${BASIS_OBSERVER_ASTER_USER:-${ASTER_USER:-}}"
ASTER_SIGNER="${BASIS_OBSERVER_ASTER_SIGNER:-${ASTER_SIGNER:-}}"
ASTER_SIGNER_CMD_ENV="${BASIS_OBSERVER_ASTER_SIGNER_CMD_ENV:-${ASTER_SIGNER_CMD_ENV:-}}"
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

case "${SPOT_PERP_BASIS_MODE}" in
  resident|auto-once) ;;
  *) die "BASIS_OBSERVER_SPOT_PERP_BASIS_MODE must be resident or auto-once" ;;
esac

case "${FUNDING_ARB_MODE}" in
  resident|auto-once) ;;
  *) die "BASIS_OBSERVER_FUNDING_ARB_MODE must be resident or auto-once" ;;
esac

if [[ -n "${FUNDING_SETTLEMENT_LEDGER}" && -n "${FUNDING_SETTLEMENT_RAW_SNAPSHOT}" ]]; then
  die "cannot combine BASIS_OBSERVER_FUNDING_SETTLEMENT_LEDGER and BASIS_OBSERVER_FUNDING_SETTLEMENT_RAW_SNAPSHOT"
fi

case "${DYNAMIC_TARGET_WSS}" in
  0|1) ;;
  *) die "BASIS_OBSERVER_DYNAMIC_TARGET_WSS must be 0 or 1" ;;
esac
[[ "${TARGET_WSS_BASE_PORT}" =~ ^[0-9]+$ ]] || die "BASIS_OBSERVER_TARGET_WSS_BASE_PORT must be numeric"
[[ "${TARGET_WSS_READY_TIMEOUT_SECS}" =~ ^[0-9]+$ ]] || die "BASIS_OBSERVER_TARGET_WSS_READY_TIMEOUT_SECS must be numeric"
[[ "${TARGET_WSS_RECONNECT_DELAY_SECS}" =~ ^[0-9]+$ ]] || die "BASIS_OBSERVER_TARGET_WSS_RECONNECT_DELAY_SECS must be numeric"

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

mkdir -p "${LOG_DIR}" "${STATE_DIR}" "${SNAPSHOT_DIR}" "${OPPORTUNITY_DIR}" "${EXECUTION_DIR}" "${PRIVATE_ORDER_EVENTS_DIR}" "${TARGET_WSS_LOG_DIR}" "${TARGET_WSS_STATE_DIR}"
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
echo "building arb-wallet-signer..."
cargo build -p arb-wallet-signer --manifest-path "${REPO_ROOT}/Cargo.toml"

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
  local -a MONITOR_ARGS
  MONITOR_ARGS=(
    "${RUNTIME_BIN}" "${command}"
    --bind "${bind_addr}"
    --out "${out_dir}"
    "${COMMON_ARGS[@]}"
  )
  append_basis_monitor_wss_args "${venue}"

  echo "starting ${venue} monitor: http://${bind_addr}/dashboard"
  nohup "${MONITOR_ARGS[@]}" >> "${log_file}" 2>&1 &
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
    --execution-reports "${EXECUTION_REPORTS_JSONL}" \
    --resident-events "${FUNDING_ARB_RESIDENT_OUT_DIR}/funding_arb_resident_live_events.jsonl" \
    "${source_args[@]}" \
    >> "${log_file}" 2>&1 &
  pid="$!"
  printf '%s\t%s\t%s\n' "${pid}" "funding-arb-monitor" "${log_file}" >> "${PID_FILE}"
  echo "  pid=${pid} log=${log_file}"
}

start_funding_arb_resident_live() {
  local log_file="${LOG_DIR}/funding-arb-resident-live.log"
  local out_dir="${FUNDING_ARB_RESIDENT_OUT_DIR}"
  local pid
  local monitor
  local source
  local max_cycles="${FUNDING_ARB_RESIDENT_MAX_CYCLES:-}"
  local asset_id_arg
  local -a args
  local -a source_args
  local -a hyperliquid_asset_id_args

  source_args=(--clear-sources)
  for monitor in "${MONITORS[@]}"; do
    source="$(status_url "${monitor}")"
    source_args+=(--source "${monitor}=${source}")
  done

  args=(
    "${RUNTIME_BIN}" funding-arb-resident-live
    "${source_args[@]}"
    --config "${CONFIG_PATH}"
    --out "${out_dir}"
    --interval-secs "${FUNDING_ARB_RESIDENT_INTERVAL_SECS}"
    --max-live-entries "${FUNDING_ARB_RESIDENT_MAX_LIVE_ENTRIES}"
    --notional-usd "${NOTIONAL_USD}"
    --taker-fee-bps "${PERP_FEE_BPS}"
    --slippage-bps "${SLIPPAGE_BPS}"
    --max-entry-price-divergence-bps "${FUNDING_ARB_MAX_ENTRY_PRICE_DIVERGENCE_BPS}"
    --min-net-funding-bps "${MIN_NET_BPS}"
  )

  [[ -n "${max_cycles}" ]] && args+=(--max-cycles "${max_cycles}")
  [[ -n "${FUNDING_SETTLEMENT_LEDGER:-}" ]] && args+=(--funding-settlement-ledger "${FUNDING_SETTLEMENT_LEDGER}")
  [[ -n "${FUNDING_SETTLEMENT_RAW_SNAPSHOT:-}" ]] && args+=(--funding-settlement-raw-snapshot "${FUNDING_SETTLEMENT_RAW_SNAPSHOT}")

  if [[ "${EXECUTE_LIVE}" == "1" ]]; then
    args+=(--private-order-events-dir "${PRIVATE_ORDER_EVENTS_DIR}" --execute-live --i-understand-funding-arb-live-orders)
    [[ -n "${HYPERLIQUID_USER:-}" ]] && args+=(--hyperliquid-user "${HYPERLIQUID_USER}")
    [[ -n "${HYPERLIQUID_SOURCE:-}" ]] && args+=(--hyperliquid-source "${HYPERLIQUID_SOURCE}")
    [[ -n "${HYPERLIQUID_VAULT_ADDRESS:-}" ]] && args+=(--hyperliquid-vault-address "${HYPERLIQUID_VAULT_ADDRESS}")
    [[ -n "${HYPERLIQUID_EXPIRES_AFTER_MS:-}" ]] && args+=(--hyperliquid-expires-after-ms "${HYPERLIQUID_EXPIRES_AFTER_MS}")
    if [[ -n "${HYPERLIQUID_ASSET_IDS:-}" ]]; then
      IFS=',' read -r -a hyperliquid_asset_id_args <<< "${HYPERLIQUID_ASSET_IDS}"
      for asset_id_arg in "${hyperliquid_asset_id_args[@]}"; do
        asset_id_arg="${asset_id_arg//[[:space:]]/}"
        [[ -n "${asset_id_arg}" ]] && args+=(--hyperliquid-asset-id "${asset_id_arg}")
      done
    fi
    [[ -n "${ASTER_USER:-}" ]] && args+=(--aster-user "${ASTER_USER}")
    [[ -n "${ASTER_SIGNER:-}" ]] && args+=(--aster-signer "${ASTER_SIGNER}")
    [[ -n "${ASTER_SIGNER_CMD_ENV:-}" ]] && args+=(--aster-signer-cmd-env "${ASTER_SIGNER_CMD_ENV}")
  else
    args+=(--dry-run)
  fi

  echo "starting cross-exchange-funding-arb resident live: out=${out_dir}"
  nohup "${args[@]}" >> "${log_file}" 2>&1 &
  pid="$!"
  printf '%s\t%s\t%s\n' "${pid}" "funding-arb-resident-live" "${log_file}" >> "${PID_FILE}"
  echo "  pid=${pid} log=${log_file}"
}

start_spot_perp_basis_resident_live() {
  local venues_csv
  local log_file="${LOG_DIR}/spot-perp-basis-resident-live.log"
  local out_dir="${BASIS_RESIDENT_OUT_DIR}"
  local pid
  local monitor
  local max_cycles="${BASIS_RESIDENT_MAX_CYCLES:-}"

  venues_csv="$(basis_resident_venues_csv)"
  if [[ -z "${venues_csv}" ]]; then
    echo "spot-perp-basis resident live skipped: no selected resident-supported venues"
    return 0
  fi

  BASIS_RESIDENT_ARGS=(
    "${RUNTIME_BIN}" multi-venue-basis-resident-live
    --venues "${venues_csv}"
    --config "${CONFIG_PATH}"
    --out "${out_dir}"
    --interval-secs "${BASIS_RESIDENT_INTERVAL_SECS}"
    --min-net-bps "${MIN_NET_BPS}"
    --auto-price-guard-bps "${AUTO_PRICE_GUARD_BPS}"
    --max-live-entries "${BASIS_RESIDENT_MAX_LIVE_ENTRIES}"
    --max-concurrent-positions "${BASIS_RESIDENT_MAX_CONCURRENT_POSITIONS}"
    --max-total-notional-usdt "${BASIS_RESIDENT_MAX_TOTAL_NOTIONAL_USDT}"
  )
  if [[ -n "${max_cycles}" ]]; then
    BASIS_RESIDENT_ARGS+=(--max-cycles "${max_cycles}")
  fi
  if [[ -n "${BASIS_RESIDENT_ADL_EVENTS_DIR:-}" ]]; then
    BASIS_RESIDENT_ARGS+=(--adl-events-dir "${BASIS_RESIDENT_ADL_EVENTS_DIR}")
  fi
  if [[ "${EXECUTE_LIVE}" == "1" ]]; then
    BASIS_RESIDENT_ARGS+=(
      --private-order-events-dir "${PRIVATE_ORDER_EVENTS_DIR}"
      --execute-live
      --i-understand-basis-live-orders
    )
  else
    BASIS_RESIDENT_ARGS+=(--dry-run)
  fi

  for monitor in "${MONITORS[@]}"; do
    if supports_basis_resident_live "${monitor}"; then
      append_basis_resident_wss_args "${monitor}"
    fi
  done

  echo "starting spot-perp-basis resident live: venues=${venues_csv} out=${out_dir}"
  nohup "${BASIS_RESIDENT_ARGS[@]}" >> "${log_file}" 2>&1 &
  pid="$!"
  printf '%s\t%s\t%s\n' "${pid}" "spot-perp-basis-resident-live" "${log_file}" >> "${PID_FILE}"
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

if strategy_enabled "spot-perp-basis" && [[ "${SPOT_PERP_BASIS_MODE}" == "resident" ]]; then
  start_spot_perp_basis_resident_live
fi

if strategy_enabled "cross-exchange-funding-arb" && [[ "${FUNDING_ARB_MODE}" == "resident" ]]; then
  start_funding_arb_resident_live
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
  BASIS_OBSERVER_EXECUTE_LIVE="${EXECUTE_LIVE}" \
  BASIS_OBSERVER_LIVE_ACK="${LIVE_ACK}" \
  BASIS_OBSERVER_SPOT_PERP_BASIS_MODE="${SPOT_PERP_BASIS_MODE}" \
  BASIS_OBSERVER_FUNDING_ARB_MODE="${FUNDING_ARB_MODE}" \
  BASIS_OBSERVER_FUNDING_SETTLEMENT_LEDGER="${FUNDING_SETTLEMENT_LEDGER}" \
  BASIS_OBSERVER_FUNDING_SETTLEMENT_RAW_SNAPSHOT="${FUNDING_SETTLEMENT_RAW_SNAPSHOT}" \
  BASIS_OBSERVER_DYNAMIC_TARGET_WSS="${DYNAMIC_TARGET_WSS}" \
  BASIS_OBSERVER_TARGET_WSS_ROOT="${TARGET_WSS_ROOT}" \
  BASIS_OBSERVER_TARGET_WSS_BASE_PORT="${TARGET_WSS_BASE_PORT}" \
  BASIS_OBSERVER_TARGET_WSS_READY_TIMEOUT_SECS="${TARGET_WSS_READY_TIMEOUT_SECS}" \
  BASIS_OBSERVER_TARGET_WSS_RECONNECT_DELAY_SECS="${TARGET_WSS_RECONNECT_DELAY_SECS}" \
  BASIS_OBSERVER_AUTO_PRICE_GUARD_BPS="${AUTO_PRICE_GUARD_BPS}" \
  BASIS_OBSERVER_PRIVATE_ORDER_EVENTS_DIR="${PRIVATE_ORDER_EVENTS_DIR}" \
  BASIS_OBSERVER_HYPERLIQUID_USER="${HYPERLIQUID_USER}" \
  BASIS_OBSERVER_HYPERLIQUID_SOURCE="${HYPERLIQUID_SOURCE}" \
  BASIS_OBSERVER_HYPERLIQUID_VAULT_ADDRESS="${HYPERLIQUID_VAULT_ADDRESS}" \
  BASIS_OBSERVER_HYPERLIQUID_EXPIRES_AFTER_MS="${HYPERLIQUID_EXPIRES_AFTER_MS}" \
  BASIS_OBSERVER_HYPERLIQUID_ASSET_IDS="${HYPERLIQUID_ASSET_IDS}" \
  BASIS_OBSERVER_ASTER_USER="${ASTER_USER}" \
  BASIS_OBSERVER_ASTER_SIGNER="${ASTER_SIGNER}" \
  BASIS_OBSERVER_ASTER_SIGNER_CMD_ENV="${ASTER_SIGNER_CMD_ENV}" \
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
mode: ${EXECUTION_MODE}
spot-perp-basis mode: ${SPOT_PERP_BASIS_MODE}
cross-exchange-funding-arb mode: ${FUNDING_ARB_MODE}
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

validation reports:
  ${EXECUTION_REPORTS_JSONL}

spot-perp-basis resident artifacts:
  ${BASIS_RESIDENT_OUT_DIR}

cross-exchange-funding-arb resident artifacts:
  ${FUNDING_ARB_RESIDENT_OUT_DIR}

中文说明：paper 模式只模拟下单；live 模式会在常驻 runner 或 funding arb 候选满足门禁时传 --execute-live 并提交真实订单。
EOF

if [[ "${FOREGROUND}" == "1" ]]; then
  supervise_started_processes
fi
