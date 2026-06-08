#!/usr/bin/env bash
set -euo pipefail

# 中文说明：把远程服务器上的 easy-arb 运行数据同步到本地。
# 默认同步 runtime 日志、机会/快照/报告、常见监控日志和已有数据库备份目录。
# 脚本不会读取本地密钥；可选 pg_dump 只在远程临时目录中生成 dump，并且不会打印数据库连接串。

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"

HOST="${EASY_ARB_SYNC_HOST:-easy-arb-logreader}"
REMOTE_ROOT="${EASY_ARB_REMOTE_ROOT:-/opt/easy-arb}"
OUT_ROOT="${EASY_ARB_SYNC_ROOT:-server-backups}"
TIMESTAMP="$(date +%Y%m%d-%H%M%S)"

INCLUDE_RUNTIME=1
INCLUDE_EXISTING_DB_BACKUPS=1
CREATE_PG_DUMP=0
CREATE_ARCHIVE=1
DRY_RUN=0

PG_DATABASE="${EASY_ARB_SYNC_PG_DATABASE:-}"
PG_URL_ENV="${EASY_ARB_SYNC_PG_URL_ENV:-}"
PG_ENV_FILE="${EASY_ARB_SYNC_PG_ENV_FILE:-}"

CUSTOM_DB_BACKUP_SPECS=()
CUSTOM_REMOTE_DIR_SPECS=()

REMOTE_ITEM_KINDS=()
REMOTE_ITEM_SOURCES=()
REMOTE_ITEM_LABELS=()
TMP_DIR=""

usage() {
  cat <<'USAGE'
用法:
  scripts/sync-server-data.sh [选项]

默认行为:
  通过 SSH 连接 easy-arb-logreader，把远程 /opt/easy-arb 下的运行数据打包下载到本地
  server-backups/<timestamp>/，并生成 server-backups/<timestamp>.tar.gz。

默认同步范围:
  1. target/arb-runtime/live/logs
  2. target/arb-runtime/live-prereq/logs
  3. target/arb-runtime/live/opportunities
  4. target/arb-runtime/live/resident-live
  5. target/arb-runtime/live/snapshots
  6. target/arb-runtime/live/live
  7. target/arb-opportunity-observer/logs
  8. target/basis-monitor-logs
  9. target/wss-book-ticker-logs
  10. 常见数据库备份目录：server-backups、db-backups、database-backups、backups、
      target/db-backups、target/database-backups、/var/backups/easy-arb、
      /var/lib/easy-arb/backups

选项:
  --host HOST                         SSH 目标，默认 easy-arb-logreader
  --remote-root PATH                  远程 easy-arb 根目录，默认 /opt/easy-arb
  --out-root PATH                     本地同步根目录，默认 server-backups
  --timestamp VALUE                   本次同步目录名，默认当前时间 YYYYMMDD-HHMMSS

  --no-runtime                        不同步默认 runtime 和监控日志目录
  --no-existing-db-backups            不同步默认数据库备份目录
  --db-backup-dir PATH[:LABEL]        额外同步远程数据库备份目录，可重复
  --remote-dir PATH[:LABEL]           额外同步任意远程目录，可重复

  --pg-dump                           在远程执行 pg_dump 并下载 dump 文件
  --pg-database NAME                  pg_dump 使用的数据库名；不传则使用远程 PGDATABASE
  --pg-url-env ENV_NAME               从远程环境变量 ENV_NAME 读取数据库连接串，不打印变量值
  --pg-env-file PATH                  远程 pg_dump 前 source 该环境文件；只传路径，不打印内容

  --no-archive                        不生成本地 tar.gz 归档
  --dry-run                           只显示计划，不连接服务器
  -h, --help                          显示帮助

环境变量:
  EASY_ARB_SYNC_HOST                  默认 SSH 目标
  EASY_ARB_REMOTE_ROOT                默认远程 easy-arb 根目录
  EASY_ARB_SYNC_ROOT                  默认本地同步根目录
  EASY_ARB_SYNC_PG_DATABASE           默认 pg_dump 数据库名
  EASY_ARB_SYNC_PG_URL_ENV            默认远程数据库连接串环境变量名
  EASY_ARB_SYNC_PG_ENV_FILE           默认远程 pg_dump 环境文件路径

示例:
  scripts/sync-server-data.sh --dry-run
  scripts/sync-server-data.sh
  scripts/sync-server-data.sh --pg-dump --pg-database easy_tool
  scripts/sync-server-data.sh --pg-dump --pg-url-env DATABASE_URL --pg-env-file /etc/easy-arb/history.env
  scripts/sync-server-data.sh --remote-dir /var/log/easy-arb:logs/var-log-easy-arb
USAGE
}

die() {
  echo "error: $*" >&2
  exit 1
}

require_command() {
  command -v "$1" >/dev/null 2>&1 || die "missing required command: $1"
}

shell_quote() {
  printf '%q' "$1"
}

cleanup_tmp_dir() {
  if [[ -n "${TMP_DIR}" && -d "${TMP_DIR}" ]]; then
    rm -rf "${TMP_DIR}"
  fi
}

absolute_out_root() {
  case "${OUT_ROOT}" in
    /*) printf '%s\n' "${OUT_ROOT}" ;;
    *) printf '%s\n' "${REPO_ROOT}/${OUT_ROOT}" ;;
  esac
}

validate_timestamp() {
  [[ "${TIMESTAMP}" =~ ^[A-Za-z0-9._-]+$ ]] || die "--timestamp may only contain letters, numbers, dot, underscore, and dash"
}

validate_label() {
  local label="$1"

  [[ -n "${label}" ]] || die "sync label must not be empty"
  [[ "${label}" != /* ]] || die "sync label must be relative: ${label}"
  [[ "${label}" != *".."* ]] || die "sync label must not contain '..': ${label}"
  [[ "${label}" =~ ^[-A-Za-z0-9._/]+$ ]] || die "sync label may only contain letters, numbers, dot, underscore, dash, and slash: ${label}"
}

safe_label_from_source() {
  local source="$1"
  local base="${source%/}"
  local safe

  base="${base##*/}"
  [[ -n "${base}" ]] || base="remote-data"
  safe="$(printf '%s' "${base}" | LC_ALL=C tr -c 'A-Za-z0-9._-' '-')"
  [[ -n "${safe}" ]] || safe="remote-data"
  printf '%s\n' "${safe}"
}

split_source_label() {
  local spec="$1"
  local default_prefix="$2"
  local source
  local label

  if [[ "${spec}" == *:* ]]; then
    source="${spec%%:*}"
    label="${spec#*:}"
  else
    source="${spec}"
    label="${default_prefix}/$(safe_label_from_source "${source}")"
  fi

  [[ -n "${source}" ]] || die "remote source path must not be empty"
  validate_label "${label}"
  printf '%s\t%s\n' "${source}" "${label}"
}

add_remote_item() {
  local kind="$1"
  local source="$2"
  local label="$3"

  [[ -n "${source}" ]] || die "remote source path must not be empty"
  validate_label "${label}"
  REMOTE_ITEM_KINDS+=("${kind}")
  REMOTE_ITEM_SOURCES+=("${source}")
  REMOTE_ITEM_LABELS+=("${label}")
}

add_default_runtime_items() {
  add_remote_item "runtime" "target/arb-runtime/live/logs" "runtime/live-logs"
  add_remote_item "runtime" "target/arb-runtime/live-prereq/logs" "runtime/live-prereq-logs"
  add_remote_item "runtime" "target/arb-runtime/live/opportunities" "runtime/opportunities"
  add_remote_item "runtime" "target/arb-runtime/live/resident-live" "runtime/resident-live"
  add_remote_item "runtime" "target/arb-runtime/live/snapshots" "runtime/snapshots"
  add_remote_item "runtime" "target/arb-runtime/live/live" "runtime/live-reports"
  add_remote_item "log" "target/arb-opportunity-observer/logs" "logs/arb-opportunity-observer"
  add_remote_item "log" "target/basis-monitor-logs" "logs/basis-monitor"
  add_remote_item "log" "target/wss-book-ticker-logs" "logs/wss-book-ticker"
}

add_default_db_backup_items() {
  add_remote_item "database-backup" "server-backups" "database-backups/server-backups"
  add_remote_item "database-backup" "db-backups" "database-backups/db-backups"
  add_remote_item "database-backup" "database-backups" "database-backups/database-backups"
  add_remote_item "database-backup" "backups" "database-backups/backups"
  add_remote_item "database-backup" "target/db-backups" "database-backups/target-db-backups"
  add_remote_item "database-backup" "target/database-backups" "database-backups/target-database-backups"
  add_remote_item "database-backup" "/var/backups/easy-arb" "database-backups/var-backups-easy-arb"
  add_remote_item "database-backup" "/var/lib/easy-arb/backups" "database-backups/var-lib-easy-arb-backups"
}

build_remote_items() {
  local spec
  local source_label
  local source
  local label

  REMOTE_ITEM_KINDS=()
  REMOTE_ITEM_SOURCES=()
  REMOTE_ITEM_LABELS=()

  if [[ "${INCLUDE_RUNTIME}" == "1" ]]; then
    add_default_runtime_items
  fi

  if [[ "${INCLUDE_EXISTING_DB_BACKUPS}" == "1" ]]; then
    add_default_db_backup_items
  fi

  if [[ "${#CUSTOM_DB_BACKUP_SPECS[@]}" -gt 0 ]]; then
    for spec in "${CUSTOM_DB_BACKUP_SPECS[@]}"; do
      source_label="$(split_source_label "${spec}" "database-backups/custom")"
      source="${source_label%%$'\t'*}"
      label="${source_label#*$'\t'}"
      add_remote_item "database-backup" "${source}" "${label}"
    done
  fi

  if [[ "${#CUSTOM_REMOTE_DIR_SPECS[@]}" -gt 0 ]]; then
    for spec in "${CUSTOM_REMOTE_DIR_SPECS[@]}"; do
      source_label="$(split_source_label "${spec}" "extra")"
      source="${source_label%%$'\t'*}"
      label="${source_label#*$'\t'}"
      add_remote_item "extra" "${source}" "${label}"
    done
  fi
}

print_plan() {
  local index

  echo "同步计划:"
  echo "  SSH 目标:      ${HOST}"
  echo "  远程根目录:    ${REMOTE_ROOT}"
  echo "  本地输出根:    $(absolute_out_root)"
  echo "  同步目录名:    ${TIMESTAMP}"
  echo "  生成归档:      ${CREATE_ARCHIVE}"
  echo "  远程 pg_dump:  ${CREATE_PG_DUMP}"
  if [[ "${CREATE_PG_DUMP}" == "1" ]]; then
    echo "  pg 数据库名:    ${PG_DATABASE:-<远程 PGDATABASE>}"
    echo "  pg 连接串变量:  ${PG_URL_ENV:-<未使用>}"
    echo "  pg 环境文件:    ${PG_ENV_FILE:-<未使用>}"
  fi
  echo
  echo "将同步的远程目录:"
  if [[ "${#REMOTE_ITEM_SOURCES[@]}" -eq 0 ]]; then
    echo "  <无目录；只有 --pg-dump 时会生成数据库 dump>"
    return
  fi
  for index in "${!REMOTE_ITEM_SOURCES[@]}"; do
    echo "  - [${REMOTE_ITEM_KINDS[${index}]}] ${REMOTE_ITEM_SOURCES[${index}]} -> ${REMOTE_ITEM_LABELS[${index}]}"
  done
}

download_remote_archive() {
  local archive_target="$1"
  local remote_command="bash -s --"
  local remote_args=()
  local arg
  local index

  remote_args+=("${REMOTE_ROOT}")
  remote_args+=("${TIMESTAMP}")
  remote_args+=("${HOST}")
  remote_args+=("${CREATE_PG_DUMP}")
  remote_args+=("${PG_DATABASE}")
  remote_args+=("${PG_URL_ENV}")
  remote_args+=("${PG_ENV_FILE}")
  remote_args+=("${#REMOTE_ITEM_SOURCES[@]}")

  for index in "${!REMOTE_ITEM_SOURCES[@]}"; do
    remote_args+=("${REMOTE_ITEM_KINDS[${index}]}")
    remote_args+=("${REMOTE_ITEM_SOURCES[${index}]}")
    remote_args+=("${REMOTE_ITEM_LABELS[${index}]}")
  done

  for arg in "${remote_args[@]}"; do
    remote_command+=" $(shell_quote "${arg}")"
  done

  ssh "${HOST}" "${remote_command}" > "${archive_target}" <<'REMOTE_SCRIPT'
set -euo pipefail

remote_root="${1%/}"
timestamp="$2"
host_label="$3"
create_pg_dump="$4"
pg_database="$5"
pg_url_env="$6"
pg_env_file="$7"
item_count="$8"
shift 8

if [[ -z "${remote_root}" ]]; then
  remote_root="/"
fi

tmp_dir="$(mktemp -d)"
stage_parent="${tmp_dir}/stage"
stage_dir="${stage_parent}/${timestamp}"
manifest="${stage_dir}/manifest.txt"

cleanup() {
  rm -rf "${tmp_dir}"
}
trap cleanup EXIT

mkdir -p "${stage_dir}"

cat > "${manifest}" <<EOF
同步时间: ${timestamp}
服务器: ${host_label}
远程根目录: ${remote_root}
说明: 服务器端先复制到临时快照目录，再 tar/gzip 打包到本地；不会打包 .env、私钥和常见密钥文件。
EOF

die_remote() {
  echo "error: $*" >&2
  exit 1
}

resolve_remote_path() {
  local source="$1"

  if [[ "${source}" == /* ]]; then
    printf '%s\n' "${source}"
  elif [[ "${remote_root}" == "/" ]]; then
    printf '/%s\n' "${source}"
  else
    printf '%s/%s\n' "${remote_root}" "${source}"
  fi
}

copy_remote_dir_to_snapshot() {
  local source_dir="$1"
  local snapshot_dir="$2"
  local rel
  local source_path
  local snapshot_path

  mkdir -p "${snapshot_dir}"

  (
    cd "${source_dir}"
    find . \
      \( \
        -name '.env' \
        -o -name '.env.*' \
        -o -name '*.pem' \
        -o -name '*.key' \
        -o -name '*.p12' \
        -o -name '*.pfx' \
        -o -name '*.secret' \
        -o -name 'secrets' \
        -o -name 'secrets.*' \
        -o -name 'id_rsa' \
        -o -name 'id_ed25519' \
      \) -prune -o -print0
  ) | while IFS= read -r -d '' rel; do
    [[ "${rel}" == "." ]] && continue

    source_path="${source_dir}/${rel#./}"
    snapshot_path="${snapshot_dir}/${rel#./}"

    if [[ -d "${source_path}" && ! -L "${source_path}" ]]; then
      mkdir -p "${snapshot_path}"
    elif [[ -f "${source_path}" || -L "${source_path}" ]]; then
      mkdir -p "$(dirname "${snapshot_path}")"
      if ! cp -Pp "${source_path}" "${snapshot_path}"; then
        if [[ ! -e "${source_path}" && ! -L "${source_path}" ]]; then
          echo "跳过已轮转或消失的文件: ${source_path}" >&2
        else
          echo "error: failed to copy remote data: ${source_path}" >&2
          return 1
        fi
      fi
    fi
  done
}

add_remote_dir() {
  local kind="$1"
  local source="$2"
  local label="$3"
  local source_path
  local snapshot_path

  source_path="$(resolve_remote_path "${source}")"
  snapshot_path="${stage_dir}/${label}"

  if [[ ! -d "${source_path}" ]]; then
    echo "跳过不存在的远程目录: ${source_path}" >&2
    return
  fi

  if [[ ! -r "${source_path}" ]]; then
    echo "跳过不可读的远程目录: ${source_path}" >&2
    return
  fi

  echo "复制远程目录: ${source_path}/ -> ${label}/" >&2
  copy_remote_dir_to_snapshot "${source_path}" "${snapshot_path}"
  echo "included ${kind}: ${source_path} -> ${label}" >> "${manifest}"
}

sanitize_dump_name() {
  local value="$1"
  local safe

  [[ -n "${value}" ]] || value="pg-dump"
  safe="$(printf '%s' "${value}" | LC_ALL=C tr -c 'A-Za-z0-9._-' '-')"
  [[ -n "${safe}" ]] || safe="pg-dump"
  printf '%s\n' "${safe}"
}

run_pg_dump_if_requested() {
  local dump_name
  local dump_file
  local connection_value

  [[ "${create_pg_dump}" == "1" ]] || return 0
  command -v pg_dump >/dev/null 2>&1 || die_remote "remote pg_dump is not available"

  if [[ -n "${pg_env_file}" ]]; then
    [[ -r "${pg_env_file}" ]] || die_remote "pg env file is not readable"
    set -a
    # shellcheck disable=SC1090
    . "${pg_env_file}"
    set +a
  fi

  dump_name="$(sanitize_dump_name "${pg_database:-${pg_url_env:-pg-dump}}")"
  mkdir -p "${stage_dir}/database-dumps"
  dump_file="${stage_dir}/database-dumps/${dump_name}-${timestamp}.dump"

  echo "生成远程数据库 dump: database-dumps/${dump_name}-${timestamp}.dump" >&2
  if [[ -n "${pg_url_env}" ]]; then
    connection_value="${!pg_url_env:-}"
    [[ -n "${connection_value}" ]] || die_remote "remote environment variable for pg connection is empty"
    pg_dump -Fc --no-owner --no-privileges -f "${dump_file}" "${connection_value}"
  elif [[ -n "${pg_database}" ]]; then
    pg_dump -Fc --no-owner --no-privileges -f "${dump_file}" "${pg_database}"
  else
    pg_dump -Fc --no-owner --no-privileges -f "${dump_file}"
  fi

  echo "included pg_dump: database-dumps/${dump_name}-${timestamp}.dump" >> "${manifest}"
}

for ((i = 0; i < item_count; i++)); do
  [[ "$#" -ge 3 ]] || die_remote "invalid remote item arguments"
  add_remote_dir "$1" "$2" "$3"
  shift 3
done

run_pg_dump_if_requested

tar -czf - -C "${stage_parent}" "${timestamp}"
REMOTE_SCRIPT
}

sync_remote_data() {
  local download_archive

  mkdir -p "${OUT_ROOT_ABS}"
  if [[ -e "${BACKUP_DIR}" ]]; then
    die "sync directory already exists: ${BACKUP_DIR}"
  fi
  if [[ "${CREATE_ARCHIVE}" == "1" && -e "${ARCHIVE_PATH}" ]]; then
    die "archive already exists: ${ARCHIVE_PATH}"
  fi

  TMP_DIR="$(mktemp -d "${OUT_ROOT_ABS}/.${TIMESTAMP}.sync.XXXXXX")"
  download_archive="${TMP_DIR}/${TIMESTAMP}.remote.tar.gz"

  echo "远程打包并下载: ${HOST}:${REMOTE_ROOT} -> ${download_archive}"
  download_remote_archive "${download_archive}"

  echo "解包到本地目录: ${BACKUP_DIR}"
  tar -xzf "${download_archive}" -C "${OUT_ROOT_ABS}"
  [[ -f "${BACKUP_DIR}/manifest.txt" ]] || die "remote archive did not contain manifest"

  if [[ "${CREATE_ARCHIVE}" == "1" ]]; then
    echo "生成本地归档: ${ARCHIVE_PATH}"
    tar -czf "${ARCHIVE_PATH}" -C "${OUT_ROOT_ABS}" "${TIMESTAMP}"
  fi
}

while [[ "$#" -gt 0 ]]; do
  case "$1" in
    --host)
      [[ "$#" -ge 2 ]] || die "--host requires a value"
      HOST="$2"
      shift 2
      ;;
    --remote-root)
      [[ "$#" -ge 2 ]] || die "--remote-root requires a value"
      REMOTE_ROOT="$2"
      shift 2
      ;;
    --out-root)
      [[ "$#" -ge 2 ]] || die "--out-root requires a value"
      OUT_ROOT="$2"
      shift 2
      ;;
    --timestamp)
      [[ "$#" -ge 2 ]] || die "--timestamp requires a value"
      TIMESTAMP="$2"
      shift 2
      ;;
    --no-runtime)
      INCLUDE_RUNTIME=0
      shift
      ;;
    --no-existing-db-backups)
      INCLUDE_EXISTING_DB_BACKUPS=0
      shift
      ;;
    --db-backup-dir)
      [[ "$#" -ge 2 ]] || die "--db-backup-dir requires a value"
      CUSTOM_DB_BACKUP_SPECS+=("$2")
      shift 2
      ;;
    --remote-dir)
      [[ "$#" -ge 2 ]] || die "--remote-dir requires a value"
      CUSTOM_REMOTE_DIR_SPECS+=("$2")
      shift 2
      ;;
    --pg-dump)
      CREATE_PG_DUMP=1
      shift
      ;;
    --pg-database)
      [[ "$#" -ge 2 ]] || die "--pg-database requires a value"
      PG_DATABASE="$2"
      shift 2
      ;;
    --pg-url-env)
      [[ "$#" -ge 2 ]] || die "--pg-url-env requires a value"
      PG_URL_ENV="$2"
      shift 2
      ;;
    --pg-env-file)
      [[ "$#" -ge 2 ]] || die "--pg-env-file requires a value"
      PG_ENV_FILE="$2"
      shift 2
      ;;
    --no-archive)
      CREATE_ARCHIVE=0
      shift
      ;;
    --dry-run)
      DRY_RUN=1
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

validate_timestamp
build_remote_items

if [[ -n "${PG_URL_ENV}" && ! "${PG_URL_ENV}" =~ ^[A-Za-z_][A-Za-z0-9_]*$ ]]; then
  die "--pg-url-env must be a valid shell environment variable name"
fi

if [[ "${CREATE_PG_DUMP}" != "1" && -n "${PG_ENV_FILE}" ]]; then
  die "--pg-env-file requires --pg-dump"
fi

if [[ "${CREATE_PG_DUMP}" != "1" && -n "${PG_DATABASE}" ]]; then
  die "--pg-database requires --pg-dump"
fi

if [[ "${CREATE_PG_DUMP}" != "1" && -n "${PG_URL_ENV}" ]]; then
  die "--pg-url-env requires --pg-dump"
fi

if [[ "${CREATE_PG_DUMP}" == "1" && -n "${PG_DATABASE}" && -n "${PG_URL_ENV}" ]]; then
  die "--pg-database and --pg-url-env are mutually exclusive"
fi

if [[ "${DRY_RUN}" == "1" ]]; then
  print_plan
  exit 0
fi

require_command ssh
require_command tar
require_command date
require_command mktemp

OUT_ROOT_ABS="$(absolute_out_root)"
BACKUP_DIR="${OUT_ROOT_ABS}/${TIMESTAMP}"
ARCHIVE_PATH="${OUT_ROOT_ABS}/${TIMESTAMP}.tar.gz"
REMOTE_ROOT_QUOTED="$(shell_quote "${REMOTE_ROOT}")"
trap cleanup_tmp_dir EXIT

echo "检查远程根目录: ${HOST}:${REMOTE_ROOT}"
ssh "${HOST}" "test -d ${REMOTE_ROOT_QUOTED}" || die "remote root is not readable: ${HOST}:${REMOTE_ROOT}"

sync_remote_data

echo
echo "同步完成:"
echo "  目录: ${BACKUP_DIR}"
if [[ "${CREATE_ARCHIVE}" == "1" ]]; then
  echo "  归档: ${ARCHIVE_PATH}"
fi
echo
echo "本地检查示例:"
echo "  sed -n '1,120p' \"${BACKUP_DIR}/manifest.txt\""
echo "  find \"${BACKUP_DIR}\" -maxdepth 3 -type f | sort | head -200"
echo "  grep -R \"error\\|failed\\|degraded\\|blocking\" \"${BACKUP_DIR}/runtime\" | head -200"
