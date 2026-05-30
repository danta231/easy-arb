#!/usr/bin/env bash
set -euo pipefail

# 中文说明：释放 funding-arb resident 的 unknown 仓位门禁。
# 该脚本只追加审计 JSONL，不查询交易所、不下单、不撤单。使用前必须先在
# 交易所后台或私有只读快照确认相关订单、挂单和仓位已经无风险。

usage() {
  cat <<'USAGE'
用法:
  scripts/release-funding-arb-unknown-position.sh \
    --position-id id \
    --confirmed-flat \
    [--resident-root dir] [--order-id id] [--symbol symbol] [--pair-id id] [--reason text] [--dry-run]

必须条件:
  --confirmed-flat 表示你已经人工确认交易所侧无真实仓位、无挂单、无残余风险。

默认 resident root:
  target/arb-runtime/live/resident-live/cross-exchange-funding-arb

示例:
  scripts/release-funding-arb-unknown-position.sh \
    --position-id pos:funding-arb:manual-reconcile:binance-bybit-chipusdt:1 \
    --order-id 2d8fc08c-0f64-4fa5-8340-2583892bcbd8 \
    --confirmed-flat \
    --dry-run
USAGE
}

die() {
  echo "error: $*" >&2
  exit 1
}

require_command() {
  command -v "$1" >/dev/null 2>&1 || die "missing required command: $1"
}

refresh_funding_arb_summary_position_counts() {
  local resident_root="$1"
  local positions_path="${resident_root}/funding_arb_resident_positions.jsonl"
  local summary_path="${resident_root}/funding_arb_resident_live_summary.json"
  local counts_json
  local tmp

  [[ -s "${positions_path}" && -s "${summary_path}" ]] || return 0

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

RESIDENT_ROOT="${ARB_RUNTIME_FUNDING_ARB_RESIDENT_ROOT:-target/arb-runtime/live/resident-live/cross-exchange-funding-arb}"
POSITION_ID=""
ORDER_ID=""
SYMBOL=""
PAIR_ID=""
REASON=""
CONFIRMED_FLAT=0
DRY_RUN=0

while [[ "$#" -gt 0 ]]; do
  case "$1" in
    --resident-root)
      [[ "$#" -ge 2 ]] || die "--resident-root requires a directory"
      RESIDENT_ROOT="$2"
      shift 2
      ;;
    --position-id)
      [[ "$#" -ge 2 ]] || die "--position-id requires a value"
      POSITION_ID="$2"
      shift 2
      ;;
    --order-id)
      [[ "$#" -ge 2 ]] || die "--order-id requires a value"
      ORDER_ID="$2"
      shift 2
      ;;
    --symbol)
      [[ "$#" -ge 2 ]] || die "--symbol requires a value"
      SYMBOL="$2"
      shift 2
      ;;
    --pair-id)
      [[ "$#" -ge 2 ]] || die "--pair-id requires a value"
      PAIR_ID="$2"
      shift 2
      ;;
    --reason)
      [[ "$#" -ge 2 ]] || die "--reason requires a value"
      REASON="$2"
      shift 2
      ;;
    --confirmed-flat|--exchange-confirmed-flat)
      CONFIRMED_FLAT=1
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
      die "unexpected positional argument: $1"
      ;;
  esac
done

require_command jq

[[ -n "${POSITION_ID}" ]] || die "--position-id is required"
[[ "${CONFIRMED_FLAT}" == "1" ]] || die "--confirmed-flat is required after exchange-side reconciliation"

POSITIONS_PATH="${RESIDENT_ROOT}/funding_arb_resident_positions.jsonl"
EVENTS_PATH="${RESIDENT_ROOT}/funding_arb_resident_live_events.jsonl"

[[ -f "${POSITIONS_PATH}" ]] || die "positions file not found: ${POSITIONS_PATH}"
[[ -d "${RESIDENT_ROOT}" ]] || die "resident root not found: ${RESIDENT_ROOT}"

last_event_type="$(
  jq -s -r --arg position_id "${POSITION_ID}" '
    [.[] | select(.position_id == $position_id)]
    | if length == 0 then "missing" else .[-1].event_type end
  ' "${POSITIONS_PATH}"
)"

case "${last_event_type}" in
  position_unknown) ;;
  position_closed)
    refresh_funding_arb_summary_position_counts "${RESIDENT_ROOT}"
    echo "already_closed position_id=${POSITION_ID}"
    exit 0
    ;;
  missing)
    die "position_id not found in ${POSITIONS_PATH}: ${POSITION_ID}"
    ;;
  *)
    die "latest event for ${POSITION_ID} is ${last_event_type}; expected position_unknown"
    ;;
esac

unknown_record="$(
  jq -s -c --arg position_id "${POSITION_ID}" '
    [.[] | select(.position_id == $position_id)]
    | .[-1]
  ' "${POSITIONS_PATH}"
)"

if [[ -z "${SYMBOL}" ]]; then
  SYMBOL="$(jq -r '.symbol // "unknown"' <<<"${unknown_record}")"
fi
if [[ -z "${PAIR_ID}" ]]; then
  PAIR_ID="$(jq -r '.pair_id // "unknown"' <<<"${unknown_record}")"
fi
if [[ -z "${REASON}" ]]; then
  REASON="manual reconciliation confirmed exchange order, open orders, and position are flat"
fi
NOTIONAL_USDT="$(jq -r '.notional_usdt // "unknown"' <<<"${unknown_record}")"
POSITION_STATE_PATH="$(jq -r '.position_state_path // ""' <<<"${unknown_record}")"
NET_FUNDING_BPS="$(jq -r '.net_funding_bps // ""' <<<"${unknown_record}")"
OPENED_AT="$(jq -r '.opened_at // ""' <<<"${unknown_record}")"

confirmed_at="$(date -u +"%Y-%m-%dT%H:%M:%SZ")"
line="$(
  jq -cn \
    --arg event_type "position_closed" \
    --arg position_id "${POSITION_ID}" \
    --arg pair_id "${PAIR_ID}" \
    --arg symbol "${SYMBOL}" \
    --arg notional_usdt "${NOTIONAL_USDT}" \
    --arg position_state_path "${POSITION_STATE_PATH}" \
    --arg net_funding_bps "${NET_FUNDING_BPS}" \
    --arg opened_at "${OPENED_AT}" \
    --arg order_id "${ORDER_ID}" \
    --arg reason "${REASON}" \
    --arg confirmed_at "${confirmed_at}" \
    '{
      closed_at: $confirmed_at,
      cycle: null,
      cycle_dir: null,
      decision: "manual_reconciliation",
      event_type: $event_type,
      net_funding_bps: (if $net_funding_bps == "" then null else $net_funding_bps end),
      notional_usdt: $notional_usdt,
      opened_at: (if $opened_at == "" then null else $opened_at end),
      position_id: $position_id,
      position_state_path: (if $position_state_path == "" then null else $position_state_path end),
      pair_id: $pair_id,
      private_confirmation_count: 0,
      symbol: $symbol,
      status: "closed",
      reason: $reason,
      residual_risk: null,
      submitted_receipt_count: 0,
      manual_reconciliation: true,
      exchange_confirmed_flat: true,
      acknowledged_residual_risk: true,
      confirmed_at: $confirmed_at
    }
    + (if $order_id == "" then {} else {order_id: $order_id} end)'
)"

if [[ "${DRY_RUN}" == "1" ]]; then
  echo "dry_run_release_record ${line}"
  exit 0
fi

printf '%s\n' "${line}" >> "${POSITIONS_PATH}"
printf '%s\n' "${line}" >> "${EVENTS_PATH}"
refresh_funding_arb_summary_position_counts "${RESIDENT_ROOT}"
echo "released_unknown_position position_id=${POSITION_ID} positions=${POSITIONS_PATH}"
