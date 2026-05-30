#!/usr/bin/env bash
set -euo pipefail

# 中文说明：清理服务器 /opt/easy-arb/target 下可重建缓存和过期运行产物。
# 默认只做 dry-run（预演），必须显式传 --execute 才会删除或改写服务器文件。
# 脚本不会读取或打印凭证，不会清理 state、position、summary、config、当前 release 二进制或锁文件。

HOST="${EASY_ARB_CLEANUP_HOST:-easy-arb-logreader}"
REMOTE_ROOT="${EASY_ARB_REMOTE_ROOT:-/opt/easy-arb}"
MODE="dry-run"

CLEAN_DEBUG_INCREMENTAL=1
CLEAN_LOGS=1
CLEAN_OPPORTUNITIES=1
CLEAN_CYCLES=1
CLEAN_EXIT=0

LOG_KEEP_HOURS=24
OPPORTUNITY_KEEP_HOURS=24
CYCLE_KEEP_HOURS=72
EXIT_KEEP_DAYS=30

TRIM_ACTIVE_OPPORTUNITIES=0
INCLUDE_OPEN_FILES=0

usage() {
  cat <<'USAGE'
用法:
  scripts/cleanup-server-target.sh [选项]

默认行为:
  只预演，不实际清理。传 --execute 后才会执行清理。

清理范围:
  1. target/debug/incremental
     Rust 增量编译缓存，可重建。默认纳入清理。

  2. target/arb-runtime/live/logs 和 live-prereq/logs
     按日志内容中的 ISO 时间戳清理超过保留期的记录。默认保留 24 小时。
     没有可识别时间戳的文件会跳过。

  3. target/arb-runtime/live/opportunities
     按 JSONL 行内容中的 ISO 时间戳清理超过保留期的轮转文件。默认保留 24 小时。
     默认只处理 *.jsonl.N 轮转文件；当前活跃 *.jsonl 文件需要 --trim-active-opportunities。

  4. target/arb-runtime/live/resident-live/*/cycles
     按 cycle 目录名或目录修改时间清理超过保留期的 cycle 目录。默认保留 72 小时。

  5. target/arb-runtime/live/resident-live/*/exit
     默认不清理。需要 --clean-exit 才会按目录修改时间清理，默认保留 30 天。

选项:
  --execute                         实际执行清理；不传则只预演
  --host HOST                       SSH 目标，默认 easy-arb-logreader
  --remote-root PATH                服务器仓库根目录，默认 /opt/easy-arb

  --log-hours HOURS                 日志内容保留小时数，默认 24
  --opportunity-hours HOURS         opportunities 内容保留小时数，默认 24
  --cycle-hours HOURS               cycles 目录保留小时数，默认 72
  --exit-days DAYS                  exit 目录保留天数，默认 30；只有 --clean-exit 时生效

  --trim-active-opportunities       同时清理当前活跃 *.jsonl 文件；默认只清理 *.jsonl.N
  --include-open-files              即使文件被进程打开也允许重写；默认跳过已打开文件
  --clean-exit                      启用 resident-live/*/exit 清理；默认关闭

  --no-debug-incremental            不清理 target/debug/incremental
  --no-logs                         不清理 logs 目录内容
  --no-opportunities                不清理 opportunities 目录内容
  --no-cycles                       不清理 cycles 目录

环境变量:
  EASY_ARB_CLEANUP_HOST             默认 SSH 目标
  EASY_ARB_REMOTE_ROOT              默认服务器仓库根目录

示例:
  scripts/cleanup-server-target.sh
  scripts/cleanup-server-target.sh --execute
  scripts/cleanup-server-target.sh --opportunity-hours 48 --cycle-hours 168 --execute
  scripts/cleanup-server-target.sh --clean-exit --exit-days 30 --execute
USAGE
}

die() {
  echo "error: $*" >&2
  exit 1
}

require_positive_integer() {
  local name="$1"
  local value="$2"
  [[ "${value}" =~ ^[1-9][0-9]*$ ]] || die "${name} must be a positive integer: ${value}"
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --execute)
      MODE="execute"
      shift
      ;;
    --host)
      [[ $# -ge 2 ]] || die "--host requires a value"
      HOST="$2"
      shift 2
      ;;
    --remote-root)
      [[ $# -ge 2 ]] || die "--remote-root requires a value"
      REMOTE_ROOT="$2"
      shift 2
      ;;
    --log-hours)
      [[ $# -ge 2 ]] || die "--log-hours requires a value"
      LOG_KEEP_HOURS="$2"
      shift 2
      ;;
    --opportunity-hours)
      [[ $# -ge 2 ]] || die "--opportunity-hours requires a value"
      OPPORTUNITY_KEEP_HOURS="$2"
      shift 2
      ;;
    --cycle-hours)
      [[ $# -ge 2 ]] || die "--cycle-hours requires a value"
      CYCLE_KEEP_HOURS="$2"
      shift 2
      ;;
    --exit-days)
      [[ $# -ge 2 ]] || die "--exit-days requires a value"
      EXIT_KEEP_DAYS="$2"
      shift 2
      ;;
    --trim-active-opportunities)
      TRIM_ACTIVE_OPPORTUNITIES=1
      shift
      ;;
    --include-open-files)
      INCLUDE_OPEN_FILES=1
      shift
      ;;
    --clean-exit)
      CLEAN_EXIT=1
      shift
      ;;
    --no-debug-incremental)
      CLEAN_DEBUG_INCREMENTAL=0
      shift
      ;;
    --no-logs)
      CLEAN_LOGS=0
      shift
      ;;
    --no-opportunities)
      CLEAN_OPPORTUNITIES=0
      shift
      ;;
    --no-cycles)
      CLEAN_CYCLES=0
      shift
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

require_positive_integer "--log-hours" "${LOG_KEEP_HOURS}"
require_positive_integer "--opportunity-hours" "${OPPORTUNITY_KEEP_HOURS}"
require_positive_integer "--cycle-hours" "${CYCLE_KEEP_HOURS}"
require_positive_integer "--exit-days" "${EXIT_KEEP_DAYS}"

echo "服务器: ${HOST}"
echo "远程根目录: ${REMOTE_ROOT}"
echo "模式: ${MODE}"
echo

ssh "${HOST}" bash -s -- \
  "${REMOTE_ROOT}" \
  "${MODE}" \
  "${CLEAN_DEBUG_INCREMENTAL}" \
  "${CLEAN_LOGS}" \
  "${CLEAN_OPPORTUNITIES}" \
  "${CLEAN_CYCLES}" \
  "${CLEAN_EXIT}" \
  "${LOG_KEEP_HOURS}" \
  "${OPPORTUNITY_KEEP_HOURS}" \
  "${CYCLE_KEEP_HOURS}" \
  "${EXIT_KEEP_DAYS}" \
  "${TRIM_ACTIVE_OPPORTUNITIES}" \
  "${INCLUDE_OPEN_FILES}" <<'REMOTE_SCRIPT'
set -euo pipefail

REMOTE_ROOT="${1%/}"
MODE="$2"
CLEAN_DEBUG_INCREMENTAL="$3"
CLEAN_LOGS="$4"
CLEAN_OPPORTUNITIES="$5"
CLEAN_CYCLES="$6"
CLEAN_EXIT="$7"
LOG_KEEP_HOURS="$8"
OPPORTUNITY_KEEP_HOURS="$9"
CYCLE_KEEP_HOURS="${10}"
EXIT_KEEP_DAYS="${11}"
TRIM_ACTIVE_OPPORTUNITIES="${12}"
INCLUDE_OPEN_FILES="${13}"

TARGET_ROOT="${REMOTE_ROOT}/target"
LIVE_ROOT="${TARGET_ROOT}/arb-runtime/live"
LIVE_PREREQ_ROOT="${TARGET_ROOT}/arb-runtime/live-prereq"

die() {
  echo "error: $*" >&2
  exit 1
}

path_size_human() {
  local path="$1"
  if [[ -e "${path}" ]]; then
    du -sh "${path}" 2>/dev/null | awk '{print $1}'
  else
    printf '0'
  fi
}

path_size_bytes() {
  local path="$1"
  if [[ -e "${path}" ]]; then
    du -sb "${path}" 2>/dev/null | awk '{print $1}'
  else
    printf '0'
  fi
}

remove_path() {
  local path="$1"
  local reason="$2"
  local size

  [[ -e "${path}" ]] || return 0
  size="$(path_size_human "${path}")"

  if [[ "${MODE}" == "execute" ]]; then
    rm -rf -- "${path}"
    echo "已清理: ${size} ${path} (${reason})"
  else
    echo "计划清理: ${size} ${path} (${reason})"
  fi
}

file_is_open() {
  local path="$1"

  if [[ "${INCLUDE_OPEN_FILES}" == "1" ]]; then
    return 1
  fi

  if command -v lsof >/dev/null 2>&1; then
    lsof -- "${path}" >/dev/null 2>&1
    return $?
  fi

  if command -v fuser >/dev/null 2>&1; then
    fuser -- "${path}" >/dev/null 2>&1
    return $?
  fi

  return 1
}

iso_timestamp_stats() {
  local cutoff_epoch="$1"
  local path="$2"

  perl -MTime::Local=timegm -e '
    my $cutoff = shift @ARGV;
    my ($old_records, $kept_records, $removed_lines, $kept_lines, $no_ts_lines) = (0, 0, 0, 0, 0);
    my $keep = 0;

    sub line_epoch {
      my ($line) = @_;
      return undef unless $line =~ /(\d{4})-(\d{2})-(\d{2})[ T](\d{2}):(\d{2}):(\d{2})(?:\.\d+)?(?:\s*)?(Z|[+-]\d{2}:?\d{2})?/;

      my ($year, $month, $day, $hour, $minute, $second, $tz) = ($1, $2, $3, $4, $5, $6, $7 // "Z");
      my $epoch = timegm($second, $minute, $hour, $day, $month - 1, $year);

      if ($tz ne "Z") {
        $tz =~ /^([+-])(\d{2}):?(\d{2})$/ or return undef;
        my $offset = ($2 * 3600) + ($3 * 60);
        $offset *= -1 if $1 eq "-";
        $epoch -= $offset;
      }

      return $epoch;
    }

    while (my $line = <>) {
      my $epoch = line_epoch($line);
      if (defined $epoch) {
        if ($epoch >= $cutoff) {
          $keep = 1;
          $kept_records++;
        } else {
          $keep = 0;
          $old_records++;
        }
      } else {
        $no_ts_lines++;
      }

      if ($keep) {
        $kept_lines++;
      } else {
        $removed_lines++;
      }
    }

    print join("\t", $old_records, $kept_records, $removed_lines, $kept_lines, $no_ts_lines), "\n";
  ' "${cutoff_epoch}" "${path}"
}

filter_iso_timestamp_file() {
  local cutoff_epoch="$1"
  local path="$2"
  local tmp="$3"

  perl -MTime::Local=timegm -e '
    my $cutoff = shift @ARGV;
    my $keep = 0;

    sub line_epoch {
      my ($line) = @_;
      return undef unless $line =~ /(\d{4})-(\d{2})-(\d{2})[ T](\d{2}):(\d{2}):(\d{2})(?:\.\d+)?(?:\s*)?(Z|[+-]\d{2}:?\d{2})?/;

      my ($year, $month, $day, $hour, $minute, $second, $tz) = ($1, $2, $3, $4, $5, $6, $7 // "Z");
      my $epoch = timegm($second, $minute, $hour, $day, $month - 1, $year);

      if ($tz ne "Z") {
        $tz =~ /^([+-])(\d{2}):?(\d{2})$/ or return undef;
        my $offset = ($2 * 3600) + ($3 * 60);
        $offset *= -1 if $1 eq "-";
        $epoch -= $offset;
      }

      return $epoch;
    }

    while (my $line = <>) {
      my $epoch = line_epoch($line);
      $keep = ($epoch >= $cutoff) if defined $epoch;
      print $line if $keep;
    }
  ' "${cutoff_epoch}" "${path}" > "${tmp}"
}

trim_file_by_iso_timestamp() {
  local path="$1"
  local cutoff_epoch="$2"
  local label="$3"
  local stats
  local old_records
  local kept_records
  local removed_lines
  local kept_lines
  local no_ts_lines
  local before_size
  local after_size
  local tmp

  [[ -s "${path}" ]] || return 0

  stats="$(iso_timestamp_stats "${cutoff_epoch}" "${path}")"
  IFS=$'\t' read -r old_records kept_records removed_lines kept_lines no_ts_lines <<< "${stats}"

  if [[ "${old_records}" == "0" ]]; then
    if [[ "${kept_records}" == "0" ]]; then
      echo "跳过: ${path} (${label}; 未识别到可清理的 ISO 时间戳记录)"
    fi
    return 0
  fi

  before_size="$(path_size_bytes "${path}")"

  if file_is_open "${path}"; then
    echo "跳过: ${path} (${label}; 文件正被进程打开，避免重写时丢失新写入；可传 --include-open-files 强制处理)"
    return 0
  fi

  if [[ "${MODE}" == "execute" ]]; then
    tmp="$(mktemp "${path}.cleanup.XXXXXX")"
    if filter_iso_timestamp_file "${cutoff_epoch}" "${path}" "${tmp}"; then
      cat "${tmp}" > "${path}"
      rm -f -- "${tmp}"
    else
      rm -f -- "${tmp}"
      return 1
    fi
    after_size="$(path_size_bytes "${path}")"
    echo "已裁剪: ${path} (${label}; 旧记录=${old_records}, 保留记录=${kept_records}, 删除行=${removed_lines}, 保留行=${kept_lines}, ${before_size} -> ${after_size} bytes)"
  else
    echo "计划裁剪: ${path} (${label}; 旧记录=${old_records}, 保留记录=${kept_records}, 删除行=${removed_lines}, 保留行=${kept_lines}, 当前=${before_size} bytes)"
  fi
}

compact_utc_epoch_from_name() {
  local name="$1"

  if [[ "${name}" =~ ([0-9]{4})-([0-9]{2})-([0-9]{2})T([0-9]{2})([0-9]{2})([0-9]{2}) ]]; then
    date -u -d "${BASH_REMATCH[1]}-${BASH_REMATCH[2]}-${BASH_REMATCH[3]} ${BASH_REMATCH[4]}:${BASH_REMATCH[5]}:${BASH_REMATCH[6]}" +%s
    return 0
  fi

  return 1
}

mtime_epoch() {
  local path="$1"
  stat -c %Y "${path}"
}

clean_debug_incremental() {
  local path="${TARGET_ROOT}/debug/incremental"
  remove_path "${path}" "Rust 增量编译缓存，可重建"
}

clean_log_files() {
  local cutoff_epoch
  local dir
  local path

  cutoff_epoch="$(( $(date -u +%s) - (LOG_KEEP_HOURS * 3600) ))"

  for dir in "${LIVE_ROOT}/logs" "${LIVE_PREREQ_ROOT}/logs"; do
    [[ -d "${dir}" ]] || continue
    while IFS= read -r -d '' path; do
      trim_file_by_iso_timestamp "${path}" "${cutoff_epoch}" "日志内容保留 ${LOG_KEEP_HOURS} 小时"
    done < <(find "${dir}" -maxdepth 1 -type f -print0)
  done
}

clean_opportunity_files() {
  local cutoff_epoch
  local dir="${LIVE_ROOT}/opportunities"
  local path
  local base

  [[ -d "${dir}" ]] || return 0
  cutoff_epoch="$(( $(date -u +%s) - (OPPORTUNITY_KEEP_HOURS * 3600) ))"

  while IFS= read -r -d '' path; do
    base="$(basename "${path}")"
    if [[ "${TRIM_ACTIVE_OPPORTUNITIES}" != "1" && ! "${base}" =~ \.jsonl\.[0-9]+$ ]]; then
      echo "跳过: ${path} (当前活跃或非轮转 opportunity 文件；可传 --trim-active-opportunities 处理)"
      continue
    fi
    trim_file_by_iso_timestamp "${path}" "${cutoff_epoch}" "opportunities 内容保留 ${OPPORTUNITY_KEEP_HOURS} 小时"
  done < <(find "${dir}" -maxdepth 1 -type f -name '*.jsonl*' -print0)
}

clean_cycle_dirs() {
  local cutoff_epoch
  local cycles_root
  local path
  local base
  local epoch

  cutoff_epoch="$(( $(date -u +%s) - (CYCLE_KEEP_HOURS * 3600) ))"

  while IFS= read -r -d '' cycles_root; do
    while IFS= read -r -d '' path; do
      base="$(basename "${path}")"
      if ! epoch="$(compact_utc_epoch_from_name "${base}")"; then
        epoch="$(mtime_epoch "${path}")"
      fi
      if (( epoch < cutoff_epoch )); then
        remove_path "${path}" "resident cycle 超过 ${CYCLE_KEEP_HOURS} 小时"
      fi
    done < <(find "${cycles_root}" -mindepth 1 -maxdepth 1 -type d -print0)
  done < <(find "${LIVE_ROOT}/resident-live" -path '*/cycles' -type d -print0 2>/dev/null || true)
}

clean_exit_dirs() {
  local cutoff_minutes
  local exit_root
  local path

  cutoff_minutes="$(( EXIT_KEEP_DAYS * 24 * 60 ))"

  while IFS= read -r -d '' exit_root; do
    while IFS= read -r -d '' path; do
      remove_path "${path}" "resident exit 超过 ${EXIT_KEEP_DAYS} 天"
    done < <(find "${exit_root}" -mindepth 1 -maxdepth 1 -type d -mmin "+${cutoff_minutes}" -print0)
  done < <(find "${LIVE_ROOT}/resident-live" -path '*/exit' -type d -print0 2>/dev/null || true)
}

[[ -d "${TARGET_ROOT}" ]] || die "target root does not exist: ${TARGET_ROOT}"

echo "开始前 target 占用: $(path_size_human "${TARGET_ROOT}")"
echo "说明: dry-run 只打印计划；execute 才会实际清理。"
echo

if [[ "${CLEAN_DEBUG_INCREMENTAL}" == "1" ]]; then
  clean_debug_incremental
fi

if [[ "${CLEAN_LOGS}" == "1" ]]; then
  clean_log_files
fi

if [[ "${CLEAN_OPPORTUNITIES}" == "1" ]]; then
  clean_opportunity_files
fi

if [[ "${CLEAN_CYCLES}" == "1" ]]; then
  clean_cycle_dirs
fi

if [[ "${CLEAN_EXIT}" == "1" ]]; then
  clean_exit_dirs
else
  echo "跳过: ${LIVE_ROOT}/resident-live/*/exit (默认不清理退出/平仓追踪数据；需要 --clean-exit)"
fi

echo
echo "结束时 target 占用: $(path_size_human "${TARGET_ROOT}")"
REMOTE_SCRIPT
