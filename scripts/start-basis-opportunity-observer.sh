#!/usr/bin/env bash
set -euo pipefail

# 中文说明：启动六交易所套利机会观察链路。
# 默认运行测试盘：公开行情监控 + 模拟下单验证，不提交订单、不撤单、不转账。
# 只有 BASIS_OBSERVER_EXECUTE_LIVE=1 且 BASIS_OBSERVER_LIVE_ACK=1 时才进入正式实盘，
# 并向底层 guarded live 命令传递真实下单确认参数。
# 默认轮询 spot-perp-basis 和 cross-exchange-funding-arb；正式实盘下两个策略
# 都使用 resident 常驻运行。

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
  3. 默认包含 spot-perp-basis 和 cross-exchange-funding-arb；正式实盘下
     两个策略都以 resident 常驻方式消费全市场候选。
  4. 如果启用 cross-exchange-funding-arb，启动专用 funding-arb-monitor，
     聚合本机 basis /status 快照并记录真实候选，不伪造机会。
  5. 测试盘默认模拟下单；正式实盘必须显式设置 BASIS_OBSERVER_EXECUTE_LIVE=1 和 BASIS_OBSERVER_LIVE_ACK=1。

常用环境变量:
  BASIS_OBSERVER_ROOT=target/arb-opportunity-observer # observer 主运行目录，保存日志、快照、机会和报告。
  BASIS_OBSERVER_STRATEGIES= # 启用策略列表；默认 spot-perp-basis,cross-exchange-funding-arb。
  BASIS_OBSERVER_MONITORS="binance bybit okx bitget aster hyperliquid" # 启用的交易所 monitor 列表。
  BASIS_OBSERVER_INTERVAL_SECS=5 # observer 轮询公开 monitor 的间隔秒数。
  BASIS_OBSERVER_MIN_NET_BPS=5 # 最小净收益阈值，单位 bps。
  ARB_RUNTIME_LOCAL_TZ=Asia/Shanghai # 面向人读的日志展示时区；默认 UTC+8。
  BASIS_OBSERVER_MIN_ABS_FUNDING_RATE=0 # 最小绝对资金费率过滤阈值；0 表示不过滤。
  BASIS_OBSERVER_NOTIONAL_USD=100.00 # 单次候选机会用于计算和下单的目标名义本金，单位美元。
  BASIS_OBSERVER_SPOT_FEE_BPS=10 # spot 腿手续费估算，单位 bps。
  BASIS_OBSERVER_PERP_FEE_BPS=5 # perp 腿手续费估算，单位 bps。
  BASIS_OBSERVER_SLIPPAGE_BPS=5 # 滑点估算，单位 bps。
  BASIS_OBSERVER_CONFIG=templates/personal_guarded_live.preflight.yaml # 风控和执行配置文件路径。
  BASIS_OBSERVER_BASIS_RESIDENT_INTERVAL_SECS=60 # spot-perp-basis 常驻 runner 扫描间隔秒数。
  BASIS_OBSERVER_BASIS_RESIDENT_MAX_CONCURRENT_POSITIONS=1 # spot-perp-basis 最多同时持有的未平仓 position 数。
  BASIS_OBSERVER_BASIS_RESIDENT_MAX_TOTAL_NOTIONAL_USDT=10.00 # spot-perp-basis 总名义本金上限，单位 USDT。
  BASIS_OBSERVER_PERP_TARGET_LEVERAGE=1 # 所有永续交易所默认目标杠杆；实盘非 reduce-only 下单前会先设置该杠杆。
  BASIS_OBSERVER_BINANCE_USDM_LEVERAGE=1 # 可选覆盖 Binance USD-M 永续目标杠杆。
  BASIS_OBSERVER_BINANCE_POSITION_MODE=hedge # Binance USD-M 持仓模式；默认 hedge（双向持仓），设为 one-way 可覆盖；留空且未传配置时才走只读接口查询。
  BASIS_OBSERVER_BYBIT_LINEAR_LEVERAGE=1 # 可选覆盖 Bybit linear 永续目标杠杆。
  BASIS_OBSERVER_BYBIT_POSITION_MODE=hedge # Bybit linear 持仓模式；默认 hedge（双向持仓），设为 one-way 可覆盖；留空且未传配置时才走只读接口查询。
  BASIS_OBSERVER_OKX_SWAP_LEVERAGE=1 # 可选覆盖 OKX swap 永续目标杠杆。
  BASIS_OBSERVER_OKX_POSITION_MODE=long-short # OKX swap 持仓模式；默认 long-short（双向持仓），设为 net 可覆盖；留空且未传配置时才走只读接口查询。
  BASIS_OBSERVER_BITGET_USDT_FUTURES_LEVERAGE=1 # 可选覆盖 Bitget USDT-FUTURES 目标杠杆。
  BASIS_OBSERVER_BITGET_POSITION_MODE=hedge # Bitget USDT-FUTURES 持仓模式；默认 hedge（双向持仓），设为 one-way 可覆盖；留空且未传配置时才走只读接口查询。
  BASIS_OBSERVER_ASTER_PERP_LEVERAGE=1 # 可选覆盖 Aster USDT perp 目标杠杆。
  BASIS_OBSERVER_ASTER_POSITION_MODE=hedge # Aster USDT perp 持仓模式；默认 hedge（双向持仓），设为 one-way 可覆盖；留空且未传配置时才走只读接口查询。
  BASIS_OBSERVER_HYPERLIQUID_PERP_LEVERAGE=1 # 可选覆盖 Hyperliquid perp 目标杠杆。
  BASIS_OBSERVER_BASIS_RESIDENT_MAX_CYCLES= # spot-perp-basis 最大循环次数；留空表示长期运行。
  BASIS_OBSERVER_FUNDING_ARB_RESIDENT_INTERVAL_SECS=60 # cross-exchange-funding-arb 常驻 runner 扫描间隔秒数。
  BASIS_OBSERVER_FUNDING_ARB_RESIDENT_MAX_CYCLES= # cross-exchange-funding-arb 最大循环次数；留空表示长期运行。
  BASIS_OBSERVER_FUNDING_ARB_AUTO_RESIDUAL_DE_RISK=1 # 实盘发现历史 unknown/残腿仓位时，是否自动进入 reduce-only 降风险恢复。
  BASIS_OBSERVER_FUNDING_ARB_ALLOW_UNKNOWN_RECOVERY=0 # 实盘启动时是否允许 resident 自动处理历史 unknown 仓位；默认 0 表示启动前失败。
  BASIS_OBSERVER_FUNDING_ARB_STARTUP_UNKNOWN_READONLY_RECONCILE=1 # 实盘启动时是否先用私有只读仓位快照自动关闭已确认双边为 0 的历史 unknown。
  BASIS_OBSERVER_FUNDING_ARB_AUTO_RECOVER_NONZERO_UNKNOWN=1 # 只读快照确认历史 unknown 仍有非 0 仓位时，是否自动切入 resident reduce-only recovery。
  BASIS_OBSERVER_FUNDING_ARB_STARTUP_DRY_RUN_CHECK=1 # 实盘启动 resident 前是否对当前 funding-arb 候选跑只读 guarded dry-run；候选级风控拒绝只跳过，不阻断 resident 启动。
  BASIS_OBSERVER_FUNDING_ARB_STARTUP_DRY_RUN_LIMIT=3 # 启动前最多检查的 funding-arb 候选数量，按净 funding bps 排序。
  BASIS_OBSERVER_FUNDING_ARB_STARTUP_PRECHECK_OUT= # funding-arb 启动前拦截报告和 dry-run 预检产物目录。
  BASIS_OBSERVER_FUNDING_SETTLEMENT_LEDGER= # 稳定结算账本输入路径；启用 raw snapshot 时必须留空。
  BASIS_OBSERVER_FUNDING_SETTLEMENT_RAW_SNAPSHOT= # 资金费率结算原始只读快照输出路径。
  BASIS_OBSERVER_DYNAMIC_TARGET_WSS=0 # 是否按候选 symbol 动态启动专用 target WSS 预热。
  BASIS_OBSERVER_TARGET_WSS_ROOT=target/arb-opportunity-observer/target-wss # 动态 target WSS 的日志和状态目录。
  BASIS_OBSERVER_TARGET_WSS_BASE_PORT=8830 # 动态 target WSS 从该本地端口开始分配。
  BASIS_OBSERVER_TARGET_WSS_READY_TIMEOUT_SECS=60 # 动态 target WSS 等待真实 WSS quote 就绪的最长秒数。
  BASIS_OBSERVER_TARGET_WSS_RECONNECT_DELAY_SECS=2 # 动态 target WSS 断线重连基础间隔秒数；runtime 会对连续失败做指数退避。
  ARB_RUNTIME_OKX_FUNDING_RATE_CACHE_TTL_SECS=60 # OKX funding-rate 全市场逐合约请求缓存秒数；0 表示每轮都重新请求。
  ARB_RUNTIME_BYBIT_LINEAR_INSTRUMENT_CACHE_TTL_SECS=300 # Bybit linear instruments-info 元数据缓存秒数；0 表示每轮都重新请求。
  ARB_RUNTIME_ASTER_SPOT_PERP_SPOT_SCAN_ENABLED=0 # Aster spot-perp 不可执行时默认跳过 spot/depth REST；1 表示恢复 spot 扫描。
  ARB_RUNTIME_HYPERLIQUID_SPOT_PERP_SPOT_SCAN_ENABLED=0 # Hyperliquid spot-perp 不可执行时默认跳过 spot context；1 表示恢复 spot 扫描。
  ARB_RUNTIME_FUNDING_ARB_DIRECT_PUBLIC_SOURCES_ENABLED= # funding-arb 是否直接读取 perp/funding 公开源；留空时仅 funding-arb 单独运行才默认启用。
  ARB_RUNTIME_LIVE_AUTO_ORDER_ENABLED=0 # 自动实盘开单开关；0 表示只扫描和监测，1 才允许自动开仓。
  BASIS_OBSERVER_EXECUTE_LIVE=0 # 是否允许正式实盘下单；默认可由 ARB_RUNTIME_LIVE_AUTO_ORDER_ENABLED 控制，1 表示允许。
  BASIS_OBSERVER_LIVE_ACK=0 # 正式实盘确认开关；进入 live 必须为 1。
  BASIS_OBSERVER_AUTO_PRICE_GUARD_BPS=2 # 自动价格保护缓冲，单位 bps。
  BASIS_OBSERVER_CURL_RETRIES=3 # 拉取本地/公开 HTTP 端点的重试次数。
  BASIS_OBSERVER_CURL_RETRY_SLEEP_SECS=1 # HTTP 重试间隔秒数。
  BASIS_OBSERVER_CURL_TIMEOUT_SECS=10 # 单次 HTTP 请求超时秒数。
  BASIS_OBSERVER_RUST_RECORDER_ENABLED=1 # resident 模式下用 Rust recorder 轮询本地机会接口，避免每轮生成 curl/jq 子进程；动态 WSS 会自动回退 shell recorder。
  ARB_RUNTIME_FUNDING_ARB_COMPACT_ARTIFACTS=1 # funding-arb resident/canary 每轮快照默认只保留本轮相关 pair；0 表示恢复完整快照。
  BASIS_OBSERVER_BLOCKING_PATH_EVENT_LIMIT=12 # health-events.jsonl 中保留的 blocking_path 前 N 条，取值 1-100，避免大快照触发系统参数长度限制。
  BASIS_OBSERVER_HEALTH_EVENT_SAMPLE_SECS=300 # 成功 poll_ok 健康事件采样间隔；0 表示每轮都写入。
  BASIS_OBSERVER_RESIDENT_HEALTH_TAIL_LINES=200 # 常驻 runner 健康摘要检查最近 N 条事件。
  BASIS_OBSERVER_LOG_ROTATE_BYTES=134217728 # observer JSONL/log 单文件轮转阈值；0 表示禁用轮转。
  BASIS_OBSERVER_LOG_ROTATE_KEEP=4 # observer JSONL/log 保留的轮转文件数量；0 表示禁用轮转。
  ARB_RUNTIME_BINANCE_RECV_WINDOW_MS=30000 # Binance 私有只读请求默认 recvWindow，单位毫秒，允许 1-60000。
  ARB_RUNTIME_BYBIT_RECV_WINDOW_MS=30000 # Bybit 私有只读请求默认 recvWindow，单位毫秒，允许 1-60000。
  ARB_RUNTIME_JSONL_ROTATE_BYTES=134217728 # Rust 常驻 runner JSONL 单文件轮转阈值；0 表示禁用轮转。
  ARB_RUNTIME_JSONL_ROTATE_KEEP=4 # Rust 常驻 runner JSONL 保留的轮转文件数量；0 表示禁用轮转。
  BASIS_OBSERVER_STARTUP_CHECK=1 # 启动后是否等待 monitor 健康检查通过。
  BASIS_OBSERVER_STARTUP_WAIT_SECS=180 # 启动健康检查最长等待秒数。
  BASIS_OBSERVER_STOP_DRAIN_SECS=15 # 停止时等待子进程自然收尾的秒数。
  BASIS_OBSERVER_STOP_GRACE_SECS=3 # 停止时发送终止信号后的宽限秒数。
  BASIS_OBSERVER_RECLAIM_STALE_MONITOR_PORTS=1 # 启动前是否回收本仓库 arb-runtime 残留 monitor 占用的端口；0 表示只报错。
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
  BINANCE_PERP_WSS_MONITOR_URL=http://127.0.0.1:8806/api/binance-wss-book-ticker/status # Binance USDM perp WSS monitor 状态接口。
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

validate_blocking_path_event_limit() {
  local value="$1"
  [[ "${value}" =~ ^([1-9]|[1-9][0-9]|100)$ ]] || die "BASIS_OBSERVER_BLOCKING_PATH_EVENT_LIMIT must be an integer between 1 and 100"
}

validate_non_negative_integer_env() {
  local name="$1"
  local value="$2"
  [[ "${value}" =~ ^[0-9]+$ ]] || die "${name} must be a non-negative integer"
}

validate_log_rotation_settings() {
  validate_non_negative_integer_env "BASIS_OBSERVER_LOG_ROTATE_BYTES" "${LOG_ROTATE_BYTES}"
  validate_non_negative_integer_env "BASIS_OBSERVER_LOG_ROTATE_KEEP" "${LOG_ROTATE_KEEP}"
  if (( LOG_ROTATE_KEEP > 32 )); then
    die "BASIS_OBSERVER_LOG_ROTATE_KEEP must be between 0 and 32"
  fi
}

validate_health_sampling_settings() {
  validate_non_negative_integer_env "BASIS_OBSERVER_HEALTH_EVENT_SAMPLE_SECS" "${HEALTH_EVENT_SAMPLE_SECS}"
  validate_non_negative_integer_env "BASIS_OBSERVER_RESIDENT_HEALTH_TAIL_LINES" "${RESIDENT_HEALTH_TAIL_LINES}"
  if (( RESIDENT_HEALTH_TAIL_LINES > 5000 )); then
    die "BASIS_OBSERVER_RESIDENT_HEALTH_TAIL_LINES must be between 0 and 5000"
  fi
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

pid_file_contains_pid() {
  local needle="$1"
  local pid
  local _name
  local _log_file

  [[ -s "${PID_FILE:-}" ]] || return 1
  while IFS=$'\t' read -r pid _name _log_file; do
    [[ "${pid}" == "${needle}" ]] && return 0
  done < "${PID_FILE}"
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

reclaim_stale_monitor_port() {
  local label="$1"
  local bind_addr="$2"
  local port
  local pid

  [[ "${RECLAIM_STALE_MONITOR_PORTS:-1}" == "1" ]] || return 0
  port="$(bind_port "${bind_addr}")" || return 0

  while IFS= read -r pid; do
    [[ "${pid}" =~ ^[0-9]+$ ]] || continue
    pid_file_contains_pid "${pid}" && continue
    if pid_looks_like_repo_arb_runtime "${pid}"; then
      echo "reclaiming stale monitor port: label=${label} bind=${bind_addr} pid=${pid}"
      kill -TERM "${pid}" 2>/dev/null || true
      if ! wait_for_pid_exit "${pid}" "${STOP_GRACE_SECS:-3}"; then
        echo "stale monitor did not exit after TERM; sending KILL: label=${label} bind=${bind_addr} pid=${pid}"
        kill -KILL "${pid}" 2>/dev/null || true
      fi
    else
      die "cannot bind ${label} on ${bind_addr}: address already in use by pid=${pid}; not a managed ${REPO_ROOT}/target arb-runtime process"
    fi
  done < <(listening_pids_for_port "${port}")
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
  local opportunities_option=""

  case "${venue}" in
    binance)
      spot_url="${BINANCE_SPOT_WSS_MONITOR_URL:-}"
      perp_url="${BINANCE_PERP_WSS_MONITOR_URL:-}"
      spot_option="--binance-spot-wss-monitor-url"
      perp_option="--binance-perp-wss-monitor-url"
      opportunities_option="--binance-opportunities-url"
      ;;
    bybit)
      spot_url="${BYBIT_SPOT_WSS_MONITOR_URL:-}"
      perp_url="${BYBIT_PERP_WSS_MONITOR_URL:-}"
      spot_option="--bybit-spot-wss-monitor-url"
      perp_option="--bybit-perp-wss-monitor-url"
      opportunities_option="--bybit-opportunities-url"
      ;;
    okx)
      spot_url="${OKX_SPOT_WSS_MONITOR_URL:-}"
      perp_url="${OKX_PERP_WSS_MONITOR_URL:-}"
      spot_option="--okx-spot-wss-monitor-url"
      perp_option="--okx-perp-wss-monitor-url"
      opportunities_option="--okx-opportunities-url"
      ;;
    bitget)
      spot_url="${BITGET_SPOT_WSS_MONITOR_URL:-}"
      perp_url="${BITGET_PERP_WSS_MONITOR_URL:-}"
      spot_option="--bitget-spot-wss-monitor-url"
      perp_option="--bitget-perp-wss-monitor-url"
      opportunities_option="--bitget-opportunities-url"
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
  BASIS_RESIDENT_ARGS+=("${opportunities_option}" "$(opportunities_url "${venue}")")
}

append_json_line() {
  local file="$1"
  shift
  rotate_append_file_if_needed "${file}"
  jq -cn "$@" >> "${file}"
}

append_json_line_from_body() {
  local file="$1"
  local body="$2"
  shift 2
  rotate_append_file_if_needed "${file}"
  printf '%s\n' "${body}" | jq -c --argjson blocking_path_limit "${BLOCKING_PATH_EVENT_LIMIT:-12}" "$@" >> "${file}"
}

append_json_body_line() {
  local file="$1"
  local body="$2"
  shift 2
  rotate_append_file_if_needed "${file}"
  printf '%s\n' "${body}" | jq -c "$@" >> "${file}"
}

append_jq_file_line() {
  local file="$1"
  local source_file="$2"
  shift 2
  rotate_append_file_if_needed "${file}"
  jq -c "$@" "${source_file}" >> "${file}"
}

append_text_line() {
  local file="$1"
  local line="$2"
  rotate_append_file_if_needed "${file}"
  printf '%s\n' "${line}" >> "${file}"
}

rotated_append_path() {
  local file="$1"
  local index="$2"
  printf '%s.%s' "${file}" "${index}"
}

rotate_append_file_if_needed() {
  local file="$1"
  local max_bytes="${LOG_ROTATE_BYTES:-${BASIS_OBSERVER_LOG_ROTATE_BYTES:-134217728}}"
  local keep="${LOG_ROTATE_KEEP:-${BASIS_OBSERVER_LOG_ROTATE_KEEP:-4}}"
  local size
  local index
  local from
  local to

  [[ "${max_bytes}" =~ ^[0-9]+$ ]] || return 0
  [[ "${keep}" =~ ^[0-9]+$ ]] || return 0
  (( max_bytes > 0 && keep > 0 )) || return 0
  [[ -f "${file}" ]] || return 0
  size="$(wc -c < "${file}" 2>/dev/null | tr -d '[:space:]' || true)"
  [[ "${size}" =~ ^[0-9]+$ ]] || return 0
  (( size >= max_bytes )) || return 0

  rm -f "$(rotated_append_path "${file}" "${keep}")"
  for (( index = keep - 1; index >= 1; index-- )); do
    from="$(rotated_append_path "${file}" "${index}")"
    to="$(rotated_append_path "${file}" "$((index + 1))")"
    [[ -e "${from}" ]] && mv "${from}" "${to}"
  done
  mv "${file}" "$(rotated_append_path "${file}" 1)"
}

health_sample_state_path() {
  local source="$1"
  local slug
  slug="$(jq -rn --arg source "${source}" '$source | @uri')"
  printf '%s/health-sample-%s.state' "${STATE_DIR}" "${slug}"
}

should_emit_health_event() {
  local source="$1"
  local key="$2"
  local sample_secs="${HEALTH_EVENT_SAMPLE_SECS:-${BASIS_OBSERVER_HEALTH_EVENT_SAMPLE_SECS:-300}}"
  local now
  local state_path
  local last_ts="0"
  local last_key=""

  [[ "${sample_secs}" =~ ^[0-9]+$ ]] || sample_secs="300"
  (( sample_secs > 0 )) || return 0
  now="$(date +%s)"
  state_path="$(health_sample_state_path "${source}")"
  if [[ -f "${state_path}" ]]; then
    last_ts="$(sed -n '1p' "${state_path}" 2>/dev/null || printf '0')"
    last_key="$(sed -n '2p' "${state_path}" 2>/dev/null || true)"
  fi
  [[ "${last_ts}" =~ ^[0-9]+$ ]] || last_ts="0"
  if [[ "${key}" != "${last_key}" || $((now - last_ts)) -ge sample_secs ]]; then
    printf '%s\n%s\n' "${now}" "${key}" > "${state_path}"
    return 0
  fi
  return 1
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
    rm -f "${inflight_file}" 2>/dev/null || true
  fi

  log_file="${LOG_DIR}/target-wss-warmup-${venue}-${slug}.log"
  (
    run_target_wss_warmup_job "${venue}" "${symbol}" "${ts}"
    warmup_status="$?"
    rm -f "${inflight_file}" 2>/dev/null || true
    exit "${warmup_status}"
  ) >> "${log_file}" 2>&1 &
  pid="$!"
  printf '%s\n' "${pid}" > "${inflight_file}"
  # target WSS warmup 只负责拉起 spot/perp monitor 并在 ready 后退出。
  # 它仍记录在 PID 文件中用于停止清理，但前台 supervisor 不把它视为核心常驻进程。
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

reap_started_process() {
  local pid="$1"
  wait "${pid}" 2>/dev/null || true
}

stop_started_processes() {
  local pid
  [[ -s "${PID_FILE}" ]] || return 0
  while IFS=$'\t' read -r pid _name _log; do
    if is_alive "${pid}"; then
      kill -TERM "${pid}" 2>/dev/null || true
    fi
  done < "${PID_FILE}"
  sleep "${STOP_GRACE_SECS:-3}"
  while IFS=$'\t' read -r pid _name _log; do
    if is_alive "${pid}"; then
      kill -KILL "${pid}" 2>/dev/null || true
    fi
    reap_started_process "${pid}"
  done < "${PID_FILE}"
}

is_validation_process_name() {
  [[ "$1" == validation-* ]]
}

is_target_wss_warmup_process_name() {
  [[ "$1" == target-wss-warmup-* ]]
}

is_core_process_name() {
  if is_target_wss_warmup_process_name "$1"; then
    return 1
  fi
  case "$1" in
    *-basis-monitor|funding-arb-monitor|opportunity-recorder|spot-perp-basis-resident-live|funding-arb-resident-live|target-wss-*) return 0 ;;
    *) return 1 ;;
  esac
}

is_supervised_core_process_name() {
  case "$1" in
    *-basis-monitor|funding-arb-monitor|opportunity-recorder|spot-perp-basis-resident-live|funding-arb-resident-live) return 0 ;;
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
    reap_started_process "${pid}"
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
    "${BASIS_RESIDENT_OUT_DIR}/multi_venue_resident_live_state.json" \
    "${BASIS_RESIDENT_OUT_DIR}/multi_venue_resident_live_summary.json" \
    "spot-perp-basis"
  mark_running_resident_artifacts_stopped \
    "${FUNDING_ARB_RESIDENT_OUT_DIR}/funding_arb_resident_live_state.json" \
    "${FUNDING_ARB_RESIDENT_OUT_DIR}/funding_arb_resident_live_summary.json" \
    "cross-exchange-funding-arb"
}

resident_process_alive() {
  local expected_name="$1"
  local pid
  local name
  local _log_file

  [[ -s "${PID_FILE}" ]] || return 1
  while IFS=$'\t' read -r pid name _log_file; do
    if [[ "${name}" == "${expected_name}" ]] && is_alive "${pid}"; then
      return 0
    fi
  done < "${PID_FILE}"
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
    "${BASIS_RESIDENT_OUT_DIR}/multi_venue_resident_live.lock" \
    "${BASIS_RESIDENT_OUT_DIR}/multi_venue_resident_live_summary.json" \
    "${BASIS_RESIDENT_OUT_DIR}/multi_venue_resident_live_state.json" \
    "${BASIS_RESIDENT_OUT_DIR}/multi_venue_resident_live_events.jsonl" \
    "spot-perp-basis-resident-live" \
    "spot-perp-basis"
  cleanup_safe_stale_resident_lock \
    "${FUNDING_ARB_RESIDENT_OUT_DIR}/funding_arb_resident_live.lock" \
    "${FUNDING_ARB_RESIDENT_OUT_DIR}/funding_arb_resident_live_summary.json" \
    "${FUNDING_ARB_RESIDENT_OUT_DIR}/funding_arb_resident_live_state.json" \
    "${FUNDING_ARB_RESIDENT_OUT_DIR}/funding_arb_resident_live_events.jsonl" \
    "funding-arb-resident-live" \
    "cross-exchange-funding-arb"
}

accepted_exit_marker_path() {
  local pid="$1"
  local name="$2"
  local safe_name="${name//[^A-Za-z0-9_.-]/_}"
  printf '%s/accepted-exits/%s.%s' "${STATE_DIR}" "${safe_name}" "${pid}"
}

funding_arb_resident_exit_is_accepted() {
  local summary_path="${FUNDING_ARB_RESIDENT_OUT_DIR}/funding_arb_resident_live_summary.json"
  local events_path="${FUNDING_ARB_RESIDENT_OUT_DIR}/funding_arb_resident_live_events.jsonl"
  local rejected_dispatch_had_no_order_effects=false

  [[ -s "${summary_path}" ]] || return 1
  refresh_funding_arb_summary_position_counts "${summary_path}"
  if [[ -s "${events_path}" ]] && jq -s -e '
    def as_count:
      if type == "number" then .
      elif type == "string" then (tonumber? // 0)
      elif type == "array" then length
      else 0 end;
    ([.[] | select((.event_type // "") == "candidate_cycle")] | last) as $last
    | ($last != null)
    and (($last.mutable_execution_started // false) == true)
    and (($last.dispatch_attempted // false) == true)
    and (($last.dispatch_plan_built // false) == true)
    and (((($last.submitted_receipt_count // $last.submitted_receipts // 0) | as_count) == 0))
    and (((($last.private_confirmation_count // $last.private_confirmations // 0) | as_count) == 0))
    and (($last.residual_risk // null) == null)
  ' "${events_path}" >/dev/null 2>&1; then
    rejected_dispatch_had_no_order_effects=true
  fi
  jq --argjson rejected_dispatch_had_no_order_effects "${rejected_dispatch_had_no_order_effects}" -e '
    def as_count:
      if type == "number" then .
      elif type == "string" then (tonumber? // 0)
      elif type == "array" then length
      else 0 end;
    (.phase // "") == "halted"
    and (((.open_position_count // 0) | as_count) == 0)
    and (((.unknown_position_count // 0) | as_count) == 0)
    and (
      (.halt_reason // "") == "funding arb unknown position recovery cycle completed; resident live stopped before new entries"
      or (.halt_reason // "") == "funding arb position recovery cycle completed; resident live stopped before new entries"
      or (.halt_reason // "") == "funding arb exit-only cycle completed; resident live stopped before new entries"
      or (
        ((.halt_reason // "") | startswith("max live entries reached: "))
        and (((.open_position_count // 0) | as_count) == 0)
        and (((.unknown_position_count // 0) | as_count) == 0)
      )
      or (
        (.halt_reason // "") == "funding arb entry dispatch was rejected after mutable execution started; resident live stopped"
        and $rejected_dispatch_had_no_order_effects
      )
    )
  ' "${summary_path}" >/dev/null 2>&1
}

spot_perp_basis_resident_exit_is_accepted() {
  local summary_path="${BASIS_RESIDENT_OUT_DIR}/multi_venue_resident_live_summary.json"

  strategy_enabled "cross-exchange-funding-arb" || return 1
  [[ -s "${summary_path}" ]] || return 1
  jq -e '
    def as_count:
      if type == "number" then .
      elif type == "string" then (tonumber? // 0)
      elif type == "array" then length
      else 0 end;
    (.phase // "") == "halted"
    and (((.open_position_count // 0) | as_count) == 0)
    and (((.unknown_position_count // 0) | as_count) == 0)
    and (((.live_entry_count // 0) | as_count) == 0)
    and ((.entry_dispatch_attempted // false) == false)
    and ((.exit_dispatch_attempted // false) == false)
  ' "${summary_path}" >/dev/null 2>&1
}

resident_exit_is_accepted() {
  local name="$1"

  case "${name}" in
    spot-perp-basis-resident-live) spot_perp_basis_resident_exit_is_accepted ;;
    funding-arb-resident-live) funding_arb_resident_exit_is_accepted ;;
    *) return 1 ;;
  esac
}

funding_arb_resident_exit_should_restart() {
  local summary_path="${FUNDING_ARB_RESIDENT_OUT_DIR}/funding_arb_resident_live_summary.json"

  [[ -s "${summary_path}" ]] || return 1
  jq -e '
    def as_count:
      if type == "number" then .
      elif type == "string" then (tonumber? // 0)
      elif type == "array" then length
      else 0 end;
    (.phase // "") == "halted"
    and (((.open_position_count // 0) | as_count) == 0)
    and (((.unknown_position_count // 0) | as_count) == 0)
    and (
      (.halt_reason // "") == "funding arb unknown position recovery cycle completed; resident live stopped before new entries"
      or (.halt_reason // "") == "funding arb position recovery cycle completed; resident live stopped before new entries"
      or (.halt_reason // "") == "funding arb exit-only cycle completed; resident live stopped before new entries"
      or (.halt_reason // "") == "funding arb entry dispatch was rejected after mutable execution started; resident live stopped"
    )
  ' "${summary_path}" >/dev/null 2>&1
}

remove_supervised_process_pid_entry() {
  local remove_pid="$1"
  local remove_name="$2"
  local tmp="${PID_FILE}.tmp.$$"
  local pid
  local name
  local log_file

  [[ -s "${PID_FILE}" ]] || return 0
  : > "${tmp}"
  while IFS=$'\t' read -r pid name log_file; do
    if [[ "${pid}" == "${remove_pid}" && "${name}" == "${remove_name}" ]]; then
      continue
    fi
    printf '%s\t%s\t%s\n' "${pid}" "${name}" "${log_file}" >> "${tmp}"
  done < "${PID_FILE}"
  mv "${tmp}" "${PID_FILE}"
}

record_accepted_resident_exit_once() {
  local pid="$1"
  local name="$2"
  local log_file="$3"
  local marker

  marker="$(accepted_exit_marker_path "${pid}" "${name}")"
  if [[ -e "${marker}" ]]; then
    return 0
  fi
  mkdir -p "$(dirname "${marker}")"
  printf '%s\t%s\t%s\n' "${pid}" "${name}" "${log_file}" > "${marker}"
  echo "supervised process completed acceptably: ${name} pid=${pid}"
}

graceful_stop_started_processes() {
  stop_core_processes
  if ! wait_for_validation_processes; then
    echo "validation drain timed out after ${STOP_DRAIN_SECS:-15}s; terminating remaining validation process(es)."
  fi
  kill_remaining_started_processes
  mark_resident_artifacts_stopped
  cleanup_safe_stale_resident_locks
}

supervise_started_processes() {
  local pid
  local name
  local log_file
  local failed
  local restart_funding_arb_resident_pid
  local restart_funding_arb_resident_name
  local restart_funding_arb_resident_log_file

  trap 'echo "stopping foreground basis opportunity observer..."; graceful_stop_started_processes; rm -f "${PID_FILE}"; exit 0' INT TERM
  echo "foreground supervision enabled; press Ctrl-C to stop."

  while true; do
    if [[ ! -s "${PID_FILE}" ]]; then
      echo "pid file removed; foreground supervisor exiting."
      exit 0
    fi

    failed=0
    restart_funding_arb_resident_pid=""
    restart_funding_arb_resident_name=""
    restart_funding_arb_resident_log_file=""
    while IFS=$'\t' read -r pid name log_file; do
      if is_supervised_core_process_name "${name}" && ! is_alive "${pid}"; then
        if resident_exit_is_accepted "${name}"; then
          record_accepted_resident_exit_once "${pid}" "${name}" "${log_file}"
          if [[ "${name}" == "funding-arb-resident-live" ]] \
            && [[ "${FUNDING_ARB_MODE}" == "resident" ]] \
            && strategy_enabled "cross-exchange-funding-arb" \
            && [[ -z "${FUNDING_ARB_RESIDENT_MAX_CYCLES:-}" ]] \
            && funding_arb_resident_exit_should_restart; then
            restart_funding_arb_resident_pid="${pid}"
            restart_funding_arb_resident_name="${name}"
            restart_funding_arb_resident_log_file="${log_file}"
          fi
          continue
        fi
        echo "error: supervised process exited: ${name} pid=${pid}" >&2
        if [[ -n "${log_file}" && -f "${log_file}" ]]; then
          tail -n 40 "${log_file}" >&2 || true
        fi
        failed=1
      fi
    done < "${PID_FILE}"

    if (( failed == 0 )) && [[ -n "${restart_funding_arb_resident_pid}" ]]; then
      if resident_process_alive "funding-arb-resident-live"; then
        remove_supervised_process_pid_entry \
          "${restart_funding_arb_resident_pid}" \
          "${restart_funding_arb_resident_name}"
      else
        echo "funding_arb_resident_restart_after_accepted_exit pid=${restart_funding_arb_resident_pid} log=${restart_funding_arb_resident_log_file}"
        remove_supervised_process_pid_entry \
          "${restart_funding_arb_resident_pid}" \
          "${restart_funding_arb_resident_name}"
        if check_funding_arb_resident_startup_risk; then
          start_funding_arb_resident_live
        else
          failed=1
        fi
      fi
    fi

    if (( failed != 0 )); then
      graceful_stop_started_processes
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

funding_arb_unresolved_unknowns_json() {
  local positions_path="${FUNDING_ARB_RESIDENT_OUT_DIR}/funding_arb_resident_positions.jsonl"

  if [[ ! -s "${positions_path}" ]]; then
    printf '[]\n'
    return 0
  fi

  jq -s -c '
    reduce .[] as $event ({};
      ($event.position_id // "") as $position_id
      | if $position_id == "" then .
        elif ($event.event_type // "") == "position_opened" then
          .[$position_id] = {
            status: "open",
            event_type: "position_opened",
            pair_id: ($event.pair_id // null),
            symbol: ($event.symbol // null),
            opened_at: ($event.opened_at // null),
            closed_at: ($event.closed_at // null),
            reason: ($event.reason // null),
            residual_risk: ($event.residual_risk // null),
            cycle_dir: ($event.cycle_dir // null)
          }
        elif ($event.event_type // "") == "position_unknown" then
          .[$position_id] = {
            status: "unknown",
            event_type: "position_unknown",
            pair_id: ($event.pair_id // null),
            symbol: ($event.symbol // null),
            opened_at: ($event.opened_at // null),
            closed_at: ($event.closed_at // null),
            reason: ($event.reason // null),
            residual_risk: ($event.residual_risk // null),
            cycle_dir: ($event.cycle_dir // null)
          }
        elif ($event.event_type // "") == "position_closed" then
          .[$position_id] = {
            status: "closed",
            event_type: "position_closed",
            pair_id: ($event.pair_id // null),
            symbol: ($event.symbol // null),
            opened_at: ($event.opened_at // null),
            closed_at: ($event.closed_at // null),
            reason: ($event.reason // null),
            residual_risk: ($event.residual_risk // null),
            cycle_dir: ($event.cycle_dir // null)
          }
        elif ($event.event_type // "") == "position_flat_cancelled" then
          .[$position_id] = {
            status: "flat_cancelled",
            event_type: "position_flat_cancelled",
            pair_id: ($event.pair_id // null),
            symbol: ($event.symbol // null),
            opened_at: ($event.opened_at // null),
            closed_at: ($event.closed_at // null),
            reason: ($event.reason // null),
            residual_risk: ($event.residual_risk // null),
            cycle_dir: ($event.cycle_dir // null)
          }
        else .
        end
    )
    | to_entries
    | map(select(.value.status == "unknown"))
  ' "${positions_path}"
}

funding_arb_open_positions_json() {
  local positions_path="${FUNDING_ARB_RESIDENT_OUT_DIR}/funding_arb_resident_positions.jsonl"

  if [[ ! -s "${positions_path}" ]]; then
    printf '[]\n'
    return 0
  fi

  jq -s -c '
    reduce .[] as $event ({};
      ($event.position_id // "") as $position_id
      | if $position_id == "" then .
        elif ($event.event_type // "") == "position_opened" then
          .[$position_id] = {
            status: "open",
            event_type: "position_opened",
            pair_id: ($event.pair_id // null),
            symbol: ($event.symbol // null),
            opened_at: ($event.opened_at // null),
            closed_at: ($event.closed_at // null),
            reason: ($event.reason // null),
            residual_risk: ($event.residual_risk // null),
            cycle_dir: ($event.cycle_dir // null),
            position_state_path: ($event.position_state_path // null),
            recovered_from_unknown: ($event.recovered_from_unknown // false)
          }
        elif ($event.event_type // "") == "position_unknown" then
          .[$position_id] = {
            status: "unknown",
            event_type: "position_unknown",
            pair_id: ($event.pair_id // null),
            symbol: ($event.symbol // null),
            opened_at: ($event.opened_at // null),
            closed_at: ($event.closed_at // null),
            reason: ($event.reason // null),
            residual_risk: ($event.residual_risk // null),
            cycle_dir: ($event.cycle_dir // null)
          }
        elif ($event.event_type // "") == "position_closed" then
          .[$position_id] = {
            status: "closed",
            event_type: "position_closed",
            pair_id: ($event.pair_id // null),
            symbol: ($event.symbol // null),
            opened_at: ($event.opened_at // null),
            closed_at: ($event.closed_at // null),
            reason: ($event.reason // null),
            residual_risk: ($event.residual_risk // null),
            cycle_dir: ($event.cycle_dir // null)
          }
        elif ($event.event_type // "") == "position_flat_cancelled" then
          .[$position_id] = {
            status: "flat_cancelled",
            event_type: "position_flat_cancelled",
            pair_id: ($event.pair_id // null),
            symbol: ($event.symbol // null),
            opened_at: ($event.opened_at // null),
            closed_at: ($event.closed_at // null),
            reason: ($event.reason // null),
            residual_risk: ($event.residual_risk // null),
            cycle_dir: ($event.cycle_dir // null)
          }
        else .
        end
    )
    | to_entries
    | map(select(.value.status == "open"))
  ' "${positions_path}"
}

funding_arb_current_candidate_pair_ids() {
  local snapshot_path="${1:-${SNAPSHOT_DIR}/funding-arb/funding_arb_monitor_snapshot.json}"

  [[ -s "${snapshot_path}" ]] || return 1
  jq -r --argjson limit "${FUNDING_ARB_STARTUP_DRY_RUN_LIMIT}" '
    [(.rows // [])[]
      | select((.is_candidate // false) == true)
    ]
    | sort_by(((.net_funding_bps // "0") | tonumber? // 0))
    | reverse
    | .[:$limit][]
    | .pair_id
  ' "${snapshot_path}"
}

write_funding_arb_startup_block_report() {
  local reason="$1"
  local unknowns_json="${2:-[]}"
  local pair_id="${3:-}"
  local artifact_path="${4:-}"
  local summary_unknown_count="${5:-0}"
  local summary_open_count="${6:-0}"
  local report_path="${FUNDING_ARB_STARTUP_PRECHECK_DIR}/funding_arb_startup_block_report.json"

  mkdir -p "${FUNDING_ARB_STARTUP_PRECHECK_DIR}"
  jq -n \
    --arg updated_at "$(date -u +%Y-%m-%dT%H:%M:%SZ)" \
    --arg reason "${reason}" \
    --arg pair_id "${pair_id}" \
    --arg artifact_path "${artifact_path}" \
    --arg resident_out_dir "${FUNDING_ARB_RESIDENT_OUT_DIR}" \
    --arg snapshot_path "${SNAPSHOT_DIR}/funding-arb/funding_arb_monitor_snapshot.json" \
    --argjson execute_live "${EXECUTE_LIVE}" \
    --argjson allow_unknown_recovery "${FUNDING_ARB_ALLOW_UNKNOWN_RECOVERY}" \
    --argjson summary_unknown_count "${summary_unknown_count}" \
    --argjson summary_open_count "${summary_open_count}" \
    --argjson unresolved_unknowns "${unknowns_json}" \
    '{
      schema_version: "1.0.0",
      updated_at: $updated_at,
      phase: "blocked",
      reason: $reason,
      execute_live: ($execute_live == 1),
      allow_unknown_recovery: ($allow_unknown_recovery == 1),
      resident_out_dir: $resident_out_dir,
      snapshot_path: $snapshot_path,
      pair_id: (if $pair_id == "" then null else $pair_id end),
      artifact_path: (if $artifact_path == "" then null else $artifact_path end),
      summary_unknown_count: $summary_unknown_count,
      summary_open_count: $summary_open_count,
      unresolved_unknown_count: ($unresolved_unknowns | length),
      unresolved_unknowns: $unresolved_unknowns
    }' > "${report_path}"
  echo "funding_arb_startup_block_report=${report_path}" >&2
}

funding_arb_guarded_dry_run_report_is_stale_candidate_skip() {
  local report_path="$1"
  jq -e '
    def allowed_blocker:
      . == "funding-arb execution constraints cannot be checked without a candidate"
      or . == "funding-arb execution preflight cannot run because no candidate was emitted"
      or . == "funding-arb manual approval plan preview is not built, so the manual gate cannot be released"
      or . == "funding-arb manual approval plan preview status is NotAttempted"
      or . == "private execution fill/order reconciliation snapshot was not provided"
      or . == "risk decision is unavailable; no funding-arb dispatch may be built"
      or . == "strategy did not emit a funding-arb candidate";

    (.signal_allowed == false)
    and (.risk_decision == "NoCandidate")
    and (.dispatch_plan_built == false)
    and ((.dispatch_request_count // 0) == 0)
    and ((.private_accounts.status // "") == "Matched")
    and ((.private_positions.status // "") == "Matched")
    and ((.snapshot_lineage.status // "") == "Matched")
    and ((.venue_execution_capability.status // "") == "Matched")
    and (((.live_blocking_reasons // []) | length) > 0)
    and (((.live_blocking_reasons // []) | all(allowed_blocker)))
  ' "${report_path}" >/dev/null
}

funding_arb_guarded_dry_run_log_is_stale_candidate_skip() {
  local log_file="$1"
  grep -F "funding arb monitor row is not a candidate: " "${log_file}" >/dev/null 2>&1
}

clear_funding_arb_startup_block_report() {
  local report_path="${FUNDING_ARB_STARTUP_PRECHECK_DIR}/funding_arb_startup_block_report.json"
  [[ -e "${report_path}" ]] || return 0
  rm -f "${report_path}"
}

funding_arb_private_positions_snapshot_confirms_flat() {
  local raw_snapshot_path="$1"
  local symbol="$2"
  local report_path="$3"
  local flat_status

  if jq -e --arg symbol "${symbol}" '
    def norm:
      tostring
      | ascii_upcase
      | gsub("[^A-Z0-9]"; "");
    def symbol_matches($raw):
      ($raw | norm) as $raw_symbol
      | ($symbol | norm) as $target_symbol
      | $raw_symbol != ""
      and (
        $raw_symbol == $target_symbol
        or $raw_symbol == ($target_symbol + "SWAP")
        or $raw_symbol == ($target_symbol + "PERP")
        or $target_symbol == ($raw_symbol + "USDT")
        or $target_symbol == ($raw_symbol + "USD")
        or $target_symbol == ($raw_symbol + "USDC")
      );
    def numeric_quantity:
      if type == "number" then .
      else (tostring | tonumber? // 0)
      end;
    def quantity_values:
      [
        .positionAmt?,
        .position_amount?,
        .positionAmtAbs?,
        .positionSize?,
        .position_size?,
        .size?,
        .szi?,
        .sz?,
        .pos?,
        .qty?,
        .quantity?,
        .total?
      ]
      | map(select(. != null and . != ""))
      | map(numeric_quantity);

    (.status == "complete")
    and (((.source_errors // []) | length) == 0)
    and (((.statements // []) | length) > 0)
    and (
      [
        (.statements // [])[] as $statement
        | (($statement.payload // "null") | fromjson? // empty)
        | .. | objects
        | select(symbol_matches(.symbol? // .instId? // .instrument? // .coin? // .asset? // ""))
        | quantity_values
        | map(abs)
        | max // 0
      ]
      | all(. == 0)
    )
  ' "${raw_snapshot_path}" >/dev/null; then
    flat_status=0
  else
    flat_status="$?"
  fi

  jq -c --arg symbol "${symbol}" '
    def norm:
      tostring
      | ascii_upcase
      | gsub("[^A-Z0-9]"; "");
    def symbol_matches($raw):
      ($raw | norm) as $raw_symbol
      | ($symbol | norm) as $target_symbol
      | $raw_symbol != ""
      and (
        $raw_symbol == $target_symbol
        or $raw_symbol == ($target_symbol + "SWAP")
        or $raw_symbol == ($target_symbol + "PERP")
        or $target_symbol == ($raw_symbol + "USDT")
        or $target_symbol == ($raw_symbol + "USD")
        or $target_symbol == ($raw_symbol + "USDC")
      );
    def numeric_quantity:
      if type == "number" then .
      else (tostring | tonumber? // 0)
      end;
    def quantity_values:
      [
        .positionAmt?,
        .position_amount?,
        .positionAmtAbs?,
        .positionSize?,
        .position_size?,
        .size?,
        .szi?,
        .sz?,
        .pos?,
        .qty?,
        .quantity?,
        .total?
      ]
      | map(select(. != null and . != ""))
      | map(numeric_quantity);
    [
      (.statements // [])[] as $statement
      | (($statement.payload // "null") | fromjson? // empty)
      | .. | objects
      | select(symbol_matches(.symbol? // .instId? // .instrument? // .coin? // .asset? // ""))
      | {
          venue_family: ($statement.venue_family // null),
          account_id: ($statement.account_id // null),
          quantity_values: quantity_values,
          max_abs_quantity: ((quantity_values | map(abs) | max) // 0)
        }
    ] as $matches
    | {
        symbol: $symbol,
        status: (.status // null),
        source_error_count: ((.source_errors // []) | length),
        statement_count: ((.statements // []) | length),
        matching_position_count: ($matches | length),
        nonzero_position_count: ($matches | map(select(.max_abs_quantity != 0)) | length),
        matches: $matches
      }
  ' "${raw_snapshot_path}" > "${report_path}"
  return "${flat_status}"
}

append_funding_arb_startup_reconciled_position_closed() {
  local position_id="$1"
  local pair_id="$2"
  local symbol="$3"
  local pair_dir="$4"
  local flat_report_path="$5"
  local opened_at="${6:-}"
  local positions_path="${FUNDING_ARB_RESIDENT_OUT_DIR}/funding_arb_resident_positions.jsonl"
  local closed_at
  closed_at="$(date -u +"%Y-%m-%dT%H:%M:%SZ")"

  mkdir -p "${FUNDING_ARB_RESIDENT_OUT_DIR}"
  jq -nc \
    --arg closed_at "${closed_at}" \
    --arg cycle_dir "${pair_dir}" \
    --arg flat_report_path "${flat_report_path}" \
    --arg opened_at "${opened_at}" \
    --arg pair_id "${pair_id}" \
    --arg position_id "${position_id}" \
    --arg symbol "${symbol}" \
    '{
      closed_at: $closed_at,
      cycle: 0,
      cycle_dir: $cycle_dir,
      decision: "flat_private_snapshot",
      event_type: "position_closed",
      flat_reconciliation_report: $flat_report_path,
      net_funding_bps: null,
      notional_usdt: "unknown",
      opened_at: (if $opened_at == "" then null else $opened_at end),
      pair_id: $pair_id,
      position_id: $position_id,
      position_state_path: null,
      private_confirmation_count: 0,
      reason: "startup private read-only reconciliation confirmed flat positions",
      residual_risk: null,
      status: "closed",
      submitted_receipt_count: 0,
      symbol: $symbol
    }' >> "${positions_path}"
}

funding_arb_reconcile_unknowns_with_private_readonly() {
  local unknowns_json="$1"
  local position_id
  local pair_id
  local symbol
  local opened_at
  local safe_pair
  local run_id
  local pair_dir
  local readonly_dir
  local log_file
  local flat_report_path
  local reconciled_count=0
  local failed_count=0
  local nonzero_confirmed_count=0
  local -a readonly_args

  FUNDING_ARB_STARTUP_NONZERO_UNKNOWN_CONFIRMED=0
  [[ "${FUNDING_ARB_STARTUP_UNKNOWN_READONLY_RECONCILE}" == "1" ]] || return 1
  [[ "${EXECUTE_LIVE}" == "1" ]] || return 1

  while IFS=$'\t' read -r position_id pair_id symbol opened_at; do
    [[ -n "${position_id}" && -n "${pair_id}" && -n "${symbol}" ]] || continue
    safe_pair="${pair_id//[^A-Za-z0-9_.-]/_}"
    run_id="$(date -u +%Y%m%dT%H%M%SZ)"
    pair_dir="${FUNDING_ARB_STARTUP_PRECHECK_DIR}/${run_id}/unknown-readonly/${safe_pair}"
    readonly_dir="${pair_dir}/private-readonly"
    log_file="${pair_dir}/readonly-reconcile.log"
    flat_report_path="${pair_dir}/flat-position-reconciliation.json"
    mkdir -p "${pair_dir}"

    readonly_args=(
      "${RUNTIME_BIN}" funding-arb-private-readonly-snapshot-once
      --snapshot "${SNAPSHOT_DIR}/funding-arb/funding_arb_monitor_snapshot.json"
      --pair-id "${pair_id}"
      --config "${CONFIG_PATH}"
      --out "${readonly_dir}"
    )
    [[ -n "${FUNDING_SETTLEMENT_RAW_SNAPSHOT:-}" ]] && readonly_args+=(--funding-settlement-raw-snapshot "${FUNDING_SETTLEMENT_RAW_SNAPSHOT}")
    [[ -n "${HYPERLIQUID_USER:-}" ]] && readonly_args+=(--hyperliquid-user "${HYPERLIQUID_USER}")
    [[ -n "${ASTER_USER:-}" ]] && readonly_args+=(--aster-user "${ASTER_USER}")
    [[ -n "${ASTER_SIGNER:-}" ]] && readonly_args+=(--aster-signer "${ASTER_SIGNER}")
    [[ -n "${ASTER_SIGNER_CMD_ENV:-}" ]] && readonly_args+=(--aster-signer-cmd-env "${ASTER_SIGNER_CMD_ENV}")

    echo "funding_arb_unknown_readonly_reconcile pair_id=${pair_id} position_id=${position_id} out=${readonly_dir}"
    if ! "${readonly_args[@]}" >> "${log_file}" 2>&1; then
      echo "warning: funding-arb unknown readonly reconciliation failed for pair_id=${pair_id}; log=${log_file}" >&2
      tail -n 20 "${log_file}" >&2 || true
      failed_count="$((failed_count + 1))"
      continue
    fi

    if funding_arb_private_positions_snapshot_confirms_flat "${readonly_dir}/funding_arb_private_position_raw_snapshot.json" "${symbol}" "${flat_report_path}"; then
      append_funding_arb_startup_reconciled_position_closed "${position_id}" "${pair_id}" "${symbol}" "${pair_dir}" "${flat_report_path}" "${opened_at}"
      echo "funding_arb_unknown_readonly_reconciled_flat pair_id=${pair_id} position_id=${position_id} report=${flat_report_path}"
      reconciled_count="$((reconciled_count + 1))"
    else
      if jq -e '(.status == "complete") and ((.source_error_count // 0) == 0) and ((.nonzero_position_count // 0) > 0)' "${flat_report_path}" >/dev/null 2>&1; then
        echo "warning: funding-arb unknown readonly reconciliation confirmed non-zero positions for pair_id=${pair_id}; report=${flat_report_path}" >&2
        nonzero_confirmed_count="$((nonzero_confirmed_count + 1))"
      fi
      echo "warning: funding-arb unknown readonly reconciliation did not confirm flat positions for pair_id=${pair_id}; report=${flat_report_path}" >&2
      failed_count="$((failed_count + 1))"
    fi
  done < <(
    printf '%s\n' "${unknowns_json}" | jq -r '
      .[]
      | [
          .key,
          (.value.pair_id // ""),
          (.value.symbol // ""),
          (.value.opened_at // "")
        ]
      | @tsv
    '
  )

  if (( nonzero_confirmed_count > 0 )); then
    FUNDING_ARB_STARTUP_NONZERO_UNKNOWN_CONFIRMED=1
  fi
  (( reconciled_count > 0 && failed_count == 0 ))
}

check_funding_arb_resident_unknown_startup_risk() {
  local unknowns_json
  local open_positions_json
  local unknown_count
  local open_count
  local summary_path="${FUNDING_ARB_RESIDENT_OUT_DIR}/funding_arb_resident_live_summary.json"
  local summary_unknown_count=0
  local summary_open_count=0

  [[ "${EXECUTE_LIVE}" == "1" ]] || return 0
  command -v jq >/dev/null 2>&1 || {
    echo "error: jq is required for live funding-arb startup risk checks" >&2
    return 1
  }

  unknowns_json="$(funding_arb_unresolved_unknowns_json)"
  unknown_count="$(printf '%s\n' "${unknowns_json}" | jq 'length')"
  if [[ -s "${summary_path}" ]]; then
    refresh_funding_arb_summary_position_counts "${summary_path}"
    summary_unknown_count="$(jq -r '(.unknown_position_count // 0) | tonumber' "${summary_path}" 2>> "${LOG_DIR}/jq-errors.log" || printf '0')"
    summary_open_count="$(jq -r '(.open_position_count // 0) | tonumber' "${summary_path}" 2>> "${LOG_DIR}/jq-errors.log" || printf '0')"
  fi

  if (( unknown_count > 0 || summary_unknown_count > 0 )); then
    if [[ "${FUNDING_ARB_AUTO_RESIDUAL_DE_RISK}" == "1" ]]; then
      FUNDING_ARB_ALLOW_UNKNOWN_RECOVERY=1
      FUNDING_ARB_STARTUP_AUTO_RESIDUAL_DERISK=1
      clear_funding_arb_startup_block_report
      echo "warning: funding-arb live startup found unresolved unknown positions; auto residual de-risk is enabled, resident will run reduce-only recovery before new entries" >&2
      return 0
    fi

    if [[ "${FUNDING_ARB_ALLOW_UNKNOWN_RECOVERY}" == "1" ]]; then
      echo "warning: funding-arb live startup will allow unknown recovery; unresolved_unknown_positions=${unknown_count} summary_unknown_count=${summary_unknown_count}" >&2
      return 0
    fi

    if funding_arb_reconcile_unknowns_with_private_readonly "${unknowns_json}"; then
      unknowns_json="$(funding_arb_unresolved_unknowns_json)"
      unknown_count="$(printf '%s\n' "${unknowns_json}" | jq 'length')"
      if [[ -s "${summary_path}" ]]; then
        refresh_funding_arb_summary_position_counts "${summary_path}"
        summary_unknown_count="$(jq -r '(.unknown_position_count // 0) | tonumber' "${summary_path}" 2>> "${LOG_DIR}/jq-errors.log" || printf '0')"
        summary_open_count="$(jq -r '(.open_position_count // 0) | tonumber' "${summary_path}" 2>> "${LOG_DIR}/jq-errors.log" || printf '0')"
      fi
      if (( unknown_count == 0 && summary_unknown_count == 0 )); then
        clear_funding_arb_startup_block_report
        echo "funding_arb_unknown_readonly_reconcile_ok reason=all_unknown_positions_confirmed_flat"
        return 0
      fi
    fi

    if [[ "${FUNDING_ARB_STARTUP_NONZERO_UNKNOWN_CONFIRMED:-0}" == "1" && "${FUNDING_ARB_AUTO_RECOVER_NONZERO_UNKNOWN}" == "1" ]]; then
      FUNDING_ARB_ALLOW_UNKNOWN_RECOVERY=1
      echo "warning: funding-arb live startup confirmed non-zero unknown positions; enabling resident reduce-only recovery before new entries" >&2
      return 0
    fi

    write_funding_arb_startup_block_report "unresolved_unknown_positions" "${unknowns_json}" "" "" "${summary_unknown_count}" "${summary_open_count}"
    echo "error: funding-arb live startup blocked before starting background processes: unresolved unknown position state exists." >&2
    printf '%s\n' "${unknowns_json}" | jq -r '
      .[:8][]
      | "  - position_id=\(.key) pair_id=\(.value.pair_id // "unknown") symbol=\(.value.symbol // "unknown") residual_risk=\(.value.residual_risk // "null") reason=\(.value.reason // "unknown")"
    ' >&2 2>> "${LOG_DIR}/jq-errors.log" || true
    echo "set BASIS_OBSERVER_FUNDING_ARB_ALLOW_UNKNOWN_RECOVERY=1 only after deciding that resident reduce-only recovery is intended." >&2
    return 1
  fi

  open_positions_json="$(funding_arb_open_positions_json)"
  open_count="$(printf '%s\n' "${open_positions_json}" | jq 'length')"
  if (( open_count > 0 || summary_open_count > 0 )); then
    if [[ "${FUNDING_ARB_STARTUP_OPEN_POSITION_CONTINUE:-0}" == "1" ]]; then
      clear_funding_arb_startup_block_report
      echo "funding_arb_open_position_startup_ok reason=existing_position_supervision_continues open_count=${open_count} summary_open_count=${summary_open_count}"
      return 0
    fi

    if [[ "${FUNDING_ARB_AUTO_RECOVER_NONZERO_UNKNOWN}" == "1" ]]; then
      FUNDING_ARB_ALLOW_UNKNOWN_RECOVERY=1
      FUNDING_ARB_STARTUP_OPEN_POSITION_CONFIRMED=1
      echo "warning: funding-arb live startup found open resident positions; enabling foreground reduce-only exit before new entries; open_count=${open_count} summary_open_count=${summary_open_count}" >&2
      return 0
    fi

    write_funding_arb_startup_block_report "open_resident_positions" "[]" "" "" "${summary_unknown_count}" "${summary_open_count}"
    echo "error: funding-arb live startup blocked before starting background processes: open resident position state exists." >&2
    printf '%s\n' "${open_positions_json}" | jq -r '
      .[:8][]
      | "  - position_id=\(.key) pair_id=\(.value.pair_id // "unknown") symbol=\(.value.symbol // "unknown") recovered_from_unknown=\(.value.recovered_from_unknown // false)"
    ' >&2 2>> "${LOG_DIR}/jq-errors.log" || true
    echo "set BASIS_OBSERVER_FUNDING_ARB_AUTO_RECOVER_NONZERO_UNKNOWN=1 only after deciding that resident reduce-only exit is intended." >&2
    return 1
  fi

  clear_funding_arb_startup_block_report
}

funding_arb_startup_open_positions_are_supervisable() {
  local open_positions_json="$1"

  printf '%s\n' "${open_positions_json}" | jq -e '
    (type == "array")
    and (length > 0)
    and all(.[]; ((.value.recovered_from_unknown // false) == false)
      and (((.value.position_state_path // "") | tostring | length) > 0))
  ' >/dev/null
}

run_funding_arb_startup_unknown_recovery_once() {
  local snapshot_path="${SNAPSHOT_DIR}/funding-arb/funding_arb_monitor_snapshot.json"
  local log_file="${FUNDING_ARB_STARTUP_PRECHECK_DIR}/unknown-recovery-resident.log"
  local summary_path="${FUNDING_ARB_RESIDENT_OUT_DIR}/funding_arb_resident_live_summary.json"
  local unknowns_json
  local open_positions_json
  local unknown_count
  local initial_open_count
  local open_count=0
  local summary_open_count=0
  local report_open_count=0
  local -a args
  local -a hyperliquid_asset_id_args
  local asset_id_arg

  [[ "${EXECUTE_LIVE}" == "1" ]] || return 0
  open_positions_json="$(funding_arb_open_positions_json)"
  initial_open_count="$(printf '%s\n' "${open_positions_json}" | jq 'length')"
  [[ "${FUNDING_ARB_ALLOW_UNKNOWN_RECOVERY}" == "1" ]] || return 0
  if [[ "${FUNDING_ARB_STARTUP_NONZERO_UNKNOWN_CONFIRMED:-0}" != "1" && "${FUNDING_ARB_STARTUP_OPEN_POSITION_CONFIRMED:-0}" != "1" && "${initial_open_count}" == "0" ]]; then
    return 0
  fi

  if [[ ! -s "${snapshot_path}" ]]; then
    write_funding_arb_startup_block_report "missing_funding_arb_snapshot_for_unknown_recovery" "[]" "" "${snapshot_path}" 0
    echo "error: funding-arb unknown recovery requires ${snapshot_path}" >&2
    return 1
  fi

  mkdir -p "${FUNDING_ARB_STARTUP_PRECHECK_DIR}"
  args=(
    "${RUNTIME_BIN}" funding-arb-resident-live
    --snapshot "${snapshot_path}"
    --config "${CONFIG_PATH}"
    --out "${FUNDING_ARB_RESIDENT_OUT_DIR}"
    --interval-secs "${FUNDING_ARB_RESIDENT_INTERVAL_SECS}"
    --max-cycles "1"
    --notional-usd "${NOTIONAL_USD}"
    --taker-fee-bps "${PERP_FEE_BPS}"
    --slippage-bps "${SLIPPAGE_BPS}"
    --max-entry-price-divergence-bps "${FUNDING_ARB_MAX_ENTRY_PRICE_DIVERGENCE_BPS}"
    --min-net-funding-bps "${MIN_NET_BPS}"
    --private-order-events-dir "${PRIVATE_ORDER_EVENTS_DIR}"
    --execute-live
    --i-understand-funding-arb-live-orders
    --allow-unknown-recovery
    --exit-only
  )
  [[ -n "${FUNDING_SETTLEMENT_LEDGER:-}" ]] && args+=(--funding-settlement-ledger "${FUNDING_SETTLEMENT_LEDGER}")
  [[ -n "${FUNDING_SETTLEMENT_RAW_SNAPSHOT:-}" ]] && args+=(--funding-settlement-raw-snapshot "${FUNDING_SETTLEMENT_RAW_SNAPSHOT}")
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

  echo "funding_arb_startup_unknown_recovery_begin log=${log_file}"
  if ! "${args[@]}" >> "${log_file}" 2>&1; then
    write_funding_arb_startup_block_report "unknown_recovery_resident_failed" "$(funding_arb_unresolved_unknowns_json)" "" "${log_file}" 0
    echo "error: funding-arb startup unknown recovery failed; log=${log_file}" >&2
    tail -n 60 "${log_file}" >&2 || true
    return 1
  fi

  unknowns_json="$(funding_arb_unresolved_unknowns_json)"
  unknown_count="$(printf '%s\n' "${unknowns_json}" | jq 'length')"
  open_positions_json="$(funding_arb_open_positions_json)"
  open_count="$(printf '%s\n' "${open_positions_json}" | jq 'length')"
  if [[ -s "${summary_path}" ]]; then
    refresh_funding_arb_summary_position_counts "${summary_path}"
    summary_open_count="$(jq -r '(.open_position_count // 0) | tonumber' "${summary_path}" 2>> "${LOG_DIR}/jq-errors.log" || printf '0')"
  fi
  report_open_count="${open_count}"
  if (( summary_open_count > report_open_count )); then
    report_open_count="${summary_open_count}"
  fi

  if (( unknown_count == 0 && report_open_count == 0 )); then
    FUNDING_ARB_ALLOW_UNKNOWN_RECOVERY=0
    FUNDING_ARB_STARTUP_NONZERO_UNKNOWN_CONFIRMED=0
    FUNDING_ARB_STARTUP_OPEN_POSITION_CONFIRMED=0
    clear_funding_arb_startup_block_report
    echo "funding_arb_startup_unknown_recovery_ok log=${log_file}"
    return 0
  fi

  if (( unknown_count == 0 && report_open_count > 0 )) \
    && funding_arb_startup_open_positions_are_supervisable "${open_positions_json}"; then
    FUNDING_ARB_ALLOW_UNKNOWN_RECOVERY=0
    FUNDING_ARB_STARTUP_NONZERO_UNKNOWN_CONFIRMED=0
    FUNDING_ARB_STARTUP_OPEN_POSITION_CONFIRMED=0
    FUNDING_ARB_STARTUP_OPEN_POSITION_CONTINUE=1
    clear_funding_arb_startup_block_report
    echo "funding_arb_startup_open_position_supervision_continue open_count=${report_open_count} log=${log_file}"
    return 0
  fi

  write_funding_arb_startup_block_report "unknown_recovery_left_residual_state" "${unknowns_json}" "" "${log_file}" "${unknown_count}" "${report_open_count}"
  echo "error: funding-arb startup unknown recovery left residual state; unknown_count=${unknown_count} open_count=${report_open_count}; log=${log_file}" >&2
  return 1
}

run_funding_arb_startup_dry_run_precheck_for_pair() {
  local pair_id="$1"
  local snapshot_path="${2:-${SNAPSHOT_DIR}/funding-arb/funding_arb_monitor_snapshot.json}"
  local safe_pair="${pair_id//[^A-Za-z0-9_.-]/_}"
  local run_id
  local pair_dir
  local readonly_dir
  local guarded_dir
  local log_file
  local report_path
  local -a readonly_args
  local -a dry_run_args

  run_id="$(date -u +%Y%m%dT%H%M%SZ)"
  pair_dir="${FUNDING_ARB_STARTUP_PRECHECK_DIR}/${run_id}/${safe_pair}"
  readonly_dir="${pair_dir}/private-readonly"
  guarded_dir="${pair_dir}/guarded-dry-run"
  log_file="${pair_dir}/startup-precheck.log"
  mkdir -p "${pair_dir}"

  readonly_args=(
    "${RUNTIME_BIN}" funding-arb-private-readonly-snapshot-once
    --snapshot "${snapshot_path}"
    --pair-id "${pair_id}"
    --config "${CONFIG_PATH}"
    --out "${readonly_dir}"
  )
  [[ -n "${FUNDING_SETTLEMENT_RAW_SNAPSHOT:-}" ]] && readonly_args+=(--funding-settlement-raw-snapshot "${FUNDING_SETTLEMENT_RAW_SNAPSHOT}")
  [[ -n "${HYPERLIQUID_USER:-}" ]] && readonly_args+=(--hyperliquid-user "${HYPERLIQUID_USER}")
  [[ -n "${ASTER_USER:-}" ]] && readonly_args+=(--aster-user "${ASTER_USER}")
  [[ -n "${ASTER_SIGNER:-}" ]] && readonly_args+=(--aster-signer "${ASTER_SIGNER}")
  [[ -n "${ASTER_SIGNER_CMD_ENV:-}" ]] && readonly_args+=(--aster-signer-cmd-env "${ASTER_SIGNER_CMD_ENV}")

  echo "funding_arb_startup_precheck_readonly pair_id=${pair_id} out=${readonly_dir}"
  if ! "${readonly_args[@]}" >> "${log_file}" 2>&1; then
    write_funding_arb_startup_block_report "readonly_precheck_failed" "[]" "${pair_id}" "${log_file}" 0
    echo "error: funding-arb startup readonly precheck failed for pair_id=${pair_id}; log=${log_file}" >&2
    tail -n 40 "${log_file}" >&2 || true
    return 1
  fi

  dry_run_args=(
    "${RUNTIME_BIN}" funding-arb-guarded-dry-run-once
    --snapshot "${snapshot_path}"
    --pair-id "${pair_id}"
    --config "${CONFIG_PATH}"
    --out "${guarded_dir}"
    --private-account-raw-snapshot "${readonly_dir}/funding_arb_private_account_raw_snapshot.json"
    --private-position-raw-snapshot "${readonly_dir}/funding_arb_private_position_raw_snapshot.json"
    --notional-usd "${NOTIONAL_USD}"
    --taker-fee-bps "${PERP_FEE_BPS}"
    --slippage-bps "${SLIPPAGE_BPS}"
    --max-entry-price-divergence-bps "${FUNDING_ARB_MAX_ENTRY_PRICE_DIVERGENCE_BPS}"
    --min-net-funding-bps "${MIN_NET_BPS}"
  )
  if [[ -n "${FUNDING_SETTLEMENT_LEDGER:-}" ]]; then
    dry_run_args+=(--funding-settlement-ledger "${FUNDING_SETTLEMENT_LEDGER}")
  else
    dry_run_args+=(--funding-settlement-raw-snapshot "${readonly_dir}/funding_arb_funding_settlement_raw_snapshot.json")
  fi

  echo "funding_arb_startup_precheck_guarded_dry_run pair_id=${pair_id} out=${guarded_dir}"
  if ! "${dry_run_args[@]}" >> "${log_file}" 2>&1; then
    if funding_arb_guarded_dry_run_log_is_stale_candidate_skip "${log_file}"; then
      echo "funding_arb_startup_precheck_skip_stale_candidate pair_id=${pair_id} log=${log_file}"
      return 2
    fi
    write_funding_arb_startup_block_report "guarded_dry_run_failed" "[]" "${pair_id}" "${log_file}" 0
    echo "error: funding-arb startup guarded dry-run failed for pair_id=${pair_id}; log=${log_file}" >&2
    tail -n 40 "${log_file}" >&2 || true
    return 1
  fi

  report_path="${guarded_dir}/funding_arb_guarded_dry_run_report.json"
  if [[ ! -s "${report_path}" ]]; then
    write_funding_arb_startup_block_report "guarded_dry_run_report_missing" "[]" "${pair_id}" "${guarded_dir}" 0
    echo "error: funding-arb startup guarded dry-run did not write report for pair_id=${pair_id}; out=${guarded_dir}" >&2
    return 1
  fi

  if jq -e '
    (.signal_allowed == true)
    and (.live_ready == true)
    and (.dispatch_plan_built == true)
    and ((.dispatch_request_count // 0) == 2)
    and (((.live_blocking_reasons // []) | length) == 0)
  ' "${report_path}" >/dev/null 2>&1; then
    echo "funding_arb_startup_precheck_ok pair_id=${pair_id} report=${report_path}"
    return 0
  fi

  if funding_arb_guarded_dry_run_report_is_stale_candidate_skip "${report_path}"; then
    echo "funding_arb_startup_precheck_skip_stale_candidate pair_id=${pair_id} report=${report_path}"
    return 2
  fi
  echo "funding_arb_startup_precheck_skip_not_live_ready pair_id=${pair_id} report=${report_path}" >&2
  jq -c '{
    pair_id,
    symbol,
    signal_allowed,
    risk_decision,
    risk_reason_codes,
    live_ready,
    live_blocking_reasons,
    private_account_status: (.private_accounts.status // null),
    private_position_status: (.private_positions.status // null),
    execution_preflight_status: (.execution_preflight.status // null)
  }' "${report_path}" >&2 2>> "${LOG_DIR}/jq-errors.log" || true
  return 3
}

check_funding_arb_resident_startup_risk() {
  local candidate_pair_ids
  local pair_id
  local pair_count=0
  local ready_count=0
  local stale_candidate_count=0
  local not_live_ready_candidate_count=0
  local precheck_status=0
  local snapshot_path="${SNAPSHOT_DIR}/funding-arb/funding_arb_monitor_snapshot.json"
  local precheck_snapshot_path

  [[ "${EXECUTE_LIVE}" == "1" ]] || return 0
  check_funding_arb_resident_unknown_startup_risk || return 1
  if [[ "${FUNDING_ARB_STARTUP_OPEN_POSITION_CONTINUE:-0}" == "1" ]]; then
    echo "funding_arb_startup_precheck_skipped reason=existing_open_position_supervision_continues"
    return 0
  fi
  if [[ "${FUNDING_ARB_STARTUP_AUTO_RESIDUAL_DERISK:-0}" == "1" ]]; then
    echo "funding_arb_startup_precheck_skipped reason=auto_residual_de_risk"
    return 0
  fi
  if [[ "${FUNDING_ARB_ALLOW_UNKNOWN_RECOVERY}" == "1" && "${FUNDING_ARB_STARTUP_NONZERO_UNKNOWN_CONFIRMED:-0}" == "1" ]]; then
    echo "funding_arb_startup_precheck_skipped reason=resident_unknown_recovery_only"
    return 0
  fi

  if [[ "${FUNDING_ARB_STARTUP_DRY_RUN_CHECK}" != "1" ]]; then
    echo "funding_arb_startup_precheck_skipped reason=dry_run_check_disabled"
    return 0
  fi

  if [[ ! -s "${snapshot_path}" ]]; then
    write_funding_arb_startup_block_report "missing_funding_arb_snapshot" "[]" "" "${snapshot_path}" 0
    echo "error: funding-arb startup precheck requires ${snapshot_path}" >&2
    return 1
  fi

  mkdir -p "${FUNDING_ARB_STARTUP_PRECHECK_DIR}"
  precheck_snapshot_path="${FUNDING_ARB_STARTUP_PRECHECK_DIR}/startup-precheck-snapshot-$(date -u +%Y%m%dT%H%M%SZ).json"
  cp "${snapshot_path}" "${precheck_snapshot_path}"

  if ! candidate_pair_ids="$(funding_arb_current_candidate_pair_ids "${precheck_snapshot_path}")"; then
    write_funding_arb_startup_block_report "funding_arb_snapshot_candidate_parse_failed" "[]" "" "${precheck_snapshot_path}" 0
    echo "error: funding-arb startup precheck could not parse current candidate pair ids" >&2
    return 1
  fi

  while IFS= read -r pair_id; do
    [[ -n "${pair_id}" ]] || continue
    pair_count="$((pair_count + 1))"
    precheck_status=0
    run_funding_arb_startup_dry_run_precheck_for_pair "${pair_id}" "${precheck_snapshot_path}" || precheck_status="$?"
    if [[ "${precheck_status}" == "0" ]]; then
      ready_count="$((ready_count + 1))"
      continue
    fi
    if [[ "${precheck_status}" == "2" ]]; then
      stale_candidate_count="$((stale_candidate_count + 1))"
      continue
    fi
    if [[ "${precheck_status}" == "3" ]]; then
      not_live_ready_candidate_count="$((not_live_ready_candidate_count + 1))"
      continue
    fi
    return 1
  done <<< "${candidate_pair_ids}"

  if (( pair_count == 0 )); then
    clear_funding_arb_startup_block_report
    echo "funding_arb_startup_precheck_ok reason=no_current_funding_arb_candidates"
    return 0
  elif (( ready_count > 0 )); then
    clear_funding_arb_startup_block_report
    echo "funding_arb_startup_precheck_ok reason=current_candidate_live_ready ready_candidate_count=${ready_count} stale_candidate_count=${stale_candidate_count} not_live_ready_candidate_count=${not_live_ready_candidate_count}"
    return 0
  elif (( ready_count == 0 && stale_candidate_count > 0 )); then
    clear_funding_arb_startup_block_report
    echo "funding_arb_startup_precheck_ok reason=all_current_candidates_stale_after_revalidation stale_candidate_count=${stale_candidate_count} not_live_ready_candidate_count=${not_live_ready_candidate_count}"
    return 0
  elif (( ready_count == 0 && not_live_ready_candidate_count > 0 )); then
    clear_funding_arb_startup_block_report
    echo "funding_arb_startup_precheck_ok reason=current_candidates_not_live_ready not_live_ready_candidate_count=${not_live_ready_candidate_count} stale_candidate_count=${stale_candidate_count}"
    return 0
  fi

  clear_funding_arb_startup_block_report
  echo "funding_arb_startup_precheck_ok reason=no_startup_blocking_candidate"
  return 0
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
  if should_emit_health_event "spot-perp-basis:${venue}" "poll_ok|status=${snapshot_status}|candidate_count=${count}|total_rows=${total_rows}"; then
    append_json_line_from_body "${HEALTH_EVENTS_JSONL}" "${body}" \
      --arg recorded_at "${ts}" \
      --arg venue "${venue}" \
      --arg endpoint "${url}" \
      --arg status "${snapshot_status}" \
      --arg updated_at "${updated_at}" \
      --argjson candidate_count "${count}" \
      --argjson total_rows "${total_rows}" \
      '
      def bounded_string:
        if type == "string" and length > 500 then .[0:500] + "...<truncated>" else . end;
      def bounded_value:
        if type == "object" then with_entries(.value |= bounded_string)
        elif type == "string" then bounded_string
        else .
        end;
      def blocking_path_metadata:
        (.blocking_path // []) as $raw
        | (if ($raw | type) == "array" then $raw else [] end) as $path
        | {
            entries: ($path[0:$blocking_path_limit] | map(bounded_value)),
            total_count: ($path | length),
            truncated: (($path | length) > $blocking_path_limit)
          };
      (blocking_path_metadata) as $blocking_path_metadata
      | {recorded_at:$recorded_at,venue:$venue,strategy:"spot-perp-basis",event:"poll_ok",endpoint:$endpoint,status:$status,updated_at:$updated_at,candidate_count:$candidate_count,total_rows:$total_rows,blocking_path:$blocking_path_metadata.entries,blocking_path_total_count:$blocking_path_metadata.total_count,blocking_path_truncated:$blocking_path_metadata.truncated,mutable_execution_started:false}'
  fi

  if (( count > 0 )); then
    append_json_body_line "${OPPORTUNITY_DIR}/${venue}-opportunities.jsonl" "${body}" \
      --arg recorded_at "${ts}" \
      --arg venue "${venue}" \
      --arg endpoint "${url}" \
      '. + {recorded_at:$recorded_at,venue:$venue,endpoint:$endpoint,mutable_execution_started:false}'
    append_json_body_line "${OPPORTUNITY_DIR}/spot-perp-basis.jsonl" "${body}" \
      --arg recorded_at "${ts}" \
      --arg venue "${venue}" \
      --arg endpoint "${url}" \
      '. + {recorded_at:$recorded_at,venue:$venue,endpoint:$endpoint,mutable_execution_started:false}'
    append_json_body_line "${OPPORTUNITY_DIR}/all-opportunities.jsonl" "${body}" \
      --arg recorded_at "${ts}" \
      --arg venue "${venue}" \
      --arg endpoint "${url}" \
      '. + {recorded_at:$recorded_at,venue:$venue,endpoint:$endpoint,mutable_execution_started:false}'

    summary="$(printf '%s\n' "${body}" | jq -r '[.rows[]? | "\(.symbol):net=\(.net_basis_bps // "null")bps profit=\(.expected_profit_usd // "null")"] | join(", ")')"
    append_text_line "${FEEDBACK_LOG}" "[$(local_log_timestamp)] opportunity venue=${venue} candidate_count=${count} ${summary}"
    start_target_wss_warmups_for_candidates "${venue}" "${body}" "${ts}"
  else
    summary="$(printf '%s\n' "${body}" | jq -r '(.blocking_path // []) | .[0:6] | map("\(.blocker):\(.reason)") | join(" | ")' 2>> "${LOG_DIR}/jq-errors.log" || true)"
    if [[ -z "${summary}" ]]; then
      summary="blocking_path=[]"
    fi
    append_text_line "${FEEDBACK_LOG}" "[$(local_log_timestamp)] opportunity venue=${venue} strategy=spot-perp-basis candidate_count=0 ${summary}"
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
  if should_emit_health_event "cross-exchange-funding-arb" "poll_ok|status=${snapshot_status}|candidate_count=${count}|total_rows=${total_rows}"; then
    append_json_line_from_body "${HEALTH_EVENTS_JSONL}" "${body}" \
      --arg recorded_at "${ts}" \
      --arg strategy "cross-exchange-funding-arb" \
      --arg endpoint "${url}" \
      --arg status "${snapshot_status}" \
      --arg updated_at "${updated_at}" \
      --argjson candidate_count "${count}" \
      --argjson total_rows "${total_rows}" \
      '
      def bounded_string:
        if type == "string" and length > 500 then .[0:500] + "...<truncated>" else . end;
      def bounded_value:
        if type == "object" then with_entries(.value |= bounded_string)
        elif type == "string" then bounded_string
        else .
        end;
      def blocking_path_metadata:
        (.blocking_path // []) as $raw
        | (if ($raw | type) == "array" then $raw else [] end) as $path
        | {
            entries: ($path[0:$blocking_path_limit] | map(bounded_value)),
            total_count: ($path | length),
            truncated: (($path | length) > $blocking_path_limit)
          };
      (blocking_path_metadata) as $blocking_path_metadata
      | {recorded_at:$recorded_at,strategy:$strategy,event:"poll_ok",endpoint:$endpoint,status:$status,updated_at:$updated_at,candidate_count:$candidate_count,total_rows:$total_rows,blocking_path:$blocking_path_metadata.entries,blocking_path_total_count:$blocking_path_metadata.total_count,blocking_path_truncated:$blocking_path_metadata.truncated,mutable_execution_started:false}'
  fi

  if (( count > 0 )); then
    append_json_body_line "${OPPORTUNITY_DIR}/cross-exchange-funding-arb.jsonl" "${body}" \
      --arg recorded_at "${ts}" \
      --arg strategy "cross-exchange-funding-arb" \
      --arg endpoint "${url}" \
      '. + {recorded_at:$recorded_at,strategy:$strategy,endpoint:$endpoint,mutable_execution_started:false}'
    append_json_body_line "${OPPORTUNITY_DIR}/all-opportunities.jsonl" "${body}" \
      --arg recorded_at "${ts}" \
      --arg strategy "cross-exchange-funding-arb" \
      --arg endpoint "${url}" \
      '. + {recorded_at:$recorded_at,strategy:$strategy,endpoint:$endpoint,mutable_execution_started:false}'

    summary="$(printf '%s\n' "${body}" | jq -r '[.rows[]? | "\(.pair_id):net=\(.net_funding_bps // "null")bps funding=\(.expected_funding_usd // "null")"] | join(", ")')"
    append_text_line "${FEEDBACK_LOG}" "[$(local_log_timestamp)] opportunity strategy=cross-exchange-funding-arb candidate_count=${count} ${summary}"
  else
    summary="$(printf '%s\n' "${body}" | jq -r '(.blocking_path // []) | .[0:6] | map("\(.blocker):\(.reason)") | join(" | ")' 2>> "${LOG_DIR}/jq-errors.log" || true)"
    if [[ -z "${summary}" ]]; then
      summary="blocking_path=[]"
    fi
    append_text_line "${FEEDBACK_LOG}" "[$(local_log_timestamp)] opportunity strategy=cross-exchange-funding-arb candidate_count=0 ${summary}"
  fi
}

count_matching_lines() {
  local pattern="$1"
  local text="$2"
  local count
  count="$(grep -c -- "${pattern}" <<< "${text}" || true)"
  printf '%s' "${count:-0}"
}

append_resident_tail_health() {
  local strategy="$1"
  local source="$2"
  local event_file="$3"
  local ts
  local window
  local cycle_errors
  local blocked_decisions
  local capacity_blocks
  local scoped_window
  local status="healthy"
  local key

  [[ "${RESIDENT_HEALTH_TAIL_LINES:-200}" =~ ^[0-9]+$ ]] || return 0
  (( RESIDENT_HEALTH_TAIL_LINES > 0 )) || return 0
  [[ -f "${event_file}" ]] || return 0

  window="$(tail -n "${RESIDENT_HEALTH_TAIL_LINES}" "${event_file}" 2>/dev/null || true)"
  scoped_window="${window}"
  if grep -q '"event_type":"resident_started"' <<< "${window}"; then
    scoped_window="$(awk '
      /"event_type":"resident_started"/ { buffer=$0 ORS; next }
      buffer != "" { buffer=buffer $0 ORS }
      END { printf "%s", buffer }
    ' <<< "${window}")"
  fi
  cycle_errors="$(count_matching_lines '"event_type":"cycle_error"' "${scoped_window}")"
  blocked_decisions="$(count_matching_lines '"decision":"blocked"' "${scoped_window}")"
  capacity_blocks="$(count_matching_lines '"event_type":"entry_capacity_blocked"' "${scoped_window}")"
  if (( cycle_errors > 0 || blocked_decisions > 0 || capacity_blocks > 0 )); then
    status="warning"
  fi
  key="resident_tail_health|status=${status}|cycle_errors=${cycle_errors}|blocked=${blocked_decisions}|capacity=${capacity_blocks}"
  should_emit_health_event "resident:${source}" "${key}" || return 0

  ts="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
  append_json_line "${HEALTH_EVENTS_JSONL}" \
    --arg recorded_at "${ts}" \
    --arg strategy "${strategy}" \
    --arg source "${source}" \
    --arg status "${status}" \
    --arg event_file "${event_file}" \
    --argjson tail_lines "${RESIDENT_HEALTH_TAIL_LINES}" \
    --argjson cycle_error_count "${cycle_errors}" \
    --argjson blocked_decision_count "${blocked_decisions}" \
    --argjson entry_capacity_blocked_count "${capacity_blocks}" \
    '{recorded_at:$recorded_at,strategy:$strategy,source:$source,event:"resident_tail_health",status:$status,event_file:$event_file,tail_lines:$tail_lines,cycle_error_count:$cycle_error_count,blocked_decision_count:$blocked_decision_count,entry_capacity_blocked_count:$entry_capacity_blocked_count,mutable_execution_started:false}'
}

append_resident_health_events() {
  local basis_dir="${BASIS_RESIDENT_OUT_DIR:-${RUN_ROOT}/resident-live/spot-perp-basis}"
  local funding_dir="${FUNDING_ARB_RESIDENT_OUT_DIR:-${RUN_ROOT}/resident-live/cross-exchange-funding-arb}"
  local venue

  append_resident_tail_health \
    "spot-perp-basis" \
    "spot-perp-basis:multi-venue" \
    "${basis_dir}/multi_venue_resident_live_events.jsonl"
  for venue in binance bybit okx bitget; do
    append_resident_tail_health \
      "spot-perp-basis" \
      "spot-perp-basis:${venue}" \
      "${basis_dir}/${venue}/resident_live_events.jsonl"
  done
  append_resident_tail_health \
    "cross-exchange-funding-arb" \
    "cross-exchange-funding-arb" \
    "${funding_dir}/funding_arb_resident_live_events.jsonl"
}

rust_recorder_disabled_reason() {
  case "${RUST_RECORDER_ENABLED:-1}" in
    1) ;;
    0) printf 'disabled_by_env'; return 0 ;;
    *) printf 'invalid_env'; return 0 ;;
  esac
  if strategy_enabled "spot-perp-basis" && [[ "${SPOT_PERP_BASIS_MODE}" != "resident" ]]; then
    printf 'spot_perp_basis_mode_%s' "${SPOT_PERP_BASIS_MODE}"
    return 0
  fi
  if strategy_enabled "cross-exchange-funding-arb" && [[ "${FUNDING_ARB_MODE}" != "resident" ]]; then
    printf 'funding_arb_mode_%s' "${FUNDING_ARB_MODE}"
    return 0
  fi
  if [[ "${DYNAMIC_TARGET_WSS}" != "0" ]]; then
    printf 'dynamic_target_wss_enabled'
    return 0
  fi
  return 1
}

run_rust_recorder() {
  local -a args
  local venue

  args=(
    "${RUNTIME_BIN}" opportunity-recorder
    --root "${RUN_ROOT}"
    --opportunity-dir "${OPPORTUNITY_DIR}"
    --logs-dir "${LOG_DIR}"
    --basis-resident-out "${BASIS_RESIDENT_OUT_DIR}"
    --funding-arb-resident-out "${FUNDING_ARB_RESIDENT_OUT_DIR}"
    --strategies "${EFFECTIVE_STRATEGIES}"
    --execution-mode "${EXECUTION_MODE}"
    --spot-perp-basis-mode "${SPOT_PERP_BASIS_MODE}"
    --funding-arb-mode "${FUNDING_ARB_MODE}"
    --interval-secs "${INTERVAL_SECS}"
    --timeout-secs "${CURL_TIMEOUT_SECS}"
    --retries "${CURL_RETRIES}"
    --retry-sleep-secs "${CURL_RETRY_SLEEP_SECS}"
    --blocking-path-event-limit "${BLOCKING_PATH_EVENT_LIMIT}"
    --health-sample-secs "${HEALTH_EVENT_SAMPLE_SECS}"
    --resident-health-tail-lines "${RESIDENT_HEALTH_TAIL_LINES}"
    --clear-sources
  )
  for venue in "${RECORDER_MONITORS[@]}"; do
    args+=(--source "${venue}=$(opportunities_url "${venue}")")
  done
  if strategy_enabled "cross-exchange-funding-arb"; then
    args+=(--funding-arb-url "$(funding_arb_opportunities_url)")
  else
    args+=(--no-funding-arb-url)
  fi

  # 中文说明：用 exec 让 pid 文件记录的 --recorder 包装进程变成真正的 Rust
  # recorder，停止脚本才能直接终止长期运行的子命令。
  exec env ARB_RUNTIME_ENABLE_LEGACY_COMMANDS=1 "${args[@]}"
}

run_recorder() {
  cd "${REPO_ROOT}"
  trap 'append_text_line "${FEEDBACK_LOG}" "[$(local_log_timestamp)] recorder_stop"; exit 0' INT TERM
  mkdir -p "${LOG_DIR}" "${STATE_DIR}" "${OPPORTUNITY_DIR}" "${EXECUTION_DIR}" "${SNAPSHOT_DIR}" "${PRIVATE_ORDER_EVENTS_DIR}" "${TARGET_WSS_LOG_DIR}" "${TARGET_WSS_STATE_DIR}"
  touch "${OPPORTUNITY_DIR}/all-opportunities.jsonl" "${OPPORTUNITY_DIR}/spot-perp-basis.jsonl" "${OPPORTUNITY_DIR}/cross-exchange-funding-arb.jsonl"
  touch "${VALIDATION_EVENTS_JSONL}" "${EXECUTION_REPORTS_JSONL}"
  IFS=' ' read -r -a RECORDER_MONITORS <<< "${EFFECTIVE_MONITORS}"
  local rust_recorder_reason=""
  if rust_recorder_reason="$(rust_recorder_disabled_reason)"; then
    append_text_line "${FEEDBACK_LOG}" "[$(local_log_timestamp)] recorder_impl=shell reason=${rust_recorder_reason}"
  else
    run_rust_recorder
    return "$?"
  fi
  append_text_line "${FEEDBACK_LOG}" "[$(local_log_timestamp)] recorder_start mode=${EXECUTION_MODE} spot_perp_basis_mode=${SPOT_PERP_BASIS_MODE} funding_arb_mode=${FUNDING_ARB_MODE} monitors=${EFFECTIVE_MONITORS} strategies=${EFFECTIVE_STRATEGIES} interval_secs=${INTERVAL_SECS} min_net_bps=${MIN_NET_BPS}"
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
    append_resident_health_events
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
  EXECUTE_LIVE="${BASIS_OBSERVER_EXECUTE_LIVE:-${ARB_RUNTIME_LIVE_AUTO_ORDER_ENABLED:-0}}"
  LIVE_ACK="${BASIS_OBSERVER_LIVE_ACK:-0}"
  case "${EXECUTE_LIVE}" in
    0|1) ;;
    *) die "BASIS_OBSERVER_EXECUTE_LIVE or ARB_RUNTIME_LIVE_AUTO_ORDER_ENABLED must be 0 or 1" ;;
  esac
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
  BLOCKING_PATH_EVENT_LIMIT="${BASIS_OBSERVER_BLOCKING_PATH_EVENT_LIMIT:-12}"
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
  SPOT_PERP_BASIS_MODE="resident"
  BASIS_RESIDENT_OUT_DIR="${BASIS_OBSERVER_BASIS_RESIDENT_OUT:-${RUN_ROOT}/resident-live/spot-perp-basis}"
  FUNDING_ARB_MODE="resident"
  FUNDING_ARB_RESIDENT_OUT_DIR="${BASIS_OBSERVER_FUNDING_ARB_RESIDENT_OUT:-${RUN_ROOT}/resident-live/cross-exchange-funding-arb}"
  HEALTH_EVENT_SAMPLE_SECS="${BASIS_OBSERVER_HEALTH_EVENT_SAMPLE_SECS:-300}"
  RESIDENT_HEALTH_TAIL_LINES="${BASIS_OBSERVER_RESIDENT_HEALTH_TAIL_LINES:-200}"
  LOG_ROTATE_BYTES="${BASIS_OBSERVER_LOG_ROTATE_BYTES:-134217728}"
  LOG_ROTATE_KEEP="${BASIS_OBSERVER_LOG_ROTATE_KEEP:-4}"
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
  validate_blocking_path_event_limit "${BLOCKING_PATH_EVENT_LIMIT}"
  validate_health_sampling_settings
  validate_log_rotation_settings
  export ARB_RUNTIME_JSONL_ROTATE_BYTES="${ARB_RUNTIME_JSONL_ROTATE_BYTES:-${LOG_ROTATE_BYTES}}"
  export ARB_RUNTIME_JSONL_ROTATE_KEEP="${ARB_RUNTIME_JSONL_ROTATE_KEEP:-${LOG_ROTATE_KEEP}}"
  export ARB_RUNTIME_BINANCE_RECV_WINDOW_MS="${ARB_RUNTIME_BINANCE_RECV_WINDOW_MS:-30000}"
  export ARB_RUNTIME_BYBIT_RECV_WINDOW_MS="${ARB_RUNTIME_BYBIT_RECV_WINDOW_MS:-30000}"
  CURL_TIMEOUT_SECS="${BASIS_OBSERVER_CURL_TIMEOUT_SECS:-10}"
  CURL_RETRIES="${BASIS_OBSERVER_CURL_RETRIES:-3}"
  CURL_RETRY_SLEEP_SECS="${BASIS_OBSERVER_CURL_RETRY_SLEEP_SECS:-1}"
  RUST_RECORDER_ENABLED="${BASIS_OBSERVER_RUST_RECORDER_ENABLED:-1}"
  EFFECTIVE_MONITORS="${BASIS_OBSERVER_EFFECTIVE_MONITORS:-binance bybit okx bitget aster hyperliquid}"
  EFFECTIVE_STRATEGIES="${BASIS_OBSERVER_EFFECTIVE_STRATEGIES:-spot-perp-basis,cross-exchange-funding-arb}"
  BINANCE_BIND="${BINANCE_BASIS_BIND:-127.0.0.1:8796}"
  BYBIT_BIND="${BYBIT_BASIS_BIND:-127.0.0.1:8797}"
  OKX_BIND="${OKX_BASIS_BIND:-127.0.0.1:8798}"
  BITGET_BIND="${BITGET_BASIS_BIND:-127.0.0.1:8803}"
  ASTER_BIND="${ASTER_BASIS_BIND:-127.0.0.1:8800}"
  HYPERLIQUID_BIND="${HYPERLIQUID_BASIS_BIND:-127.0.0.1:8799}"
  FUNDING_ARB_BIND="${FUNDING_ARB_BIND:-127.0.0.1:8804}"
  case "${RUST_RECORDER_ENABLED}" in
    0|1) ;;
    *) die "BASIS_OBSERVER_RUST_RECORDER_ENABLED must be 0 or 1" ;;
  esac
  run_recorder
  exit 0
fi

if [[ "${1:-}" == "-h" || "${1:-}" == "--help" ]]; then
  usage
  exit 0
fi

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
EXECUTE_LIVE="${CLI_EXECUTE_LIVE:-${BASIS_OBSERVER_EXECUTE_LIVE:-${ARB_RUNTIME_LIVE_AUTO_ORDER_ENABLED:-0}}}"
LIVE_PAUSE_FILE="${BASIS_OBSERVER_LIVE_PAUSE_FILE:-${RUN_ROOT}/LIVE_TRADING_PAUSED}"
LIVE_ACK="${BASIS_OBSERVER_LIVE_ACK:-0}"
case "${EXECUTE_LIVE}" in
  0|1) ;;
  *) die "BASIS_OBSERVER_EXECUTE_LIVE or ARB_RUNTIME_LIVE_AUTO_ORDER_ENABLED must be 0 or 1" ;;
esac
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
OBSERVER_BUILD="${BASIS_OBSERVER_BUILD:-1}"
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
SPOT_PERP_BASIS_MODE="resident"
BASIS_RESIDENT_INTERVAL_SECS="${BASIS_OBSERVER_BASIS_RESIDENT_INTERVAL_SECS:-60}"
BASIS_RESIDENT_MAX_CONCURRENT_POSITIONS="${BASIS_OBSERVER_BASIS_RESIDENT_MAX_CONCURRENT_POSITIONS:-1}"
BASIS_RESIDENT_MAX_TOTAL_NOTIONAL_USDT="${BASIS_OBSERVER_BASIS_RESIDENT_MAX_TOTAL_NOTIONAL_USDT:-10.00}"
BASIS_RESIDENT_MAX_CYCLES="${BASIS_OBSERVER_BASIS_RESIDENT_MAX_CYCLES:-}"
BASIS_RESIDENT_OUT_DIR="${BASIS_OBSERVER_BASIS_RESIDENT_OUT:-${RUN_ROOT}/resident-live/spot-perp-basis}"
BASIS_RESIDENT_ADL_EVENTS_DIR="${BASIS_OBSERVER_BASIS_RESIDENT_ADL_EVENTS_DIR:-}"
FUNDING_ARB_MODE="resident"
FUNDING_ARB_RESIDENT_INTERVAL_SECS="${BASIS_OBSERVER_FUNDING_ARB_RESIDENT_INTERVAL_SECS:-60}"
FUNDING_ARB_RESIDENT_MAX_CYCLES="${BASIS_OBSERVER_FUNDING_ARB_RESIDENT_MAX_CYCLES:-}"
FUNDING_ARB_RESIDENT_OUT_DIR="${BASIS_OBSERVER_FUNDING_ARB_RESIDENT_OUT:-${RUN_ROOT}/resident-live/cross-exchange-funding-arb}"
FUNDING_ARB_AUTO_RESIDUAL_DE_RISK="${BASIS_OBSERVER_FUNDING_ARB_AUTO_RESIDUAL_DE_RISK:-1}"
FUNDING_ARB_ALLOW_UNKNOWN_RECOVERY="${BASIS_OBSERVER_FUNDING_ARB_ALLOW_UNKNOWN_RECOVERY:-0}"
FUNDING_ARB_STARTUP_UNKNOWN_READONLY_RECONCILE="${BASIS_OBSERVER_FUNDING_ARB_STARTUP_UNKNOWN_READONLY_RECONCILE:-1}"
FUNDING_ARB_AUTO_RECOVER_NONZERO_UNKNOWN="${BASIS_OBSERVER_FUNDING_ARB_AUTO_RECOVER_NONZERO_UNKNOWN:-1}"
FUNDING_ARB_STARTUP_DRY_RUN_CHECK="${BASIS_OBSERVER_FUNDING_ARB_STARTUP_DRY_RUN_CHECK:-1}"
FUNDING_ARB_STARTUP_DRY_RUN_LIMIT="${BASIS_OBSERVER_FUNDING_ARB_STARTUP_DRY_RUN_LIMIT:-3}"
FUNDING_ARB_STARTUP_PRECHECK_DIR="${BASIS_OBSERVER_FUNDING_ARB_STARTUP_PRECHECK_OUT:-${RUN_ROOT}/startup-precheck/cross-exchange-funding-arb}"
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
CURL_TIMEOUT_SECS="${BASIS_OBSERVER_CURL_TIMEOUT_SECS:-10}"
CURL_RETRIES="${BASIS_OBSERVER_CURL_RETRIES:-3}"
CURL_RETRY_SLEEP_SECS="${BASIS_OBSERVER_CURL_RETRY_SLEEP_SECS:-1}"
RUST_RECORDER_ENABLED="${BASIS_OBSERVER_RUST_RECORDER_ENABLED:-1}"
BLOCKING_PATH_EVENT_LIMIT="${BASIS_OBSERVER_BLOCKING_PATH_EVENT_LIMIT:-12}"
HEALTH_EVENT_SAMPLE_SECS="${BASIS_OBSERVER_HEALTH_EVENT_SAMPLE_SECS:-300}"
RESIDENT_HEALTH_TAIL_LINES="${BASIS_OBSERVER_RESIDENT_HEALTH_TAIL_LINES:-200}"
LOG_ROTATE_BYTES="${BASIS_OBSERVER_LOG_ROTATE_BYTES:-134217728}"
LOG_ROTATE_KEEP="${BASIS_OBSERVER_LOG_ROTATE_KEEP:-4}"
STARTUP_CHECK="${BASIS_OBSERVER_STARTUP_CHECK:-1}"
STARTUP_WAIT_SECS="${BASIS_OBSERVER_STARTUP_WAIT_SECS:-180}"
STOP_DRAIN_SECS="${BASIS_OBSERVER_STOP_DRAIN_SECS:-15}"
STOP_GRACE_SECS="${BASIS_OBSERVER_STOP_GRACE_SECS:-3}"
RECLAIM_STALE_MONITOR_PORTS="${BASIS_OBSERVER_RECLAIM_STALE_MONITOR_PORTS:-1}"
FOREGROUND="${BASIS_OBSERVER_FOREGROUND:-0}"
if [[ -n "${CLI_STRATEGIES}" ]]; then
  STRATEGIES="${CLI_STRATEGIES}"
elif [[ -n "${BASIS_OBSERVER_STRATEGIES:-}" ]]; then
  STRATEGIES="${BASIS_OBSERVER_STRATEGIES}"
else
  STRATEGIES="spot-perp-basis,cross-exchange-funding-arb"
fi

case "${FUNDING_ARB_AUTO_RESIDUAL_DE_RISK}" in
  0|1) ;;
  *) die "BASIS_OBSERVER_FUNDING_ARB_AUTO_RESIDUAL_DE_RISK must be 0 or 1" ;;
esac

case "${FUNDING_ARB_ALLOW_UNKNOWN_RECOVERY}" in
  0|1) ;;
  *) die "BASIS_OBSERVER_FUNDING_ARB_ALLOW_UNKNOWN_RECOVERY must be 0 or 1" ;;
esac

case "${FUNDING_ARB_STARTUP_UNKNOWN_READONLY_RECONCILE}" in
  0|1) ;;
  *) die "BASIS_OBSERVER_FUNDING_ARB_STARTUP_UNKNOWN_READONLY_RECONCILE must be 0 or 1" ;;
esac

case "${FUNDING_ARB_AUTO_RECOVER_NONZERO_UNKNOWN}" in
  0|1) ;;
  *) die "BASIS_OBSERVER_FUNDING_ARB_AUTO_RECOVER_NONZERO_UNKNOWN must be 0 or 1" ;;
esac

case "${FUNDING_ARB_STARTUP_DRY_RUN_CHECK}" in
  0|1) ;;
  *) die "BASIS_OBSERVER_FUNDING_ARB_STARTUP_DRY_RUN_CHECK must be 0 or 1" ;;
esac

if ! [[ "${FUNDING_ARB_STARTUP_DRY_RUN_LIMIT}" =~ ^[0-9]+$ ]] || (( FUNDING_ARB_STARTUP_DRY_RUN_LIMIT < 1 )); then
  die "BASIS_OBSERVER_FUNDING_ARB_STARTUP_DRY_RUN_LIMIT must be a positive integer"
fi

if [[ -n "${FUNDING_SETTLEMENT_LEDGER}" && -n "${FUNDING_SETTLEMENT_RAW_SNAPSHOT}" ]]; then
  die "cannot combine BASIS_OBSERVER_FUNDING_SETTLEMENT_LEDGER and BASIS_OBSERVER_FUNDING_SETTLEMENT_RAW_SNAPSHOT"
fi

case "${DYNAMIC_TARGET_WSS}" in
  0|1) ;;
  *) die "BASIS_OBSERVER_DYNAMIC_TARGET_WSS must be 0 or 1" ;;
esac
case "${RUST_RECORDER_ENABLED}" in
  0|1) ;;
  *) die "BASIS_OBSERVER_RUST_RECORDER_ENABLED must be 0 or 1" ;;
esac
case "${OBSERVER_BUILD}" in
  0|1) ;;
  *) die "BASIS_OBSERVER_BUILD must be 0 or 1" ;;
esac
if [[ "${OBSERVER_BUILD}" == "1" ]]; then
  require_command cargo
fi
[[ "${TARGET_WSS_BASE_PORT}" =~ ^[0-9]+$ ]] || die "BASIS_OBSERVER_TARGET_WSS_BASE_PORT must be numeric"
[[ "${TARGET_WSS_READY_TIMEOUT_SECS}" =~ ^[0-9]+$ ]] || die "BASIS_OBSERVER_TARGET_WSS_READY_TIMEOUT_SECS must be numeric"
[[ "${TARGET_WSS_RECONNECT_DELAY_SECS}" =~ ^[0-9]+$ ]] || die "BASIS_OBSERVER_TARGET_WSS_RECONNECT_DELAY_SECS must be numeric"
validate_blocking_path_event_limit "${BLOCKING_PATH_EVENT_LIMIT}"
validate_health_sampling_settings
validate_log_rotation_settings
export ARB_RUNTIME_JSONL_ROTATE_BYTES="${ARB_RUNTIME_JSONL_ROTATE_BYTES:-${LOG_ROTATE_BYTES}}"
export ARB_RUNTIME_JSONL_ROTATE_KEEP="${ARB_RUNTIME_JSONL_ROTATE_KEEP:-${LOG_ROTATE_KEEP}}"
export ARB_RUNTIME_BINANCE_RECV_WINDOW_MS="${ARB_RUNTIME_BINANCE_RECV_WINDOW_MS:-30000}"
export ARB_RUNTIME_BYBIT_RECV_WINDOW_MS="${ARB_RUNTIME_BYBIT_RECV_WINDOW_MS:-30000}"
if [[ -z "${ARB_RUNTIME_FUNDING_ARB_DIRECT_PUBLIC_SOURCES_ENABLED:-}" ]]; then
  if strategy_enabled "cross-exchange-funding-arb" && ! strategy_enabled "spot-perp-basis"; then
    ARB_RUNTIME_FUNDING_ARB_DIRECT_PUBLIC_SOURCES_ENABLED=1
  else
    ARB_RUNTIME_FUNDING_ARB_DIRECT_PUBLIC_SOURCES_ENABLED=0
  fi
fi
export ARB_RUNTIME_FUNDING_ARB_DIRECT_PUBLIC_SOURCES_ENABLED

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
cleanup_safe_stale_resident_locks

: > "${PID_FILE}"

cd "${REPO_ROOT}"
if [[ "${OBSERVER_BUILD}" == "1" ]]; then
  echo "building arb-runtime with live-exec feature..."
  cargo build -p arb-runtime --features live-exec --manifest-path "${REPO_ROOT}/Cargo.toml"
  echo "building arb-wallet-signer..."
  cargo build -p arb-wallet-signer --manifest-path "${REPO_ROOT}/Cargo.toml"
fi

if strategy_enabled "cross-exchange-funding-arb" && [[ "${FUNDING_ARB_MODE}" == "resident" ]]; then
  if ! check_funding_arb_resident_unknown_startup_risk; then
    rm -f "${PID_FILE}"
    exit 1
  fi
  if ! run_funding_arb_startup_unknown_recovery_once; then
    rm -f "${PID_FILE}"
    exit 1
  fi
fi

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

  reclaim_stale_monitor_port "${venue}-basis-monitor" "${bind_addr}"
  echo "starting ${venue} monitor: $(status_url "${venue}")"
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
  local asset_id_arg
  local -a source_args
  local -a exchange_pnl_args
  local -a hyperliquid_asset_id_args

  source_args=(--clear-sources)
  for monitor in "${MONITORS[@]}"; do
    source="$(status_url "${monitor}")"
    source_args+=(--source "${monitor}=${source}")
  done
  exchange_pnl_args=(--exchange-pnl-config "${CONFIG_PATH}")
  [[ -n "${HYPERLIQUID_USER:-}" ]] && exchange_pnl_args+=(--exchange-pnl-hyperliquid-user "${HYPERLIQUID_USER}")
  [[ -n "${HYPERLIQUID_SOURCE:-}" ]] && exchange_pnl_args+=(--exchange-pnl-hyperliquid-source "${HYPERLIQUID_SOURCE}")
  [[ -n "${HYPERLIQUID_VAULT_ADDRESS:-}" ]] && exchange_pnl_args+=(--exchange-pnl-hyperliquid-vault-address "${HYPERLIQUID_VAULT_ADDRESS}")
  [[ -n "${HYPERLIQUID_EXPIRES_AFTER_MS:-}" ]] && exchange_pnl_args+=(--exchange-pnl-hyperliquid-expires-after-ms "${HYPERLIQUID_EXPIRES_AFTER_MS}")
  if [[ -n "${HYPERLIQUID_ASSET_IDS:-}" ]]; then
    IFS=',' read -r -a hyperliquid_asset_id_args <<< "${HYPERLIQUID_ASSET_IDS}"
    for asset_id_arg in "${hyperliquid_asset_id_args[@]}"; do
      asset_id_arg="${asset_id_arg//[[:space:]]/}"
      [[ -n "${asset_id_arg}" ]] && exchange_pnl_args+=(--exchange-pnl-hyperliquid-asset-id "${asset_id_arg}")
    done
  fi
  [[ -n "${ASTER_USER:-}" ]] && exchange_pnl_args+=(--exchange-pnl-aster-user "${ASTER_USER}")
  [[ -n "${ASTER_SIGNER:-}" ]] && exchange_pnl_args+=(--exchange-pnl-aster-signer "${ASTER_SIGNER}")
  [[ -n "${ASTER_SIGNER_CMD_ENV:-}" ]] && exchange_pnl_args+=(--exchange-pnl-aster-signer-cmd-env "${ASTER_SIGNER_CMD_ENV}")

  reclaim_stale_monitor_port "funding-arb-monitor" "${FUNDING_ARB_BIND}"
  echo "starting funding arb monitor: http://${FUNDING_ARB_BIND}/api/funding-arb/status"
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
    --opportunity-history "${OPPORTUNITY_DIR}/cross-exchange-funding-arb.jsonl" \
    "${source_args[@]}" \
    "${exchange_pnl_args[@]}" \
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
    [[ "${FUNDING_ARB_ALLOW_UNKNOWN_RECOVERY}" == "1" ]] && args+=(--allow-unknown-recovery)
    [[ "${FUNDING_ARB_AUTO_RESIDUAL_DE_RISK}" == "0" ]] && args+=(--disable-auto-residual-de-risk)
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

if strategy_enabled "spot-perp-basis" || [[ "${ARB_RUNTIME_FUNDING_ARB_DIRECT_PUBLIC_SOURCES_ENABLED}" != "1" ]]; then
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
else
  echo "basis monitors skipped: funding-arb direct public sources enabled and spot-perp-basis strategy disabled"
fi

if strategy_enabled "cross-exchange-funding-arb"; then
  start_funding_arb_monitor
fi

if [[ "${STARTUP_CHECK}" == "1" ]]; then
  echo "checking /opportunities endpoints before starting recorder..."
  if strategy_enabled "spot-perp-basis" || [[ "${ARB_RUNTIME_FUNDING_ARB_DIRECT_PUBLIC_SOURCES_ENABLED}" != "1" ]]; then
    for monitor in "${MONITORS[@]}"; do
      if ! wait_for_monitor_opportunities "${monitor}"; then
        stop_started_processes
        rm -f "${PID_FILE}"
        exit 1
      fi
    done
  fi
  if strategy_enabled "cross-exchange-funding-arb"; then
    if ! wait_for_funding_arb_opportunities; then
      stop_started_processes
      rm -f "${PID_FILE}"
      exit 1
    fi
  fi
fi

if strategy_enabled "cross-exchange-funding-arb" && [[ "${FUNDING_ARB_MODE}" == "resident" ]]; then
  if ! check_funding_arb_resident_startup_risk; then
    stop_started_processes
    rm -f "${PID_FILE}"
    exit 1
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
  BASIS_OBSERVER_BASIS_RESIDENT_OUT="${BASIS_RESIDENT_OUT_DIR}" \
  BASIS_OBSERVER_FUNDING_ARB_MODE="${FUNDING_ARB_MODE}" \
  BASIS_OBSERVER_FUNDING_ARB_RESIDENT_OUT="${FUNDING_ARB_RESIDENT_OUT_DIR}" \
  BASIS_OBSERVER_FUNDING_SETTLEMENT_LEDGER="${FUNDING_SETTLEMENT_LEDGER}" \
  BASIS_OBSERVER_FUNDING_SETTLEMENT_RAW_SNAPSHOT="${FUNDING_SETTLEMENT_RAW_SNAPSHOT}" \
  BASIS_OBSERVER_BINANCE_POSITION_MODE="${BASIS_OBSERVER_BINANCE_POSITION_MODE:-hedge}" \
  BASIS_OBSERVER_BYBIT_POSITION_MODE="${BASIS_OBSERVER_BYBIT_POSITION_MODE:-hedge}" \
  BASIS_OBSERVER_OKX_POSITION_MODE="${BASIS_OBSERVER_OKX_POSITION_MODE:-long-short}" \
  BASIS_OBSERVER_BITGET_POSITION_MODE="${BASIS_OBSERVER_BITGET_POSITION_MODE:-hedge}" \
  BASIS_OBSERVER_ASTER_POSITION_MODE="${BASIS_OBSERVER_ASTER_POSITION_MODE:-hedge}" \
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
  BASIS_OBSERVER_CURL_TIMEOUT_SECS="${CURL_TIMEOUT_SECS}" \
  BASIS_OBSERVER_CURL_RETRIES="${CURL_RETRIES}" \
  BASIS_OBSERVER_CURL_RETRY_SLEEP_SECS="${CURL_RETRY_SLEEP_SECS}" \
  BASIS_OBSERVER_RUST_RECORDER_ENABLED="${RUST_RECORDER_ENABLED}" \
  BASIS_OBSERVER_BLOCKING_PATH_EVENT_LIMIT="${BLOCKING_PATH_EVENT_LIMIT}" \
  BASIS_OBSERVER_HEALTH_EVENT_SAMPLE_SECS="${HEALTH_EVENT_SAMPLE_SECS}" \
  BASIS_OBSERVER_RESIDENT_HEALTH_TAIL_LINES="${RESIDENT_HEALTH_TAIL_LINES}" \
  BASIS_OBSERVER_LOG_ROTATE_BYTES="${LOG_ROTATE_BYTES}" \
  BASIS_OBSERVER_LOG_ROTATE_KEEP="${LOG_ROTATE_KEEP}" \
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

JSON APIs:
  Binance: $(status_url binance)
  Bybit:   $(status_url bybit)
  OKX:     $(status_url okx)
  Bitget:  $(status_url bitget)
  Aster:   $(status_url aster)
  Hyperliquid: $(status_url hyperliquid)
  Funding arb: http://${FUNDING_ARB_BIND}/api/funding-arb/status

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
