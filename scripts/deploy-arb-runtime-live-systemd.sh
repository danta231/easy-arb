#!/usr/bin/env bash
set -euo pipefail

# 中文说明：在服务器上部署 easy-arb runtime live 栈。
# 职责边界：
# 1. 拉取代码并构建 release 二进制。
# 2. 安装/刷新 systemd unit。
# 3. 重启 easy-arb-runtime-live，并做基础健康检查。
# 脚本不读取、不打印密钥；密钥仍由 systemd unit 的 EnvironmentFile 注入。

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"

SERVICE_NAME="${EASY_ARB_DEPLOY_SERVICE_NAME:-easy-arb-runtime-live}"
DEPLOY_USER="${EASY_ARB_DEPLOY_USER:-deploy}"
UNIT_SOURCE="${EASY_ARB_DEPLOY_UNIT_SOURCE:-${REPO_ROOT}/deploy/systemd/${SERVICE_NAME}.service}"
UNIT_DEST="${EASY_ARB_DEPLOY_UNIT_DEST:-/etc/systemd/system/${SERVICE_NAME}.service}"
VERIFY_MODE="${EASY_ARB_DEPLOY_VERIFY:-none}"
STOP_LEGACY="${EASY_ARB_DEPLOY_STOP_LEGACY:-auto}"
HEALTH_WAIT_SECS="${EASY_ARB_DEPLOY_HEALTH_WAIT_SECS:-120}"
JOURNAL_LINES="${EASY_ARB_DEPLOY_JOURNAL_LINES:-80}"

PULL=1
INSTALL_UNIT=1
ENABLE_SERVICE=1
RESTART_SERVICE=1
HEALTH_CHECK=1

usage() {
  cat <<'USAGE'
用法:
  scripts/deploy-arb-runtime-live-systemd.sh [选项]

默认行为:
  1. 在当前仓库执行 git pull --ff-only。
  2. 构建 arb-runtime 和 arb-wallet-signer 的 release 二进制。
  3. 安装 deploy/systemd/easy-arb-runtime-live.service 到 /etc/systemd/system/。
  4. systemctl daemon-reload。
  5. 如果服务尚未由 systemd 运行，自动尝试停止旧 deploy 脚本栈。
  6. enable 并 restart easy-arb-runtime-live。
  7. 等待 portfolio 和 funding-arb 健康接口可访问。

日常更新:
  cd /opt/easy-arb
  scripts/deploy-arb-runtime-live-systemd.sh

日常启停:
  sudo systemctl restart easy-arb-runtime-live
  sudo systemctl stop easy-arb-runtime-live
  sudo systemctl status easy-arb-runtime-live --no-pager -l
  journalctl -u easy-arb-runtime-live -f

选项:
  --no-pull                  不执行 git pull；适合代码已通过其他方式同步到服务器时使用
  --no-install-unit          不重新安装 systemd unit
  --no-enable                不执行 systemctl enable
  --no-restart               只构建/安装，不重启服务
  --no-health-check          不等待健康接口

  --stop-legacy              重启前强制尝试停止旧 deploy 脚本栈
  --no-stop-legacy           不停止旧 deploy 脚本栈
  --verify MODE              部署前验证模式：none、smoke、workspace；默认 none
                             smoke 只跑 Bitget/OKX WSS bootstrap 相关测试
                             workspace 跑 fmt、clippy、workspace tests 和 xtask quality-gate
  --health-wait-secs SECS    健康检查最长等待秒数，默认 120
  --journal-lines N          部署完成后显示 journal 行数，默认 80
  -h, --help                 显示帮助

环境变量:
  EASY_ARB_DEPLOY_SERVICE_NAME       systemd 服务名，默认 easy-arb-runtime-live
  EASY_ARB_DEPLOY_USER               构建/拉代码使用的系统用户，默认 deploy
  EASY_ARB_DEPLOY_UNIT_SOURCE        unit 源文件路径，默认 deploy/systemd/easy-arb-runtime-live.service
  EASY_ARB_DEPLOY_UNIT_DEST          unit 安装路径，默认 /etc/systemd/system/easy-arb-runtime-live.service
  EASY_ARB_DEPLOY_VERIFY             验证模式，默认 none
  EASY_ARB_DEPLOY_STOP_LEGACY        auto、always、never；默认 auto
  EASY_ARB_DEPLOY_HEALTH_WAIT_SECS   健康检查最长等待秒数，默认 120
  EASY_ARB_DEPLOY_JOURNAL_LINES      journal 输出行数，默认 80
USAGE
}

die() {
  echo "error: $*" >&2
  exit 1
}

info() {
  echo "==> $*"
}

warn() {
  echo "warning: $*" >&2
}

require_command() {
  command -v "$1" >/dev/null 2>&1 || die "missing required command: $1"
}

require_non_negative_integer() {
  local name="$1"
  local value="$2"

  [[ "${value}" =~ ^[0-9]+$ ]] || die "${name} must be a non-negative integer: ${value}"
}

run_as_deploy_in_repo() {
  sudo -H -u "${DEPLOY_USER}" bash -lc 'export PATH="$HOME/.cargo/bin:$PATH"; cd "$1" && shift && "$@"' bash "${REPO_ROOT}" "$@"
}

require_deploy_command() {
  run_as_deploy_in_repo command -v "$1" >/dev/null 2>&1 \
    || die "missing required command for ${DEPLOY_USER}: $1"
}

should_stop_legacy_stack() {
  case "${STOP_LEGACY}" in
    always)
      return 0
      ;;
    never)
      return 1
      ;;
    auto)
      if sudo systemctl is-active --quiet "${SERVICE_NAME}"; then
        return 1
      fi
      return 0
      ;;
    *)
      die "EASY_ARB_DEPLOY_STOP_LEGACY must be auto, always, or never: ${STOP_LEGACY}"
      ;;
  esac
}

stop_legacy_stack() {
  info "尝试停止旧 deploy 脚本栈"
  sudo -H -u "${DEPLOY_USER}" bash -lc 'export PATH="$HOME/.cargo/bin:$PATH"; cd "$1" && shift && exec env "$@"' bash "${REPO_ROOT}" \
    ARB_RUNTIME_LIVE_BASE_ENV_FILE=/dev/null \
    "ARB_RUNTIME_LIVE_ROOT=${REPO_ROOT}/target/arb-runtime/live" \
    "ARB_RUNTIME_LIVE_PREREQ_ROOT=${REPO_ROOT}/target/arb-runtime/live-prereq" \
    "ARB_RUNTIME_LIVE_BIN=${REPO_ROOT}/target/release/arb-runtime" \
    scripts/stop-arb-runtime-live.sh
}

run_verification() {
  case "${VERIFY_MODE}" in
    none)
      return 0
      ;;
    smoke)
      info "运行 smoke 验证"
      run_as_deploy_in_repo cargo test -p arb-runtime okx_and_bitget_wss_rest_bootstrap_prepare_all_usdt_rows
      ;;
    workspace)
      info "运行 workspace 验证"
      run_as_deploy_in_repo cargo fmt --all -- --check
      run_as_deploy_in_repo cargo clippy --workspace --all-targets -- -D warnings
      run_as_deploy_in_repo cargo test --workspace
      run_as_deploy_in_repo cargo xtask quality-gate
      ;;
    *)
      die "--verify must be one of: none, smoke, workspace"
      ;;
  esac
}

wait_for_http_ok() {
  local label="$1"
  local url="$2"
  local deadline="$((SECONDS + HEALTH_WAIT_SECS))"
  local last_error=""

  info "等待健康接口: ${label} ${url}"
  while (( SECONDS <= deadline )); do
    if last_error="$(curl -fsS --max-time 2 "${url}" 2>&1 >/dev/null)"; then
      echo "healthy: ${label}"
      return 0
    fi
    sleep 2
  done

  echo "health check failed: ${label} ${url}" >&2
  if [[ -n "${last_error}" ]]; then
    echo "${last_error}" >&2
  fi
  return 1
}

print_service_diagnostics() {
  warn "服务未达到预期状态，输出诊断信息"
  sudo systemctl status "${SERVICE_NAME}" --no-pager -l || true
  sudo journalctl -u "${SERVICE_NAME}" -n "${JOURNAL_LINES}" --no-pager || true
  sudo ss -H -ltnp 2>/dev/null | grep -E ':(8796|8797|8798|8799|8800|8803|8804|8805|8806|8789|8791|8793|8794|8795|8816|8817|8818|8819|8820|8821|8822|8823)\b' || true
  ps -eo user,pid,ppid,lstart,cmd | grep -E '[a]rb-runtime|[s]tart-arb-runtime-live' || true
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --no-pull)
      PULL=0
      shift
      ;;
    --no-install-unit)
      INSTALL_UNIT=0
      shift
      ;;
    --no-enable)
      ENABLE_SERVICE=0
      shift
      ;;
    --no-restart)
      RESTART_SERVICE=0
      shift
      ;;
    --no-health-check)
      HEALTH_CHECK=0
      shift
      ;;
    --stop-legacy)
      STOP_LEGACY="always"
      shift
      ;;
    --no-stop-legacy)
      STOP_LEGACY="never"
      shift
      ;;
    --verify)
      [[ $# -ge 2 ]] || die "--verify requires a value"
      VERIFY_MODE="$2"
      shift 2
      ;;
    --health-wait-secs)
      [[ $# -ge 2 ]] || die "--health-wait-secs requires a value"
      HEALTH_WAIT_SECS="$2"
      shift 2
      ;;
    --journal-lines)
      [[ $# -ge 2 ]] || die "--journal-lines requires a value"
      JOURNAL_LINES="$2"
      shift 2
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      die "unknown option: $1"
      ;;
  esac
done

require_non_negative_integer "--health-wait-secs" "${HEALTH_WAIT_SECS}"
require_non_negative_integer "--journal-lines" "${JOURNAL_LINES}"

require_command bash
require_command curl
require_command grep
require_command ps
require_command sudo
require_command systemctl

[[ -d "${REPO_ROOT}/.git" ]] || die "not a git repository: ${REPO_ROOT}"
[[ -f "${UNIT_SOURCE}" ]] || die "systemd unit source not found: ${UNIT_SOURCE}"
grep -q '^ReadWritePaths=/var/lib/easy-arb /tmp$' "${UNIT_SOURCE}" \
  || die "systemd unit must include: ReadWritePaths=/var/lib/easy-arb /tmp"

info "仓库: ${REPO_ROOT}"
info "服务: ${SERVICE_NAME}"
info "部署用户: ${DEPLOY_USER}"
info "验证模式: ${VERIFY_MODE}"

require_deploy_command cargo
require_deploy_command git

if [[ "${PULL}" == "1" ]]; then
  info "检查服务器工作区状态"
  run_as_deploy_in_repo git status --short
  info "拉取最新代码"
  run_as_deploy_in_repo git pull --ff-only
else
  info "跳过 git pull"
fi

run_verification

info "构建 release 二进制"
run_as_deploy_in_repo cargo build --release -p arb-runtime -p arb-wallet-signer

info "确认 release 二进制存在"
run_as_deploy_in_repo test -x "${REPO_ROOT}/target/release/arb-runtime"
run_as_deploy_in_repo test -x "${REPO_ROOT}/target/release/arb-wallet-signer"

if [[ "${INSTALL_UNIT}" == "1" ]]; then
  info "安装 systemd unit"
  sudo install -m 0644 "${UNIT_SOURCE}" "${UNIT_DEST}"
  sudo systemctl daemon-reload
  if ! sudo systemctl cat "${SERVICE_NAME}" | grep -q '^ReadWritePaths=/var/lib/easy-arb /tmp$'; then
    die "installed systemd unit is missing: ReadWritePaths=/var/lib/easy-arb /tmp"
  fi
else
  info "跳过 systemd unit 安装"
fi

if should_stop_legacy_stack; then
  stop_legacy_stack || warn "旧 deploy 脚本栈停止命令未完整成功；如果后续重启失败，请查看端口占用诊断"
else
  info "跳过旧 deploy 脚本栈停止"
fi

if [[ "${ENABLE_SERVICE}" == "1" ]]; then
  info "启用 systemd 服务"
  sudo systemctl enable "${SERVICE_NAME}"
fi

if [[ "${RESTART_SERVICE}" == "1" ]]; then
  info "重启 systemd 服务"
  if ! sudo systemctl restart "${SERVICE_NAME}"; then
    print_service_diagnostics
    exit 1
  fi
else
  info "跳过 systemd 服务重启"
fi

if [[ "${RESTART_SERVICE}" == "1" ]]; then
  info "检查 systemd 服务状态"
  if ! sudo systemctl is-active --quiet "${SERVICE_NAME}"; then
    print_service_diagnostics
    exit 1
  fi
fi

if [[ "${HEALTH_CHECK}" == "1" && "${RESTART_SERVICE}" == "1" ]]; then
  wait_for_http_ok "portfolio" "http://127.0.0.1:8805/health" || {
    print_service_diagnostics
    exit 1
  }
  wait_for_http_ok "funding-arb" "http://127.0.0.1:8804/health" || {
    print_service_diagnostics
    exit 1
  }
else
  info "跳过健康检查"
fi

info "部署完成"
sudo systemctl status "${SERVICE_NAME}" --no-pager -l
if [[ "${JOURNAL_LINES}" != "0" ]]; then
  sudo journalctl -u "${SERVICE_NAME}" -n "${JOURNAL_LINES}" --no-pager
fi
