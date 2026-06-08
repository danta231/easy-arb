use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Read;
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::{Arc, RwLock};
use std::thread;
use std::time::Duration;

use crate::monitors::dashboard::system_navigation_pages_json_with_wss_pid_file;
use crate::*;

const PORTFOLIO_FUNDING_SETTLEMENT_GRACE_MS: u64 = 5 * 60 * 1000;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct PortfolioDashboardSnapshot {
    pub(crate) status: String,
    pub(crate) updated_at: String,
    pub(crate) balances: Vec<PortfolioBalanceRow>,
    pub(crate) positions: Vec<PortfolioPositionRow>,
    pub(crate) source_errors: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct PortfolioDashboardCacheState {
    pub(crate) snapshot: Option<PortfolioDashboardSnapshot>,
    pub(crate) cache_refreshed_at: String,
    pub(crate) refresh_error: Option<String>,
}

pub(crate) struct PortfolioDashboardHttpState {
    pub(crate) options: Arc<PortfolioDashboardOptions>,
    pub(crate) cache: Arc<RwLock<PortfolioDashboardCacheState>>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ErrorLogsSnapshot {
    pub(crate) status: String,
    pub(crate) updated_at: String,
    pub(crate) entries: Vec<ErrorLogEntry>,
    pub(crate) source_errors: Vec<String>,
    pub(crate) source_notices: Vec<String>,
    pub(crate) source_count: usize,
    pub(crate) skipped_line_count: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ErrorLogEntry {
    pub(crate) observed_at: Option<String>,
    pub(crate) severity: String,
    pub(crate) category: String,
    pub(crate) source: String,
    pub(crate) strategy: Option<String>,
    pub(crate) pair_id: Option<String>,
    pub(crate) symbol: Option<String>,
    pub(crate) message: String,
    pub(crate) path: String,
    pub(crate) line_number: usize,
    pub(crate) raw: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct PortfolioBalanceRow {
    pub(crate) venue_family: String,
    pub(crate) account_id: String,
    pub(crate) asset: String,
    pub(crate) available: Option<String>,
    pub(crate) margin_balance: Option<String>,
    pub(crate) maintenance_margin: Option<String>,
    pub(crate) margin_buffer: Option<String>,
    pub(crate) free: Option<String>,
    pub(crate) locked: Option<String>,
    pub(crate) reserved: Option<String>,
    pub(crate) pending: Option<String>,
    pub(crate) borrowed: Option<String>,
    pub(crate) lent: Option<String>,
    pub(crate) unsettled: Option<String>,
    pub(crate) source_status: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct PortfolioPositionRow {
    pub(crate) position_id: Option<String>,
    pub(crate) position_kind: Option<String>,
    pub(crate) coin: String,
    pub(crate) symbol: String,
    pub(crate) strategy: String,
    pub(crate) venue_family: String,
    pub(crate) account_id: String,
    pub(crate) fee: Option<String>,
    pub(crate) fee_rate_bps: Option<String>,
    pub(crate) settled_funding_usd: Option<String>,
    pub(crate) accumulated_position: Option<String>,
    pub(crate) open_average_price: Option<String>,
    pub(crate) close_average_price: Option<String>,
    pub(crate) open_close_spread_pct: Option<String>,
    pub(crate) realtime_funding_rate: Option<String>,
    pub(crate) realtime_funding_interval_hours: Option<String>,
    pub(crate) funding_settlement_time: Option<String>,
    pub(crate) opened_at: Option<String>,
    pub(crate) closed_at: Option<String>,
    pub(crate) open_close_condition: Option<String>,
    pub(crate) position_status: String,
    pub(crate) position_quantity: String,
    pub(crate) position_group_id: Option<String>,
    pub(crate) position_group_label: Option<String>,
    pub(crate) position_leg_role: Option<String>,
    pub(crate) position_limit: Option<String>,
    pub(crate) manual_close_request_id: Option<String>,
    pub(crate) manual_close_request_status: Option<String>,
    pub(crate) manual_close_request_detail: Option<String>,
    pub(crate) source: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct PortfolioFundingContext {
    pub(crate) venue_family: String,
    pub(crate) symbol: String,
    pub(crate) funding_rate: String,
    pub(crate) next_funding_time_ms: String,
    pub(crate) funding_interval_hours: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PortfolioResidentPositionKind {
    SpotPerpBasis,
    CrossExchangeFundingArb,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct PortfolioResidentRegistryDir {
    pub(crate) path: PathBuf,
    pub(crate) kind: PortfolioResidentPositionKind,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct PortfolioResidentPositionRef {
    pub(crate) kind: PortfolioResidentPositionKind,
    pub(crate) position_id: String,
    pub(crate) position_state_path: PathBuf,
    pub(crate) pair_id: Option<String>,
    pub(crate) symbol: Option<String>,
    pub(crate) notional_usdt: String,
    pub(crate) status: String,
    pub(crate) net_funding_bps: Option<String>,
    pub(crate) taker_fee_bps: Option<String>,
    pub(crate) position_limit: Option<String>,
    pub(crate) opened_at: Option<String>,
    pub(crate) closed_at: Option<String>,
    pub(crate) manual_close_request_id: Option<String>,
    pub(crate) manual_close_request_status: Option<String>,
    pub(crate) manual_close_request_detail: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct PortfolioFundingArbLegDisplay {
    pub(crate) role: String,
    pub(crate) venue_family: String,
    pub(crate) account_id: String,
    pub(crate) side: String,
    pub(crate) quantity: String,
    pub(crate) entry_limit_price: String,
}

/// 启动组合看板。
///
/// 中文说明：默认只读取本地文件和 resident live 注册表；启用
/// `--enable-manual-close` 后仅追加 funding-arb 手动平仓请求，真实下单仍由
/// resident live 进程完成。该入口不会访问交易所私有接口，不读取凭证，不直接提交订单。
pub fn run_portfolio_dashboard(
    options: PortfolioDashboardOptions,
) -> RuntimeResult<PortfolioDashboardReport> {
    validate_portfolio_dashboard_options(&options)?;
    if options.once {
        let snapshot = build_portfolio_dashboard_snapshot(&options)?;
        return Ok(PortfolioDashboardReport {
            bind_addr: options.bind_addr,
            status: snapshot.status,
            balance_count: snapshot.balances.len(),
            position_count: snapshot.positions.len(),
            source_error_count: snapshot.source_errors.len(),
        });
    }

    let bind_addr = options.bind_addr.clone();
    let state = Arc::new(options);
    let _handle = start_portfolio_dashboard_http_api(&bind_addr, Arc::clone(&state))?;
    loop {
        thread::sleep(Duration::from_secs(3600));
    }
}

pub(crate) fn validate_portfolio_dashboard_options(
    options: &PortfolioDashboardOptions,
) -> RuntimeResult<()> {
    if options.bind_addr.trim().is_empty() {
        return Err(cli_arg_error(
            "portfolio-dashboard --bind must not be empty",
        ));
    }
    Ok(())
}

pub(crate) fn build_portfolio_dashboard_snapshot(
    options: &PortfolioDashboardOptions,
) -> RuntimeResult<PortfolioDashboardSnapshot> {
    let mut balances = Vec::new();
    let mut positions = Vec::new();
    let mut source_errors = Vec::new();
    let mut source_timestamps = Vec::new();

    load_portfolio_account_sources(
        options,
        &mut balances,
        &mut source_timestamps,
        &mut source_errors,
    );
    let funding_contexts =
        load_portfolio_funding_contexts(options, &mut source_timestamps, &mut source_errors);
    load_portfolio_position_sources(
        options,
        &funding_contexts,
        &mut positions,
        &mut source_timestamps,
        &mut source_errors,
    );
    load_portfolio_funding_settlement_sources(
        options,
        &mut positions,
        &mut source_timestamps,
        &mut source_errors,
    );

    let status = if balances.is_empty() && positions.is_empty() {
        "missing"
    } else if source_errors.is_empty() {
        "healthy"
    } else {
        "degraded"
    }
    .to_owned();
    let updated_at = source_timestamps
        .into_iter()
        .max()
        .unwrap_or_else(current_utc_timestamp_string);

    Ok(PortfolioDashboardSnapshot {
        status,
        updated_at,
        balances,
        positions,
        source_errors,
    })
}

const PORTFOLIO_DASHBOARD_CACHE_REFRESH_SECS: u64 = 15;

pub(crate) fn initial_portfolio_dashboard_cache_state() -> PortfolioDashboardCacheState {
    PortfolioDashboardCacheState {
        snapshot: None,
        cache_refreshed_at: current_utc_timestamp_string(),
        refresh_error: Some("组合看板快照后台刷新尚未完成".to_owned()),
    }
}

pub(crate) fn refresh_portfolio_dashboard_cache_once(
    options: &PortfolioDashboardOptions,
    cache: &RwLock<PortfolioDashboardCacheState>,
) {
    let refreshed_at = current_utc_timestamp_string();
    let next_state = match build_portfolio_dashboard_snapshot(options) {
        Ok(snapshot) => PortfolioDashboardCacheState {
            snapshot: Some(snapshot),
            cache_refreshed_at: refreshed_at,
            refresh_error: None,
        },
        Err(error) => {
            let previous_snapshot = cache
                .read()
                .expect("portfolio dashboard cache lock poisoned")
                .snapshot
                .clone();
            PortfolioDashboardCacheState {
                snapshot: previous_snapshot,
                cache_refreshed_at: refreshed_at,
                refresh_error: Some(error.to_string()),
            }
        }
    };
    *cache
        .write()
        .expect("portfolio dashboard cache lock poisoned") = next_state;
}

fn start_portfolio_dashboard_cache_refresher(
    options: Arc<PortfolioDashboardOptions>,
    cache: Arc<RwLock<PortfolioDashboardCacheState>>,
) -> thread::JoinHandle<()> {
    thread::spawn(move || loop {
        refresh_portfolio_dashboard_cache_once(options.as_ref(), cache.as_ref());
        thread::sleep(Duration::from_secs(PORTFOLIO_DASHBOARD_CACHE_REFRESH_SECS));
    })
}

pub(crate) fn portfolio_dashboard_snapshot_from_cache(
    cache: &PortfolioDashboardCacheState,
) -> PortfolioDashboardSnapshot {
    let mut snapshot = cache
        .snapshot
        .clone()
        .unwrap_or_else(|| PortfolioDashboardSnapshot {
            status: "loading".to_owned(),
            updated_at: cache.cache_refreshed_at.clone(),
            balances: Vec::new(),
            positions: Vec::new(),
            source_errors: Vec::new(),
        });
    if let Some(error) = &cache.refresh_error {
        snapshot.status = if snapshot.snapshot_has_rows() {
            "degraded".to_owned()
        } else {
            "loading".to_owned()
        };
        snapshot.source_errors.push(format!(
            "组合看板缓存刷新失败，刷新时间 {}：{error}",
            cache.cache_refreshed_at
        ));
    }
    snapshot
}

impl PortfolioDashboardSnapshot {
    fn snapshot_has_rows(&self) -> bool {
        !self.balances.is_empty() || !self.positions.is_empty()
    }
}

pub(crate) fn portfolio_dashboard_health_json_from_cache(
    cache: &PortfolioDashboardCacheState,
) -> String {
    let snapshot = portfolio_dashboard_snapshot_from_cache(cache);
    format!(
        "{{\"balance_count\":{},\"cache_refreshed_at\":{},\"position_count\":{},\"source_error_count\":{},\"status\":{},\"updated_at\":{}}}",
        snapshot.balances.len(),
        json_string(&cache.cache_refreshed_at),
        snapshot.positions.len(),
        snapshot.source_errors.len(),
        json_string(&snapshot.status),
        json_string(&snapshot.updated_at)
    )
}

fn load_portfolio_account_sources(
    options: &PortfolioDashboardOptions,
    balances: &mut Vec<PortfolioBalanceRow>,
    source_timestamps: &mut Vec<String>,
    source_errors: &mut Vec<String>,
) {
    let mut loaded = false;
    if let Some(path) = &options.account_snapshot_path {
        loaded = true;
        match read_utf8(path).and_then(|json| portfolio_balance_rows_from_snapshot_json(&json)) {
            Ok((updated_at, mut rows)) => {
                if let Some(updated_at) = updated_at {
                    source_timestamps.push(updated_at);
                }
                balances.append(&mut rows);
            }
            Err(error) => source_errors.push(format!(
                "account snapshot `{}` failed: {error}",
                path.display()
            )),
        }
    }
    if let Some(path) = &options.account_raw_snapshot_path {
        loaded = true;
        match read_utf8(path)
            .and_then(|json| portfolio_balance_rows_from_raw_snapshot_json_with_errors(&json))
        {
            Ok((updated_at, mut rows, mut errors)) => {
                if let Some(updated_at) = updated_at {
                    source_timestamps.push(updated_at);
                }
                balances.append(&mut rows);
                source_errors.append(&mut errors);
            }
            Err(error) => source_errors.push(format!(
                "account raw snapshot `{}` failed: {error}",
                path.display()
            )),
        }
    }
    if let Some(path) = &options.resident_root {
        loaded = true;
        match portfolio_balance_rows_from_resident_root(path) {
            Ok((updated_at, mut rows, mut errors)) => {
                if let Some(updated_at) = updated_at {
                    source_timestamps.push(updated_at);
                }
                balances.append(&mut rows);
                source_errors.append(&mut errors);
            }
            Err(error) => source_errors.push(format!(
                "resident root account snapshots `{}` failed: {error}",
                path.display()
            )),
        }
    }
    if !loaded {
        source_errors.push("account snapshot source was not provided".to_owned());
    }
}

fn load_portfolio_funding_contexts(
    options: &PortfolioDashboardOptions,
    source_timestamps: &mut Vec<String>,
    source_errors: &mut Vec<String>,
) -> Vec<PortfolioFundingContext> {
    let Some(path) = options
        .funding_snapshot_path
        .clone()
        .or_else(|| portfolio_default_funding_snapshot_path(options))
    else {
        return Vec::new();
    };
    match read_utf8(&path).and_then(|json| portfolio_funding_contexts_from_snapshot_json(&json)) {
        Ok((updated_at, contexts)) => {
            if let Some(updated_at) = updated_at {
                source_timestamps.push(updated_at);
            }
            contexts
        }
        Err(error) => {
            source_errors.push(format!(
                "funding snapshot `{}` failed: {error}",
                path.display()
            ));
            Vec::new()
        }
    }
}

pub(crate) fn portfolio_default_funding_snapshot_path(
    options: &PortfolioDashboardOptions,
) -> Option<PathBuf> {
    let root = options.resident_root.as_ref()?;
    [
        root.join("snapshots/funding-arb/funding_arb_monitor_snapshot.json"),
        root.join("funding_arb_monitor_snapshot.json"),
        root.join("resident-live/cross-exchange-funding-arb/funding_arb_monitor_snapshot.json"),
    ]
    .into_iter()
    .find(|path| path.exists())
}

fn load_portfolio_position_sources(
    options: &PortfolioDashboardOptions,
    funding_contexts: &[PortfolioFundingContext],
    positions: &mut Vec<PortfolioPositionRow>,
    source_timestamps: &mut Vec<String>,
    source_errors: &mut Vec<String>,
) {
    let mut loaded = false;
    if let Some(path) = &options.position_snapshot_path {
        loaded = true;
        match read_utf8(path)
            .and_then(|json| portfolio_position_rows_from_snapshot_json(&json, funding_contexts))
        {
            Ok((updated_at, mut rows)) => {
                if let Some(updated_at) = updated_at {
                    source_timestamps.push(updated_at);
                }
                positions.append(&mut rows);
            }
            Err(error) => source_errors.push(format!(
                "position snapshot `{}` failed: {error}",
                path.display()
            )),
        }
    }
    if let Some(path) = &options.position_raw_snapshot_path {
        loaded = true;
        match read_utf8(path).and_then(|json| {
            portfolio_position_rows_from_raw_snapshot_json(&json, funding_contexts)
        }) {
            Ok((updated_at, mut rows)) => {
                if let Some(updated_at) = updated_at {
                    source_timestamps.push(updated_at);
                }
                positions.append(&mut rows);
            }
            Err(error) => source_errors.push(format!(
                "position raw snapshot `{}` failed: {error}",
                path.display()
            )),
        }
    }
    if let Some(path) = &options.resident_root {
        loaded = true;
        match portfolio_position_rows_from_resident_root(path, funding_contexts) {
            Ok((updated_at, mut rows)) => {
                if let Some(updated_at) = updated_at {
                    source_timestamps.push(updated_at);
                }
                positions.append(&mut rows);
            }
            Err(error) => source_errors.push(format!(
                "resident root `{}` failed: {error}",
                path.display()
            )),
        }
    }
    if !loaded {
        source_errors.push("position snapshot source was not provided".to_owned());
    }
}

fn load_portfolio_funding_settlement_sources(
    options: &PortfolioDashboardOptions,
    positions: &mut [PortfolioPositionRow],
    source_timestamps: &mut Vec<String>,
    source_errors: &mut Vec<String>,
) {
    let Some(root) = &options.resident_root else {
        return;
    };
    match portfolio_funding_settlement_entries_from_resident_root(root) {
        Ok(Some((updated_at, entries))) => {
            if let Some(updated_at) = updated_at {
                source_timestamps.push(updated_at);
            }
            if let Err(error) = portfolio_apply_funding_settlement_entries(positions, &entries) {
                source_errors.push(format!("funding settlement entries failed: {error}"));
            }
        }
        Ok(None) => {}
        Err(error) => source_errors.push(format!(
            "resident root funding settlement snapshots `{}` failed: {error}",
            root.display()
        )),
    }
}

pub(crate) fn portfolio_funding_settlement_entries_from_resident_root(
    root: &Path,
) -> RuntimeResult<Option<(Option<String>, Vec<FundingSettlementLedgerEntry>)>> {
    let mut paths = Vec::new();
    for path in [
        root.join("private-readonly/funding_settlement_raw_snapshot.json"),
        root.join("portfolio-private-readonly/funding_settlement_raw_snapshot.json"),
    ] {
        if path.exists() {
            paths.push(path);
        }
    }
    portfolio_collect_named_files(root, "funding_settlement_raw_snapshot.json", 8, &mut paths)?;
    portfolio_collect_named_files(
        root,
        "funding_arb_funding_settlement_raw_snapshot.json",
        8,
        &mut paths,
    )?;
    paths.sort();
    paths.dedup();
    if paths.is_empty() {
        return Ok(None);
    }

    let mut latest_updated_at = None::<String>;
    let mut entries_by_key = BTreeMap::<String, FundingSettlementLedgerEntry>::new();
    for path in paths {
        let input = read_utf8(&path)?;
        let snapshot = parse_funding_settlement_raw_snapshot_json(&input)?;
        let entries = funding_settlement_entries_from_raw_snapshot(&snapshot)?;
        if let Some(updated_at) = snapshot.updated_at {
            if latest_updated_at
                .as_ref()
                .is_none_or(|current| &updated_at > current)
            {
                latest_updated_at = Some(updated_at);
            }
        }
        for entry in entries {
            entries_by_key
                .entry(portfolio_funding_settlement_entry_dedupe_key(&entry))
                .or_insert(entry);
        }
    }
    Ok(Some((
        latest_updated_at,
        entries_by_key.into_values().collect(),
    )))
}

pub(crate) fn portfolio_funding_settlement_entry_dedupe_key(
    entry: &FundingSettlementLedgerEntry,
) -> String {
    format!(
        "{}\u{1f}{}\u{1f}{}\u{1f}{}\u{1f}{}",
        normalize_venue_family(&entry.venue_family),
        funding_settlement_display_symbol_from_raw(&entry.symbol),
        entry.account_id,
        entry
            .timestamp_ms
            .map(|timestamp| timestamp.to_string())
            .unwrap_or_default(),
        entry.amount_usd.trim_start_matches('+')
    )
}

pub(crate) fn portfolio_apply_funding_settlement_entries(
    rows: &mut [PortfolioPositionRow],
    entries: &[FundingSettlementLedgerEntry],
) -> RuntimeResult<()> {
    for row in rows {
        let mut total = MonitorDecimal { raw: 0 };
        let mut matched = false;
        for entry in entries
            .iter()
            .filter(|entry| portfolio_position_row_matches_funding_settlement(row, entry))
        {
            let amount = MonitorDecimal::parse(
                "portfolio.settled_funding_usd",
                entry.amount_usd.trim_start_matches('+'),
            )?;
            total = total.checked_add(amount, "portfolio settled funding sum")?;
            matched = true;
        }
        if matched {
            row.settled_funding_usd = Some(total.format_trimmed());
        }
    }
    Ok(())
}

pub(crate) fn portfolio_position_row_matches_funding_settlement(
    row: &PortfolioPositionRow,
    entry: &FundingSettlementLedgerEntry,
) -> bool {
    if funding_settlement_display_symbol_from_raw(&row.symbol)
        != funding_settlement_display_symbol_from_raw(&entry.symbol)
    {
        return false;
    }
    let entry_venue = normalize_venue_family(&entry.venue_family);
    let venue_matches = row
        .venue_family
        .split('/')
        .map(normalize_venue_family)
        .any(|venue| venue == entry_venue)
        || normalize_venue_family(&row.venue_family) == entry_venue;
    if !venue_matches {
        return false;
    }
    if !row
        .account_id
        .split('/')
        .map(str::trim)
        .any(|account_id| account_id == entry.account_id)
    {
        return false;
    }

    portfolio_position_row_matches_funding_settlement_window(row, entry)
}

pub(crate) fn portfolio_position_row_matches_funding_settlement_window(
    row: &PortfolioPositionRow,
    entry: &FundingSettlementLedgerEntry,
) -> bool {
    let opened_at_ms = row
        .opened_at
        .as_deref()
        .and_then(portfolio_timestamp_ms_from_value);
    let settlement_ms = row
        .funding_settlement_time
        .as_deref()
        .and_then(portfolio_max_timestamp_ms_from_value);
    let requires_window = row.position_group_id.is_some() || opened_at_ms.is_some();

    if !requires_window {
        return true;
    }

    let Some(entry_ms) = entry.timestamp_ms else {
        return false;
    };

    if opened_at_ms.is_some_and(|opened_at_ms| entry_ms < opened_at_ms) {
        return false;
    }
    if settlement_ms.is_some_and(|settlement_ms| {
        entry_ms > settlement_ms.saturating_add(PORTFOLIO_FUNDING_SETTLEMENT_GRACE_MS)
    }) {
        return false;
    }

    true
}

pub(crate) fn portfolio_timestamp_ms_from_value(value: &str) -> Option<u64> {
    let raw = value.trim();
    if raw.is_empty() || raw == "-" {
        return None;
    }
    if raw.chars().all(|ch| ch.is_ascii_digit()) {
        return match raw.len() {
            13 => raw.parse::<u64>().ok(),
            10 => raw
                .parse::<u64>()
                .ok()
                .and_then(|seconds| seconds.checked_mul(1000)),
            _ => None,
        };
    }
    UtcTimestamp::from_str(raw)
        .ok()
        .and_then(|timestamp| runtime_timestamp_millis(timestamp).ok())
        .and_then(|timestamp_ms| u64::try_from(timestamp_ms).ok())
}

pub(crate) fn portfolio_max_timestamp_ms_from_value(value: &str) -> Option<u64> {
    let mut timestamps = Vec::new();
    let mut start = None::<usize>;
    for (index, ch) in value.char_indices() {
        if ch.is_ascii_digit() {
            if start.is_none() {
                start = Some(index);
            }
        } else if let Some(start_index) = start.take() {
            if let Some(timestamp) = portfolio_timestamp_ms_from_value(&value[start_index..index]) {
                timestamps.push(timestamp);
            }
        }
    }
    if let Some(start_index) = start {
        if let Some(timestamp) = portfolio_timestamp_ms_from_value(&value[start_index..]) {
            timestamps.push(timestamp);
        }
    }
    timestamps
        .into_iter()
        .max()
        .or_else(|| portfolio_timestamp_ms_from_value(value))
}

pub(crate) fn portfolio_dashboard_snapshot_json(snapshot: &PortfolioDashboardSnapshot) -> String {
    format!(
        "{{\"balance_count\":{},\"balances\":[{}],\"mutable_execution_started\":false,\"position_count\":{},\"positions\":[{}],\"source_error_count\":{},\"source_errors\":{},\"status\":{},\"updated_at\":{}}}",
        snapshot.balances.len(),
        snapshot
            .balances
            .iter()
            .map(portfolio_balance_row_json)
            .collect::<Vec<_>>()
            .join(","),
        snapshot.positions.len(),
        snapshot
            .positions
            .iter()
            .map(portfolio_position_row_json)
            .collect::<Vec<_>>()
            .join(","),
        snapshot.source_errors.len(),
        json_string_array(&snapshot.source_errors),
        json_string(&snapshot.status),
        json_string(&snapshot.updated_at),
    )
}

pub(crate) fn portfolio_balances_json(snapshot: &PortfolioDashboardSnapshot) -> String {
    format!(
        "{{\"balance_count\":{},\"balances\":[{}],\"status\":{},\"updated_at\":{}}}",
        snapshot.balances.len(),
        snapshot
            .balances
            .iter()
            .map(portfolio_balance_row_json)
            .collect::<Vec<_>>()
            .join(","),
        json_string(&snapshot.status),
        json_string(&snapshot.updated_at),
    )
}

pub(crate) fn portfolio_positions_json(snapshot: &PortfolioDashboardSnapshot) -> String {
    format!(
        "{{\"position_count\":{},\"positions\":[{}],\"status\":{},\"updated_at\":{}}}",
        snapshot.positions.len(),
        snapshot
            .positions
            .iter()
            .map(portfolio_position_row_json)
            .collect::<Vec<_>>()
            .join(","),
        json_string(&snapshot.status),
        json_string(&snapshot.updated_at),
    )
}

pub(crate) const ERROR_LOG_MAX_ENTRIES: usize = 10;
pub(crate) const ERROR_LOG_MAX_ENTRIES_PER_SOURCE: usize = 120;
pub(crate) const ERROR_LOG_MAX_SOURCE_FILES: usize = 80;
const ERROR_LOG_MESSAGE_CHAR_LIMIT: usize = 2_000;
const ERROR_LOG_RAW_CHAR_LIMIT: usize = 1_200;
const ERROR_LOG_SOURCE_ERRORS_LIMIT: usize = 40;
pub(crate) const ERROR_LOG_SOURCE_ERROR_CHAR_LIMIT: usize = 300;
pub(crate) const ERROR_LOG_SOURCE_TAIL_BYTES: u64 = 1_048_576;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ErrorLogSourceKind {
    Jsonl,
    Text,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ErrorLogSource {
    source: String,
    path: PathBuf,
    kind: ErrorLogSourceKind,
}

pub(crate) fn build_error_logs_snapshot(options: &PortfolioDashboardOptions) -> ErrorLogsSnapshot {
    let mut source_errors = Vec::new();
    let sources = limited_error_log_sources(
        portfolio_error_log_sources(options, &mut source_errors),
        &mut source_errors,
    );
    let source_count = sources.len();
    let mut entries = Vec::new();
    let mut skipped_line_count = 0usize;
    let mut source_notices = Vec::new();

    for source in sources {
        let result = match source.kind {
            ErrorLogSourceKind::Jsonl => append_jsonl_error_log_entries(
                &source,
                &mut entries,
                &mut skipped_line_count,
                &mut source_notices,
            ),
            ErrorLogSourceKind::Text => {
                append_text_error_log_entries(&source, &mut entries, &mut source_notices)
            }
        };
        if let Err(error) = result {
            source_errors.push(format!("{}: {error}", source.path.display()));
        }
    }

    entries.sort_by(|left, right| {
        error_log_timestamp_sort_key(right.observed_at.as_deref())
            .cmp(&error_log_timestamp_sort_key(left.observed_at.as_deref()))
            .then_with(|| {
                right
                    .observed_at
                    .as_deref()
                    .unwrap_or("")
                    .cmp(left.observed_at.as_deref().unwrap_or(""))
            })
            .then_with(|| right.path.cmp(&left.path))
            .then_with(|| right.line_number.cmp(&left.line_number))
    });
    if entries.len() > ERROR_LOG_MAX_ENTRIES {
        entries.truncate(ERROR_LOG_MAX_ENTRIES);
    }
    if source_errors.len() > ERROR_LOG_SOURCE_ERRORS_LIMIT {
        let omitted = source_errors.len() - ERROR_LOG_SOURCE_ERRORS_LIMIT;
        source_errors.truncate(ERROR_LOG_SOURCE_ERRORS_LIMIT);
        source_errors.push(format!(
            "omitted {omitted} additional error log source notices"
        ));
    }
    for source_error in &mut source_errors {
        *source_error = truncate_for_json(source_error, ERROR_LOG_SOURCE_ERROR_CHAR_LIMIT);
    }
    if source_notices.len() > ERROR_LOG_SOURCE_ERRORS_LIMIT {
        let omitted = source_notices.len() - ERROR_LOG_SOURCE_ERRORS_LIMIT;
        source_notices.truncate(ERROR_LOG_SOURCE_ERRORS_LIMIT);
        source_notices.push(format!(
            "omitted {omitted} additional error log source notices"
        ));
    }
    for source_notice in &mut source_notices {
        *source_notice = truncate_for_json(source_notice, ERROR_LOG_SOURCE_ERROR_CHAR_LIMIT);
    }
    let status = if source_count == 0 {
        "missing"
    } else if !source_errors.is_empty() {
        "degraded"
    } else if entries.is_empty() {
        "healthy"
    } else {
        "degraded"
    }
    .to_owned();

    ErrorLogsSnapshot {
        status,
        updated_at: current_utc_timestamp_string(),
        entries,
        source_errors,
        source_notices,
        source_count,
        skipped_line_count,
    }
}

fn limited_error_log_sources(
    sources: Vec<ErrorLogSource>,
    source_errors: &mut Vec<String>,
) -> Vec<ErrorLogSource> {
    if sources.len() <= ERROR_LOG_MAX_SOURCE_FILES {
        return sources;
    }
    let original_count = sources.len();
    let mut keyed_sources = sources
        .into_iter()
        .map(|source| {
            (
                error_log_source_sort_key(&source.path),
                source.path.clone(),
                source,
            )
        })
        .collect::<Vec<_>>();
    keyed_sources.sort_by(|left, right| right.0.cmp(&left.0).then_with(|| right.1.cmp(&left.1)));
    let sources = keyed_sources
        .into_iter()
        .take(ERROR_LOG_MAX_SOURCE_FILES)
        .map(|(_, _, source)| source)
        .collect::<Vec<_>>();
    source_errors.push(format!(
        "错误日志源过多：仅扫描最近 {ERROR_LOG_MAX_SOURCE_FILES} 个来源，跳过 {} 个旧来源",
        original_count - ERROR_LOG_MAX_SOURCE_FILES
    ));
    sources
}

fn error_log_source_sort_key(path: &Path) -> u128 {
    fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .ok()
        .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_millis())
        .unwrap_or(0)
}

pub(crate) fn portfolio_error_log_sources(
    options: &PortfolioDashboardOptions,
    source_errors: &mut Vec<String>,
) -> Vec<ErrorLogSource> {
    let mut sources = Vec::new();
    let mut seen = BTreeSet::new();
    let Some(run_root) = portfolio_error_log_run_root(options) else {
        return sources;
    };

    let logs_dir = run_root.join("logs");
    if logs_dir.is_dir() {
        match fs::read_dir(&logs_dir) {
            Ok(entries) => {
                for entry in entries {
                    let Ok(entry) = entry else {
                        continue;
                    };
                    let path = entry.path();
                    if !path.is_file() {
                        continue;
                    }
                    let Some(file_name) = path
                        .file_name()
                        .and_then(|value| value.to_str())
                        .map(str::to_owned)
                    else {
                        continue;
                    };
                    let kind = if file_name.ends_with(".jsonl") {
                        Some(ErrorLogSourceKind::Jsonl)
                    } else if file_name.ends_with(".log") {
                        Some(ErrorLogSourceKind::Text)
                    } else {
                        None
                    };
                    if let Some(kind) = kind {
                        push_error_log_source(
                            &mut sources,
                            &mut seen,
                            file_name
                                .trim_end_matches(".jsonl")
                                .trim_end_matches(".log"),
                            path,
                            kind,
                        );
                    }
                }
            }
            Err(error) => source_errors.push(format!("{}: {error}", logs_dir.display())),
        }
    }

    for (source, path) in [
        (
            "live-reports",
            run_root.join("live").join("live-reports.jsonl"),
        ),
        (
            "validation-events",
            run_root.join("live").join("validation-events.jsonl"),
        ),
        (
            "funding-arb-resident-live",
            run_root
                .join("resident-live")
                .join("cross-exchange-funding-arb")
                .join("funding_arb_resident_live_events.jsonl"),
        ),
        (
            "spot-perp-basis-resident-live",
            run_root
                .join("resident-live")
                .join("spot-perp-basis")
                .join("multi_venue_resident_live_events.jsonl"),
        ),
    ] {
        if path.exists() {
            push_error_log_source(
                &mut sources,
                &mut seen,
                source,
                path,
                ErrorLogSourceKind::Jsonl,
            );
        }
    }

    let resident_root = run_root.join("resident-live");
    if resident_root.is_dir() {
        collect_resident_error_event_sources(
            &resident_root,
            5,
            &mut sources,
            &mut seen,
            source_errors,
        );
    }

    sources
}

pub(crate) fn portfolio_error_log_run_root(options: &PortfolioDashboardOptions) -> Option<PathBuf> {
    let root = options.resident_root.as_ref()?;
    if root.join("logs").is_dir() || root.join("resident-live").is_dir() {
        return Some(root.clone());
    }
    if root
        .file_name()
        .and_then(|value| value.to_str())
        .is_some_and(|name| name == "resident-live")
    {
        if let Some(parent) = root.parent() {
            return Some(parent.to_path_buf());
        }
    }
    Some(root.clone())
}

fn collect_resident_error_event_sources(
    root: &Path,
    remaining_depth: usize,
    sources: &mut Vec<ErrorLogSource>,
    seen: &mut BTreeSet<String>,
    source_errors: &mut Vec<String>,
) {
    if remaining_depth == 0 {
        return;
    }
    let entries = match fs::read_dir(root) {
        Ok(entries) => entries,
        Err(error) => {
            source_errors.push(format!("{}: {error}", root.display()));
            return;
        }
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_resident_error_event_sources(
                &path,
                remaining_depth - 1,
                sources,
                seen,
                source_errors,
            );
            continue;
        }
        let Some(file_name) = path.file_name().and_then(|value| value.to_str()) else {
            continue;
        };
        if file_name.ends_with("_events.jsonl") || file_name == "resident_live_events.jsonl" {
            let source = resident_error_source_label(&path);
            push_error_log_source(sources, seen, &source, path, ErrorLogSourceKind::Jsonl);
        }
    }
}

fn resident_error_source_label(path: &Path) -> String {
    let path_text = path.display().to_string();
    if path_text.contains("cross-exchange-funding-arb") {
        "funding-arb-resident-live".to_owned()
    } else if path_text.contains("spot-perp-basis") {
        "spot-perp-basis-resident-live".to_owned()
    } else {
        path.file_stem()
            .and_then(|value| value.to_str())
            .unwrap_or("resident-events")
            .to_owned()
    }
}

fn push_error_log_source(
    sources: &mut Vec<ErrorLogSource>,
    seen: &mut BTreeSet<String>,
    source: &str,
    path: PathBuf,
    kind: ErrorLogSourceKind,
) {
    let key = path.display().to_string();
    if seen.insert(key) {
        sources.push(ErrorLogSource {
            source: source.to_owned(),
            path,
            kind,
        });
    }
}

fn append_jsonl_error_log_entries(
    source: &ErrorLogSource,
    entries: &mut Vec<ErrorLogEntry>,
    skipped_line_count: &mut usize,
    source_notices: &mut Vec<String>,
) -> RuntimeResult<()> {
    let (input, tailed) = read_error_log_source_tail_utf8(&source.path)?;
    if tailed {
        source_notices.push(format!(
            "{}：错误日志页面仅扫描末尾 {} 字节，避免大日志文件拖垮浏览器",
            source.path.display(),
            ERROR_LOG_SOURCE_TAIL_BYTES
        ));
    }
    let mut source_entries = Vec::new();
    for (line_index, line) in input.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let fields = match parse_json_object_value_slices(trimmed) {
            Ok(fields) => fields,
            Err(_) => {
                *skipped_line_count += 1;
                continue;
            }
        };
        if let Some(entry) =
            error_log_entry_from_json_fields(source, line_index + 1, trimmed, &fields)?
        {
            source_entries.push(entry);
        }
    }
    append_bounded_source_entries(entries, source_entries);
    Ok(())
}

fn append_text_error_log_entries(
    source: &ErrorLogSource,
    entries: &mut Vec<ErrorLogEntry>,
    source_notices: &mut Vec<String>,
) -> RuntimeResult<()> {
    let (input, tailed) = read_error_log_source_tail_utf8(&source.path)?;
    if tailed {
        source_notices.push(format!(
            "{}：错误日志页面仅扫描末尾 {} 字节，避免大日志文件拖垮浏览器",
            source.path.display(),
            ERROR_LOG_SOURCE_TAIL_BYTES
        ));
    }
    let mut source_entries = Vec::new();
    for (line_index, line) in input.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.is_empty() || !text_log_line_is_error(trimmed) {
            continue;
        }
        let strategy = text_log_field_value(trimmed, "strategy");
        let pair_id = text_log_pair_id(trimmed);
        let symbol = text_log_symbol(trimmed);
        let message = compact_repeated_pipe_segments(trimmed);
        source_entries.push(ErrorLogEntry {
            observed_at: timestamp_from_text_log_line(trimmed),
            severity: severity_from_text_log_message(&message).to_owned(),
            category: "文本日志".to_owned(),
            source: source.source.clone(),
            strategy,
            pair_id,
            symbol,
            message,
            path: source.path.display().to_string(),
            line_number: line_index + 1,
            raw: truncate_for_json(trimmed, ERROR_LOG_RAW_CHAR_LIMIT),
        });
    }
    append_bounded_source_entries(entries, source_entries);
    Ok(())
}

fn read_error_log_source_tail_utf8(path: &Path) -> RuntimeResult<(String, bool)> {
    let metadata = fs::metadata(path).map_err(|error| RuntimeError::Io {
        path: path.to_path_buf(),
        message: error.to_string(),
    })?;
    let file_len = metadata.len();
    if file_len <= ERROR_LOG_SOURCE_TAIL_BYTES {
        return read_utf8(path).map(|value| (value, false));
    }

    let mut file = fs::File::open(path).map_err(|error| RuntimeError::Io {
        path: path.to_path_buf(),
        message: error.to_string(),
    })?;
    file.seek(SeekFrom::Start(file_len - ERROR_LOG_SOURCE_TAIL_BYTES))
        .map_err(|error| RuntimeError::Io {
            path: path.to_path_buf(),
            message: error.to_string(),
        })?;
    let mut bytes = Vec::with_capacity(ERROR_LOG_SOURCE_TAIL_BYTES as usize);
    file.read_to_end(&mut bytes)
        .map_err(|error| RuntimeError::Io {
            path: path.to_path_buf(),
            message: error.to_string(),
        })?;

    if let Some(newline_index) = bytes.iter().position(|byte| *byte == b'\n') {
        bytes.drain(..=newline_index);
    }
    Ok((String::from_utf8_lossy(&bytes).into_owned(), true))
}

fn append_bounded_source_entries(
    entries: &mut Vec<ErrorLogEntry>,
    mut source_entries: Vec<ErrorLogEntry>,
) {
    if source_entries.len() > ERROR_LOG_MAX_ENTRIES_PER_SOURCE {
        let keep_from = source_entries.len() - ERROR_LOG_MAX_ENTRIES_PER_SOURCE;
        source_entries.drain(..keep_from);
    }
    entries.extend(source_entries);
}

fn error_log_entry_from_json_fields(
    source: &ErrorLogSource,
    line_number: usize,
    raw: &str,
    fields: &BTreeMap<String, &str>,
) -> RuntimeResult<Option<ErrorLogEntry>> {
    let mut reasons = Vec::new();
    for field in [
        "blocking_reason_details",
        "blocking_reasons",
        "dry_run_live_blocking_reasons",
        "live_blocking_reasons",
        "no_dispatch_reasons",
        "risk_reason_codes",
    ] {
        extend_unique(
            &mut reasons,
            optional_json_value_string_array_if_array(fields, field, "error log")?,
        );
    }
    extend_unique(
        &mut reasons,
        optional_json_blocking_path_reasons(fields, "blocking_path", "error log")?,
    );
    for field in [
        "error",
        "last_error",
        "reason",
        "halt_reason",
        "residual_risk",
        "detail",
    ] {
        if let Some(value) = optional_json_value_string(fields, field, "error log")? {
            push_unique_reason(&mut reasons, value);
        }
    }

    let event_type =
        optional_first_json_value_string(fields, &["event_type", "event"], "error log")?;
    let venue = optional_json_value_string(fields, "venue", "error log")?;
    let mut strategy = optional_json_value_string(fields, "strategy", "error log")?;
    let candidate_count = optional_json_usize(fields, "candidate_count", "error log")
        .ok()
        .flatten();
    if strategy.is_none() && venue.is_some() && candidate_count == Some(0) {
        strategy = Some(SPOT_PERP_BASIS_OBSERVER_STRATEGY.to_owned());
    }
    let status = optional_json_value_string(fields, "status", "error log")?;
    let dispatch_allowed = optional_json_bool(fields, "dispatch_allowed", "error log")
        .ok()
        .flatten();
    let dispatch_attempted = optional_json_bool(fields, "dispatch_attempted", "error log")
        .ok()
        .flatten();
    let mutable_execution_started =
        optional_json_bool(fields, "mutable_execution_started", "error log")
            .ok()
            .flatten();
    let dry_run_live_ready = optional_json_bool(fields, "dry_run_live_ready", "error log")
        .ok()
        .flatten()
        .or_else(|| {
            optional_json_bool(fields, "live_ready", "error log")
                .ok()
                .flatten()
        });
    let manual_gate_released = optional_json_bool(fields, "manual_gate_released", "error log")
        .ok()
        .flatten();
    let dispatch_plan_built = optional_json_bool(fields, "dispatch_plan_built", "error log")
        .ok()
        .flatten();

    let event_is_error = event_type.as_deref().is_some_and(event_type_is_error_like);
    let status_is_bad = status.as_deref().is_some_and(error_log_status_is_bad);
    if status_is_bad {
        if let Some(status) = &status {
            push_unique_reason(&mut reasons, format!("status={status}"));
        }
    }
    if event_is_error && reasons.is_empty() {
        if let Some(event_type) = &event_type {
            push_unique_reason(&mut reasons, event_type.clone());
        }
    }
    if dispatch_allowed == Some(false) && reasons.is_empty() {
        push_unique_reason(&mut reasons, "dispatch_allowed=false");
    }
    if candidate_count == Some(0) {
        match strategy.as_deref() {
            Some(CROSS_EXCHANGE_FUNDING_ARB_OBSERVER_STRATEGY) => {
                push_unique_reason(&mut reasons, "candidate_count=0：当前没有 funding-arb 候选");
            }
            Some(SPOT_PERP_BASIS_OBSERVER_STRATEGY) => {
                push_unique_reason(
                    &mut reasons,
                    "candidate_count=0：当前没有 spot-perp-basis 候选",
                );
            }
            _ => {}
        }
    }

    if reasons.is_empty() && !event_is_error && !status_is_bad {
        return Ok(None);
    }

    let gate_passed_then_failed = dry_run_live_ready == Some(true)
        && manual_gate_released == Some(true)
        && dispatch_plan_built == Some(true)
        && (dispatch_attempted == Some(true) || mutable_execution_started == Some(true))
        && (!reasons.is_empty() || dispatch_allowed == Some(false));
    let candidate_unavailable = event_type
        .as_deref()
        .is_some_and(|event| event.contains("no_candidate"))
        || candidate_count == Some(0);
    let category = if gate_passed_then_failed {
        "门禁通过后下单失败"
    } else if candidate_unavailable {
        "候选不可用"
    } else if dispatch_allowed == Some(false)
        || event_type
            .as_deref()
            .is_some_and(|event| event.contains("blocked"))
    {
        "下单门禁阻断"
    } else if event_type
        .as_deref()
        .is_some_and(|event| event.contains("unknown"))
    {
        "未知状态"
    } else if status_is_bad {
        "状态异常"
    } else {
        "运行错误"
    };

    let pair_id = optional_json_value_string(fields, "pair_id", "error log")?;
    let symbol = optional_json_value_string(fields, "symbol", "error log")?;
    let (blocking_path_pair_id, blocking_path_symbol) =
        if candidate_count == Some(0) && pair_id.is_none() && symbol.is_none() {
            (None, None)
        } else {
            optional_json_blocking_path_pair(fields, "blocking_path", "error log")?
        };
    let position_id_symbol = optional_json_value_string(fields, "position_id", "error log")?
        .and_then(|value| error_log_symbol_from_position_id(&value));

    let message = compact_repeated_pipe_segments(&reasons.join(" | "));
    let severity = if gate_passed_then_failed {
        "error".to_owned()
    } else if candidate_unavailable && !status_is_bad && !event_is_error {
        "warning".to_owned()
    } else {
        severity_from_message(&message).to_owned()
    };
    Ok(Some(ErrorLogEntry {
        observed_at: optional_first_json_value_string(
            fields,
            &[
                "recorded_at",
                "observed_at",
                "updated_at",
                "validation_finished_at",
                "opened_at",
            ],
            "error log",
        )?,
        severity,
        category: category.to_owned(),
        source: source.source.clone(),
        strategy,
        pair_id: pair_id.or(blocking_path_pair_id),
        symbol: symbol.or(blocking_path_symbol).or(position_id_symbol),
        message: truncate_for_json(&message, ERROR_LOG_MESSAGE_CHAR_LIMIT),
        path: source.path.display().to_string(),
        line_number,
        raw: truncate_for_json(raw, ERROR_LOG_RAW_CHAR_LIMIT),
    }))
}

fn optional_json_value_string_array_if_array(
    fields: &BTreeMap<String, &str>,
    field: &'static str,
    source: &'static str,
) -> RuntimeResult<Vec<String>> {
    let Some(value) = fields.get(field) else {
        return Ok(Vec::new());
    };
    if !value.trim().starts_with('[') {
        return Ok(Vec::new());
    }
    optional_json_value_string_array(fields, field, source)
}

fn optional_json_blocking_path_reasons(
    fields: &BTreeMap<String, &str>,
    field: &'static str,
    source: &'static str,
) -> RuntimeResult<Vec<String>> {
    let Some(value) = fields.get(field) else {
        return Ok(Vec::new());
    };
    let trimmed = value.trim();
    if !trimmed.starts_with('[') {
        return Ok(Vec::new());
    }
    let mut reasons = Vec::new();
    for item in json_array_value_slices(trimmed)? {
        let item = item.trim();
        if item.starts_with('"') {
            reasons.push(json_value_to_string(item, field, source)?);
            continue;
        }
        if !item.starts_with('{') {
            continue;
        }
        let item_fields = parse_json_object_value_slices(item)?;
        let reason = optional_json_value_string(&item_fields, "reason", source)?;
        let blocker = optional_json_value_string(&item_fields, "blocker", source)?;
        let stage = optional_json_value_string(&item_fields, "stage", source)?;
        if let Some(reason) = reason {
            let label = match (stage.as_deref(), blocker.as_deref()) {
                (Some(stage), Some(blocker)) => format!("{stage}:{blocker}"),
                (Some(stage), None) => stage.to_owned(),
                (None, Some(blocker)) => blocker.to_owned(),
                (None, None) => "blocking_path".to_owned(),
            };
            reasons.push(compact_blocking_summary_text(&label, &reason));
        }
    }
    Ok(compact_repeated_reasons(reasons))
}

fn optional_json_blocking_path_pair(
    fields: &BTreeMap<String, &str>,
    field: &'static str,
    source: &'static str,
) -> RuntimeResult<(Option<String>, Option<String>)> {
    let Some(value) = fields.get(field) else {
        return Ok((None, None));
    };
    let trimmed = value.trim();
    if !trimmed.starts_with('[') {
        return Ok((None, None));
    }
    for item in json_array_value_slices(trimmed)? {
        let item = item.trim();
        if !item.starts_with('{') {
            continue;
        }
        let item_fields = parse_json_object_value_slices(item)?;
        let pair_id = optional_json_value_string(&item_fields, "pair_id", source)?;
        let symbol = optional_json_value_string(&item_fields, "symbol", source)?.or_else(|| {
            optional_json_value_string(&item_fields, "evidence", source)
                .ok()
                .flatten()
                .and_then(|evidence| error_log_symbol_from_evidence(&evidence))
        });
        if pair_id.is_some() || symbol.is_some() {
            return Ok((pair_id, symbol));
        }
    }
    Ok((None, None))
}

fn error_log_symbol_from_evidence(evidence: &str) -> Option<String> {
    evidence
        .split_whitespace()
        .find_map(|part| part.strip_prefix("symbol="))
        .map(str::trim)
        .filter(|value| !value.is_empty() && *value != "null")
        .map(|value| {
            value
                .trim_matches(|ch: char| ch == ',' || ch == ';' || ch == '"' || ch == '\'')
                .to_owned()
        })
}

fn error_log_symbol_from_position_id(position_id: &str) -> Option<String> {
    let mut parts = position_id.split(':').collect::<Vec<_>>();
    if parts.len() < 4 || parts.first().copied() != Some("pos") {
        return None;
    }
    parts.pop()?;
    parts
        .pop()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

fn text_log_pair_id(line: &str) -> Option<String> {
    text_log_field_value(line, "pair_id").or_else(|| text_log_field_value(line, "pair"))
}

fn text_log_symbol(line: &str) -> Option<String> {
    text_log_field_value(line, "symbol")
        .or_else(|| text_log_field_value(line, "venue_symbol"))
        .or_else(|| text_log_field_value(line, "instId"))
        .or_else(|| text_log_field_value(line, "inst_id"))
        .or_else(|| {
            let venue = text_log_field_value(line, "venue")?;
            text_log_field_value(line, "strategy")?;
            Some(format!("{venue} 全市场"))
        })
}

fn text_log_field_value(line: &str, field: &str) -> Option<String> {
    let prefix = format!("{field}=");
    line.split_whitespace()
        .find_map(|part| part.strip_prefix(&prefix))
        .and_then(clean_text_log_field_value)
}

fn clean_text_log_field_value(value: &str) -> Option<String> {
    let trimmed = value.trim_matches(|ch: char| {
        ch == ','
            || ch == ';'
            || ch == '"'
            || ch == '\''
            || ch == '['
            || ch == ']'
            || ch == '('
            || ch == ')'
    });
    if trimmed.is_empty() || trimmed == "-" || trimmed.eq_ignore_ascii_case("null") {
        None
    } else {
        Some(trimmed.to_owned())
    }
}

fn event_type_is_error_like(event_type: &str) -> bool {
    let lower = event_type.to_ascii_lowercase();
    [
        "error", "fail", "failed", "blocked", "halt", "halted", "unknown", "reject", "rejected",
        "incident", "capacity",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
}

fn error_log_status_is_bad(status: &str) -> bool {
    let lower = status.to_ascii_lowercase();
    !matches!(
        lower.as_str(),
        "ok" | "healthy" | "complete" | "completed" | "streaming" | "starting" | "matched"
    )
}

fn text_log_line_is_error(line: &str) -> bool {
    let lower = line.to_ascii_lowercase();
    [
        " error",
        "error:",
        "failed",
        " fail",
        "rejected",
        "blocked",
        "unknown",
        "unsafe",
        "halted",
        "panic",
        "denied",
        "refused",
        "unavailable",
        "invalid",
        "mismatch",
        "cannot",
        "失败",
        "错误",
        "阻断",
        "拒绝",
        "未知",
        "不可用",
        "无法",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
}

fn severity_from_message(message: &str) -> &'static str {
    let lower = message.to_ascii_lowercase();
    if lower.contains("unknown")
        || lower.contains("residual")
        || lower.contains("panic")
        || lower.contains("unsafe")
        || message.contains("未知")
    {
        "critical"
    } else if lower.contains("blocked")
        || lower.contains("rejected")
        || lower.contains("failed")
        || lower.contains("error")
        || lower.contains("invalid")
        || message.contains("阻断")
        || message.contains("拒绝")
        || message.contains("失败")
        || message.contains("错误")
    {
        "error"
    } else {
        "warning"
    }
}

fn severity_from_text_log_message(message: &str) -> &'static str {
    if message.contains("candidate_count=0") {
        "warning"
    } else {
        severity_from_message(message)
    }
}

fn timestamp_from_text_log_line(line: &str) -> Option<String> {
    if let Some(close) = line.find(']') {
        if line.starts_with('[') && close > 1 {
            return Some(line[1..close].to_owned());
        }
    }
    line.split_whitespace()
        .next()
        .filter(|value| value.contains('T') && value.contains(':'))
        .map(str::to_owned)
}

fn error_log_timestamp_sort_key(value: Option<&str>) -> Option<(i64, u32)> {
    let value = value?.trim();
    if value.is_empty() {
        return None;
    }
    if let Ok(timestamp) = UtcTimestamp::parse_rfc3339_z(value) {
        return Some((timestamp.unix_seconds(), timestamp.nanoseconds()));
    }
    parse_error_log_offset_timestamp(value)
}

fn parse_error_log_offset_timestamp(value: &str) -> Option<(i64, u32)> {
    let (date_time, offset) = split_error_log_timestamp_offset(value)?;
    let (date, time) = date_time.split_once('T')?;
    let (year, month, day) = parse_error_log_date(date)?;
    let (hour, minute, second, nanoseconds) = parse_error_log_time(time)?;
    let offset_seconds = parse_error_log_offset_seconds(offset)?;

    let local_seconds = error_log_days_from_civil(year, month, day)
        .checked_mul(86_400)?
        .checked_add(i64::from(hour * 3_600 + minute * 60 + second))?;
    let utc_seconds = local_seconds.checked_sub(offset_seconds)?;
    let timestamp = UtcTimestamp::from_unix_parts(utc_seconds, nanoseconds).ok()?;
    Some((timestamp.unix_seconds(), timestamp.nanoseconds()))
}

fn split_error_log_timestamp_offset(value: &str) -> Option<(&str, &str)> {
    let bytes = value.as_bytes();
    if value.len() >= 6 {
        let offset_start = value.len() - 6;
        if matches!(bytes[offset_start], b'+' | b'-') && bytes[offset_start + 3] == b':' {
            return Some(value.split_at(offset_start));
        }
    }
    if value.len() >= 5 {
        let offset_start = value.len() - 5;
        if matches!(bytes[offset_start], b'+' | b'-') {
            return Some(value.split_at(offset_start));
        }
    }
    None
}

fn parse_error_log_offset_seconds(value: &str) -> Option<i64> {
    let sign = match value.as_bytes().first()? {
        b'+' => 1_i64,
        b'-' => -1_i64,
        _ => return None,
    };
    let body = &value[1..];
    let (hour, minute) = if body.len() == 5 && body.as_bytes()[2] == b':' {
        (
            parse_error_log_fixed_digits(&body[0..2])?,
            parse_error_log_fixed_digits(&body[3..5])?,
        )
    } else if body.len() == 4 {
        (
            parse_error_log_fixed_digits(&body[0..2])?,
            parse_error_log_fixed_digits(&body[2..4])?,
        )
    } else {
        return None;
    };
    if hour > 23 || minute > 59 {
        return None;
    }
    Some(sign * i64::from(hour * 3_600 + minute * 60))
}

fn parse_error_log_date(value: &str) -> Option<(i32, u32, u32)> {
    if value.len() != 10 || !value.is_ascii() {
        return None;
    }
    let bytes = value.as_bytes();
    if bytes[4] != b'-' || bytes[7] != b'-' {
        return None;
    }
    let year = parse_error_log_fixed_digits(&value[0..4])? as i32;
    let month = parse_error_log_fixed_digits(&value[5..7])?;
    let day = parse_error_log_fixed_digits(&value[8..10])?;
    if month == 0 || month > 12 {
        return None;
    }
    if day == 0 || day > error_log_days_in_month(year, month) {
        return None;
    }
    Some((year, month, day))
}

fn parse_error_log_time(value: &str) -> Option<(u32, u32, u32, u32)> {
    if value.len() < 8 || !value.is_ascii() {
        return None;
    }
    let bytes = value.as_bytes();
    if bytes[2] != b':' || bytes[5] != b':' {
        return None;
    }
    let hour = parse_error_log_fixed_digits(&value[0..2])?;
    let minute = parse_error_log_fixed_digits(&value[3..5])?;
    let second = parse_error_log_fixed_digits(&value[6..8])?;
    if hour > 23 || minute > 59 || second > 59 {
        return None;
    }

    let nanoseconds = if value.len() == 8 {
        0
    } else {
        let fraction = value[8..].strip_prefix('.')?;
        parse_error_log_fractional_nanos(fraction)?
    };
    Some((hour, minute, second, nanoseconds))
}

fn parse_error_log_fixed_digits(value: &str) -> Option<u32> {
    if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    value.parse::<u32>().ok()
}

fn parse_error_log_fractional_nanos(value: &str) -> Option<u32> {
    if value.is_empty() || value.len() > 9 || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let mut nanoseconds = 0_u32;
    for byte in value.bytes() {
        nanoseconds = nanoseconds * 10 + u32::from(byte - b'0');
    }
    for _ in value.len()..9 {
        nanoseconds *= 10;
    }
    Some(nanoseconds)
}

fn error_log_days_in_month(year: i32, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if error_log_is_leap_year(year) => 29,
        2 => 28,
        _ => 0,
    }
}

fn error_log_is_leap_year(year: i32) -> bool {
    (year % 4 == 0 && year % 100 != 0) || year % 400 == 0
}

fn error_log_days_from_civil(year: i32, month: u32, day: u32) -> i64 {
    let year = i64::from(year) - i64::from(month <= 2);
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let year_of_era = year - era * 400;
    let month = i64::from(month);
    let day = i64::from(day);
    let day_of_year = (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

pub(crate) fn truncate_for_json(value: &str, max_chars: usize) -> String {
    let mut out = value.chars().take(max_chars).collect::<String>();
    if value.chars().count() > max_chars {
        out.push_str("...");
    }
    out
}

pub(crate) fn compact_repeated_pipe_segments(value: &str) -> String {
    for marker in [" candidate_count=0 ", " blocking_path=", " blocking_path: "] {
        if let Some(index) = value.find(marker) {
            let split = index + marker.len();
            if let Some(compacted) = compact_pipe_segment_tail(&value[split..]) {
                return format!("{}{}", &value[..split], compacted);
            }
        }
    }
    compact_pipe_segment_tail(value).unwrap_or_else(|| value.to_owned())
}

fn compact_pipe_segment_tail(value: &str) -> Option<String> {
    if !value.contains('|') {
        return None;
    }
    let segments = value
        .split('|')
        .map(str::trim)
        .filter(|segment| !segment.is_empty())
        .map(compact_feedback_log_segment)
        .collect::<Vec<_>>();
    if segments.len() < 2 {
        return None;
    }
    let compacted = compact_repeated_reasons(segments);
    let joined = compacted.join(" | ");
    (joined != value.trim()).then_some(joined)
}

pub(crate) fn compact_repeated_reasons(values: Vec<String>) -> Vec<String> {
    let mut counted = Vec::<(String, usize)>::new();
    for value in values {
        let value = value.trim();
        if value.is_empty() {
            continue;
        }
        if let Some((_, count)) = counted.iter_mut().find(|(existing, _)| existing == value) {
            *count += 1;
        } else {
            counted.push((value.to_owned(), 1));
        }
    }
    counted
        .into_iter()
        .map(|(value, count)| counted_reason_text(value, count))
        .collect()
}

pub(crate) fn counted_reason_text(value: String, count: usize) -> String {
    if count > 1 {
        format!("{value} (x{count})")
    } else {
        value
    }
}

fn compact_feedback_log_segment(segment: &str) -> String {
    let trimmed = segment.trim();
    if let Some(index) = trimmed.find(":阻断") {
        let label = trimmed[..index].trim();
        let reason = trimmed[index + 1..].trim();
        return compact_blocking_summary_text(label, reason);
    }
    if trimmed.contains("阻断原因：") || trimmed.contains("阻断原因:") {
        return format!("阻断原因：{}", compact_blocking_reason_for_summary(trimmed));
    }
    trimmed.to_owned()
}

pub(crate) fn compact_blocking_summary_text(label: &str, reason: &str) -> String {
    let reason = compact_blocking_reason_for_summary(reason);
    let label = label.trim();
    if label == "candidate_count=0" || label.ends_with(":candidate_count=0") {
        format!("阻断原因：{reason}")
    } else if label.is_empty() {
        reason
    } else {
        format!("{label}:{reason}")
    }
}

pub(crate) fn compact_blocking_reason_for_summary(reason: &str) -> String {
    let trimmed = reason.trim();
    let mut summary = trimmed;
    for marker in ["阻断原因：", "阻断原因:"] {
        if let Some(index) = summary.find(marker) {
            summary = &summary[index + marker.len()..];
            break;
        }
    }
    let detail_code = blocking_reason_code_after_summary(summary);
    let summary = summary.split(['；', ';']).next().unwrap_or(summary).trim();
    let summary = if summary.is_empty() { trimmed } else { summary };
    match detail_code {
        Some(code) if !summary.contains(&code) => format!("{summary}；{code}"),
        _ => summary.to_owned(),
    }
}

fn blocking_reason_code_after_summary(reason: &str) -> Option<String> {
    let detail = reason
        .split_once('；')
        .map(|(_, detail)| detail)
        .or_else(|| reason.split_once(';').map(|(_, detail)| detail))?;
    detail
        .split(|ch: char| ch.is_whitespace() || matches!(ch, ',' | ';' | '；' | ':' | ')'))
        .map(|token| token.trim_matches(|ch: char| matches!(ch, '"' | '\'' | '(' | '[' | ']')))
        .find(|token| {
            token.contains('_')
                && token
                    .chars()
                    .all(|ch| ch.is_ascii_uppercase() || ch.is_ascii_digit() || ch == '_')
        })
        .map(str::to_owned)
}

pub(crate) fn error_logs_json(snapshot: &ErrorLogsSnapshot) -> String {
    format!(
        "{{\"entries\":[{}],\"entry_count\":{},\"entry_limit\":{},\"schema_version\":\"1.0.0\",\"skipped_line_count\":{},\"source_count\":{},\"source_error_count\":{},\"source_errors\":{},\"source_notice_count\":{},\"source_notices\":{},\"status\":{},\"updated_at\":{}}}",
        snapshot
            .entries
            .iter()
            .map(error_log_entry_json)
            .collect::<Vec<_>>()
            .join(","),
        snapshot.entries.len(),
        ERROR_LOG_MAX_ENTRIES,
        snapshot.skipped_line_count,
        snapshot.source_count,
        snapshot.source_errors.len(),
        json_string_array(&snapshot.source_errors),
        snapshot.source_notices.len(),
        json_string_array(&snapshot.source_notices),
        json_string(&snapshot.status),
        json_string(&snapshot.updated_at),
    )
}

fn error_log_entry_json(entry: &ErrorLogEntry) -> String {
    format!(
        "{{\"category\":{},\"line_number\":{},\"message\":{},\"observed_at\":{},\"pair_id\":{},\"path\":{},\"raw\":{},\"severity\":{},\"source\":{},\"strategy\":{},\"symbol\":{}}}",
        json_string(&entry.category),
        entry.line_number,
        json_string(&entry.message),
        json_option_string(&entry.observed_at),
        json_option_string(&entry.pair_id),
        json_string(&entry.path),
        json_string(&entry.raw),
        json_string(&entry.severity),
        json_string(&entry.source),
        json_option_string(&entry.strategy),
        json_option_string(&entry.symbol),
    )
}

pub(crate) fn portfolio_balance_row_json(row: &PortfolioBalanceRow) -> String {
    format!(
        "{{\"account_id\":{},\"asset\":{},\"available\":{},\"borrowed\":{},\"free\":{},\"lent\":{},\"locked\":{},\"maintenance_margin\":{},\"margin_balance\":{},\"margin_buffer\":{},\"pending\":{},\"reserved\":{},\"source_status\":{},\"unsettled\":{},\"venue_family\":{}}}",
        json_string(&row.account_id),
        json_string(&row.asset),
        json_option_string(&row.available),
        json_option_string(&row.borrowed),
        json_option_string(&row.free),
        json_option_string(&row.lent),
        json_option_string(&row.locked),
        json_option_string(&row.maintenance_margin),
        json_option_string(&row.margin_balance),
        json_option_string(&row.margin_buffer),
        json_option_string(&row.pending),
        json_option_string(&row.reserved),
        json_string(&row.source_status),
        json_option_string(&row.unsettled),
        json_string(&row.venue_family),
    )
}

pub(crate) fn portfolio_position_row_json(row: &PortfolioPositionRow) -> String {
    let opened_at = row
        .opened_at
        .as_ref()
        .map(|value| portfolio_normalize_datetime_display(value));
    let closed_at = row
        .closed_at
        .as_ref()
        .map(|value| portfolio_normalize_datetime_display(value));
    format!(
        "{{\"account_id\":{},\"accumulated_position\":{},\"closed_at\":{},\"close_action\":{},\"close_average_price\":{},\"coin\":{},\"fee\":{},\"fee_rate_bps\":{},\"funding_settlement_time\":{},\"manual_close_request_detail\":{},\"manual_close_request_id\":{},\"manual_close_request_status\":{},\"opened_at\":{},\"open_average_price\":{},\"open_close_condition\":{},\"open_close_spread_pct\":{},\"position_group_id\":{},\"position_group_label\":{},\"position_id\":{},\"position_kind\":{},\"position_leg_role\":{},\"position_limit\":{},\"position_quantity\":{},\"position_status\":{},\"realtime_funding_interval_hours\":{},\"realtime_funding_rate\":{},\"settled_funding_usd\":{},\"source\":{},\"strategy\":{},\"symbol\":{},\"venue_family\":{}}}",
        json_string(&row.account_id),
        json_option_string(&row.accumulated_position),
        json_option_string(&closed_at),
        portfolio_position_close_action_json(row),
        json_option_string(&row.close_average_price),
        json_string(&row.coin),
        json_option_string(&row.fee),
        json_option_string(&row.fee_rate_bps),
        json_option_string(&row.funding_settlement_time),
        json_option_string(&row.manual_close_request_detail),
        json_option_string(&row.manual_close_request_id),
        json_option_string(&row.manual_close_request_status),
        json_option_string(&opened_at),
        json_option_string(&row.open_average_price),
        json_option_string(&row.open_close_condition),
        json_option_string(&row.open_close_spread_pct),
        json_option_string(&row.position_group_id),
        json_option_string(&row.position_group_label),
        json_option_string(&row.position_id),
        json_option_string(&row.position_kind),
        json_option_string(&row.position_leg_role),
        json_option_string(&row.position_limit),
        json_string(&row.position_quantity),
        json_string(&row.position_status),
        json_option_string(&row.realtime_funding_interval_hours),
        json_option_string(&row.realtime_funding_rate),
        json_option_string(&row.settled_funding_usd),
        json_string(&row.source),
        json_string(&row.strategy),
        json_string(&row.symbol),
        json_string(&row.venue_family),
    )
}

pub(crate) fn portfolio_position_close_action_json(row: &PortfolioPositionRow) -> String {
    let disabled_reason = portfolio_position_close_action_disabled_reason(row);
    let eligible = disabled_reason.is_none();
    format!(
        "{{\"disabled_reason\":{},\"eligible\":{},\"order_type\":{},\"position_id\":{},\"request_id\":{},\"status\":{},\"supported\":{}}}",
        json_option_string(&disabled_reason),
        eligible,
        json_string(funding_arb_manual_close_order_type()),
        json_option_string(&row.position_id),
        json_option_string(&row.manual_close_request_id),
        json_option_string(&row.manual_close_request_status),
        row.position_kind.as_deref() == Some("cross_exchange_funding_arb"),
    )
}

pub(crate) fn portfolio_position_close_action_disabled_reason(
    row: &PortfolioPositionRow,
) -> Option<String> {
    let Some(position_id) = row
        .position_id
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    else {
        return Some("该仓位行缺少稳定 position_id，不能从 Portfolio 卡片触发平仓。".to_owned());
    };
    if row.position_kind.as_deref() != Some("cross_exchange_funding_arb") {
        return Some(
            "一键平仓第一版仅支持 funding-arb perp-perp 仓位；spot-perp basis 暂不支持。"
                .to_owned(),
        );
    }
    if row.position_status != "open" {
        return Some(format!(
            "仓位 `{position_id}` 当前状态为 `{}`，不是 active open 仓位。",
            row.position_status
        ));
    }
    if let Some(status) = row.manual_close_request_status.as_deref() {
        if funding_arb_manual_close_status_is_pending(status) {
            return Some(format!(
                "仓位 `{position_id}` 已有手动平仓请求，当前状态 `{status}`。"
            ));
        }
    }
    None
}

pub(crate) fn portfolio_normalize_datetime_display(value: &str) -> String {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return trimmed.to_owned();
    }
    portfolio_max_timestamp_ms_from_value(trimmed)
        .and_then(portfolio_timestamp_string_from_unix_millis)
        .unwrap_or_else(|| trimmed.to_owned())
}

pub(crate) fn portfolio_timestamp_string_from_unix_millis(timestamp_ms: u64) -> Option<String> {
    let seconds = i64::try_from(timestamp_ms / 1_000).ok()?;
    let nanos = u32::try_from(timestamp_ms % 1_000)
        .ok()?
        .checked_mul(1_000_000)?;
    UtcTimestamp::from_unix_parts(seconds, nanos)
        .ok()
        .map(|timestamp| timestamp.to_string())
}

pub(crate) fn portfolio_balance_rows_from_snapshot_json(
    input: &str,
) -> RuntimeResult<(Option<String>, Vec<PortfolioBalanceRow>)> {
    let snapshot = parse_funding_private_account_snapshot_json(input)?;
    let rows = portfolio_balance_rows_from_private_account_snapshot(&snapshot);
    Ok((snapshot.updated_at, rows))
}

pub(crate) fn portfolio_balance_rows_from_private_account_snapshot(
    snapshot: &FundingPrivateAccountSnapshot,
) -> Vec<PortfolioBalanceRow> {
    snapshot
        .accounts
        .iter()
        .map(|entry| PortfolioBalanceRow {
            venue_family: normalize_venue_family(&entry.venue_family),
            account_id: entry.account_id.clone(),
            asset: "USD".to_owned(),
            available: entry.available_usd.clone(),
            margin_balance: entry.margin_balance_usd.clone(),
            maintenance_margin: entry.maintenance_margin_usd.clone(),
            margin_buffer: entry.margin_buffer_usd.clone().or_else(|| {
                portfolio_margin_buffer(
                    entry.margin_balance_usd.as_deref(),
                    entry.maintenance_margin_usd.as_deref(),
                )
            }),
            free: None,
            locked: None,
            reserved: None,
            pending: None,
            borrowed: None,
            lent: None,
            unsettled: None,
            source_status: snapshot.status.clone(),
        })
        .collect()
}

#[cfg(test)]
pub(crate) fn portfolio_balance_rows_from_raw_snapshot_json(
    input: &str,
) -> RuntimeResult<(Option<String>, Vec<PortfolioBalanceRow>)> {
    let (updated_at, rows, _) = portfolio_balance_rows_from_raw_snapshot_json_with_errors(input)?;
    Ok((updated_at, rows))
}

pub(crate) fn portfolio_balance_rows_from_raw_snapshot_json_with_errors(
    input: &str,
) -> RuntimeResult<(Option<String>, Vec<PortfolioBalanceRow>, Vec<String>)> {
    let snapshot =
        parse_funding_private_raw_snapshot_json(input, "portfolio account raw snapshot")?;
    let mut rows = Vec::new();
    for statement in &snapshot.statements {
        rows.extend(portfolio_balance_rows_from_raw_statement(
            statement,
            &snapshot.status,
        )?);
    }
    if rows.is_empty() {
        rows = portfolio_balance_rows_from_private_account_snapshot(
            &funding_private_account_snapshot_from_raw_snapshot(&snapshot)?,
        );
    }
    Ok((snapshot.updated_at, rows, snapshot.source_errors))
}

pub(crate) fn portfolio_balance_rows_from_resident_root(
    root: &Path,
) -> RuntimeResult<(Option<String>, Vec<PortfolioBalanceRow>, Vec<String>)> {
    let mut paths = Vec::new();
    portfolio_collect_named_files(
        root,
        "funding_arb_private_account_raw_snapshot.json",
        8,
        &mut paths,
    )?;
    let mut latest_updated_at = None::<String>;
    let mut rows_by_key = BTreeMap::<String, (String, PortfolioBalanceRow)>::new();
    let mut unscoped_source_errors = Vec::new();
    let mut latest_success_by_venue = BTreeMap::<String, String>::new();
    let mut latest_source_error_by_venue = BTreeMap::<String, (String, String)>::new();

    for path in paths {
        let input = read_utf8(&path)?;
        let (updated_at, rows, mut errors) =
            portfolio_balance_rows_from_raw_snapshot_json_with_errors(&input)?;
        if let Some(updated_at) = &updated_at {
            if latest_updated_at
                .as_ref()
                .is_none_or(|current| updated_at > current)
            {
                latest_updated_at = Some(updated_at.clone());
            }
        }
        let sort_key = format!(
            "{}\u{1f}{}",
            updated_at.as_deref().unwrap_or_default(),
            path.display()
        );
        for source_error in errors.drain(..) {
            let Some(venue_family) = portfolio_source_error_venue_family(&source_error) else {
                unscoped_source_errors.push(source_error);
                continue;
            };
            if latest_source_error_by_venue
                .get(&venue_family)
                .is_none_or(|(current_key, _)| &sort_key >= current_key)
            {
                latest_source_error_by_venue.insert(venue_family, (sort_key.clone(), source_error));
            }
        }
        for row in rows {
            if latest_success_by_venue
                .get(&row.venue_family)
                .is_none_or(|current_key| &sort_key >= current_key)
            {
                latest_success_by_venue.insert(row.venue_family.clone(), sort_key.clone());
            }
            let row_key = format!(
                "{}\u{1f}{}\u{1f}{}",
                row.venue_family, row.account_id, row.asset
            );
            if rows_by_key
                .get(&row_key)
                .is_none_or(|(current_key, _)| &sort_key >= current_key)
            {
                rows_by_key.insert(row_key, (sort_key.clone(), row));
            }
        }
    }
    let mut source_errors = unscoped_source_errors;
    for (venue_family, (error_sort_key, source_error)) in latest_source_error_by_venue {
        if latest_success_by_venue
            .get(&venue_family)
            .is_some_and(|success_sort_key| success_sort_key > &error_sort_key)
        {
            continue;
        }
        source_errors.push(source_error);
    }

    Ok((
        latest_updated_at,
        rows_by_key
            .into_values()
            .map(|(_, row)| row)
            .collect::<Vec<_>>(),
        source_errors,
    ))
}

pub(crate) fn portfolio_collect_named_files(
    root: &Path,
    file_name: &str,
    remaining_depth: usize,
    paths: &mut Vec<PathBuf>,
) -> RuntimeResult<()> {
    if root.file_name().and_then(|value| value.to_str()) == Some(file_name) && root.is_file() {
        paths.push(root.to_path_buf());
        return Ok(());
    }
    if remaining_depth == 0 || !root.is_dir() {
        return Ok(());
    }
    for entry in fs::read_dir(root).map_err(|error| RuntimeError::Io {
        path: root.to_path_buf(),
        message: error.to_string(),
    })? {
        let entry = entry.map_err(|error| RuntimeError::Io {
            path: root.to_path_buf(),
            message: error.to_string(),
        })?;
        let path = entry.path();
        if path.is_dir() {
            portfolio_collect_named_files(&path, file_name, remaining_depth - 1, paths)?;
        } else if path.file_name().and_then(|value| value.to_str()) == Some(file_name) {
            paths.push(path);
        }
    }
    Ok(())
}

pub(crate) fn portfolio_balance_rows_from_raw_statement(
    statement: &FundingPrivateRawStatement,
    source_status: &str,
) -> RuntimeResult<Vec<PortfolioBalanceRow>> {
    let normalized_venue_family = normalize_venue_family(&statement.venue_family);
    let venue_specific_rows = match normalized_venue_family.as_str() {
        "binance" => portfolio_binance_balance_rows_from_raw_statement(statement, source_status)?,
        "bybit" => portfolio_bybit_balance_rows_from_raw_statement(statement, source_status)?,
        "bitget" => portfolio_bitget_balance_rows_from_raw_statement(statement, source_status)?,
        "okx" => portfolio_okx_balance_rows_from_raw_statement(statement, source_status)?,
        "aster" => portfolio_aster_balance_rows_from_raw_statement(statement, source_status)?,
        "hyperliquid" => {
            portfolio_hyperliquid_balance_rows_from_raw_statement(statement, source_status)?
        }
        _ => None,
    };
    if let Some(rows) = venue_specific_rows {
        return Ok(rows);
    }

    let mut object_slices = Vec::new();
    collect_json_objects_recursive(&statement.payload_json, &mut object_slices)?;
    let mut rows = Vec::new();
    let mut seen = BTreeSet::new();
    for object in object_slices {
        let fields = parse_json_object_value_slices(object)?;
        let asset = first_portfolio_balance_asset_from_raw_fields(&fields)?;
        let available = first_json_scalar_string(
            &fields,
            &[
                "available",
                "availableBalance",
                "available_balance",
                "availableToWithdraw",
                "maxWithdrawAmount",
                "withdrawable",
            ],
        )?;
        let free = first_json_scalar_string(&fields, &["free", "cashBal", "cashBalance"])?;
        let locked = first_json_scalar_string(
            &fields,
            &["locked", "frozenBal", "frozen", "hold", "holdBalance"],
        )?;
        let reserved = first_json_scalar_string(&fields, &["reserved", "reservation"])?;
        let pending = first_json_scalar_string(&fields, &["pending", "pendingBalance"])?;
        let borrowed = first_json_scalar_string(&fields, &["borrowed", "debt", "liability"])?;
        let lent = first_json_scalar_string(&fields, &["lent", "loaned"])?;
        let unsettled =
            first_json_scalar_string(&fields, &["unsettled", "unsettledBalance", "upl"])?;
        let margin_balance = first_json_scalar_string(
            &fields,
            &[
                "margin_balance_usd",
                "marginBalance",
                "crossWalletBalance",
                "walletBalance",
                "balance",
                "equity",
                "accountEquity",
                "totalEq",
                "accountValue",
            ],
        )?;
        let maintenance_margin = first_json_scalar_string(
            &fields,
            &[
                "maintenance_margin_usd",
                "maintMargin",
                "maintenanceMargin",
                "mmr",
                "crossMaintenanceMarginUsed",
            ],
        )?;
        let margin_buffer = first_json_scalar_string(
            &fields,
            &[
                "margin_buffer_usd",
                "available_margin_usd",
                "availableMargin",
                "availEq",
            ],
        )?
        .or_else(|| {
            portfolio_margin_buffer(margin_balance.as_deref(), maintenance_margin.as_deref())
        });

        let has_balance_value = available.is_some()
            || free.is_some()
            || locked.is_some()
            || reserved.is_some()
            || pending.is_some()
            || borrowed.is_some()
            || lent.is_some()
            || unsettled.is_some()
            || margin_balance.is_some()
            || maintenance_margin.is_some()
            || margin_buffer.is_some();
        if !has_balance_value {
            continue;
        }
        let asset = asset.unwrap_or_else(|| "USD".to_owned());
        let key = format!(
            "{}\u{1f}{}\u{1f}{}\u{1f}{:?}\u{1f}{:?}",
            normalized_venue_family, statement.account_id, asset, available, margin_balance,
        );
        if !seen.insert(key) {
            continue;
        }
        rows.push(PortfolioBalanceRow {
            venue_family: normalized_venue_family.clone(),
            account_id: statement.account_id.clone(),
            asset,
            available,
            margin_balance,
            maintenance_margin,
            margin_buffer,
            free,
            locked,
            reserved,
            pending,
            borrowed,
            lent,
            unsettled,
            source_status: source_status.to_owned(),
        });
    }
    Ok(rows)
}

pub(crate) fn portfolio_binance_balance_rows_from_raw_statement(
    statement: &FundingPrivateRawStatement,
    source_status: &str,
) -> RuntimeResult<Option<Vec<PortfolioBalanceRow>>> {
    let payload = statement.payload_json.trim();
    if !payload.starts_with('{') {
        return Ok(None);
    }
    let fields = parse_json_object_value_slices(payload)?;
    if let Some(assets) = fields.get("assets") {
        return portfolio_binance_balance_rows_from_raw_array(
            statement,
            source_status,
            assets,
            PortfolioBinanceBalanceArrayKind::UsdmAssets,
        )
        .map(Some);
    }
    if let Some(balances) = fields.get("balances") {
        return portfolio_binance_balance_rows_from_raw_array(
            statement,
            source_status,
            balances,
            PortfolioBinanceBalanceArrayKind::SpotBalances,
        )
        .map(Some);
    }
    Ok(None)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PortfolioBinanceBalanceArrayKind {
    SpotBalances,
    UsdmAssets,
}

pub(crate) fn portfolio_binance_balance_rows_from_raw_array(
    statement: &FundingPrivateRawStatement,
    source_status: &str,
    array_json: &str,
    kind: PortfolioBinanceBalanceArrayKind,
) -> RuntimeResult<Vec<PortfolioBalanceRow>> {
    let mut rows = Vec::new();
    let mut seen = BTreeSet::new();
    for value in json_array_value_slices(array_json)? {
        let fields = parse_json_object_value_slices(value)?;
        let Some(asset) = json_scalar_string_from_fields(&fields, "asset")?
            .map(|value| portfolio_balance_asset_from_raw_field("asset", &value))
        else {
            continue;
        };
        if !portfolio_binance_raw_balance_has_owned_amount(&fields, kind)? {
            continue;
        }

        let available = first_json_scalar_string(
            &fields,
            &["available", "availableBalance", "maxWithdrawAmount"],
        )?;
        let free = first_json_scalar_string(&fields, &["free"])?;
        let locked = first_json_scalar_string(&fields, &["locked", "initialMargin"])?;
        let reserved = first_json_scalar_string(&fields, &["reserved", "openOrderInitialMargin"])?;
        let pending = first_json_scalar_string(&fields, &["pending", "pendingBalance"])?;
        let borrowed = first_json_scalar_string(&fields, &["borrowed", "debt", "liability"])?;
        let lent = first_json_scalar_string(&fields, &["lent", "loaned"])?;
        let unsettled =
            first_json_scalar_string(&fields, &["unsettled", "unsettledBalance", "crossUnPnl"])?;
        let margin_balance = first_json_scalar_string(
            &fields,
            &[
                "marginBalance",
                "crossWalletBalance",
                "walletBalance",
                "balance",
            ],
        )?;
        let maintenance_margin = first_json_scalar_string(
            &fields,
            &["maintMargin", "maintenanceMargin", "totalPositionMM"],
        )?;
        let margin_buffer = first_json_scalar_string(&fields, &["availableMargin", "availEq"])?
            .or_else(|| {
                portfolio_margin_buffer(margin_balance.as_deref(), maintenance_margin.as_deref())
            });

        let key = format!(
            "{}\u{1f}{}\u{1f}{}",
            normalize_venue_family(&statement.venue_family),
            statement.account_id,
            asset
        );
        if !seen.insert(key) {
            continue;
        }
        rows.push(PortfolioBalanceRow {
            venue_family: normalize_venue_family(&statement.venue_family),
            account_id: statement.account_id.clone(),
            asset,
            available,
            margin_balance,
            maintenance_margin,
            margin_buffer,
            free,
            locked,
            reserved,
            pending,
            borrowed,
            lent,
            unsettled,
            source_status: source_status.to_owned(),
        });
    }
    Ok(rows)
}

pub(crate) fn portfolio_binance_raw_balance_has_owned_amount(
    fields: &BTreeMap<String, &str>,
    kind: PortfolioBinanceBalanceArrayKind,
) -> RuntimeResult<bool> {
    match kind {
        PortfolioBinanceBalanceArrayKind::SpotBalances => {
            portfolio_any_nonzero_json_scalar(fields, &["free", "locked"])
        }
        PortfolioBinanceBalanceArrayKind::UsdmAssets => portfolio_any_nonzero_json_scalar(
            fields,
            &[
                "walletBalance",
                "crossWalletBalance",
                "marginBalance",
                "balance",
                "initialMargin",
                "positionInitialMargin",
                "openOrderInitialMargin",
                "maintMargin",
                "unrealizedProfit",
                "crossUnPnl",
            ],
        ),
    }
}

pub(crate) fn portfolio_bybit_balance_rows_from_raw_statement(
    statement: &FundingPrivateRawStatement,
    source_status: &str,
) -> RuntimeResult<Option<Vec<PortfolioBalanceRow>>> {
    let payload = statement.payload_json.trim();
    if !payload.starts_with('{') {
        return Ok(None);
    }
    let fields = parse_json_object_value_slices(payload)?;
    let Some(result) = fields.get("result") else {
        return Ok(None);
    };
    let result_fields = parse_json_object_value_slices(result)?;
    let Some(accounts) = result_fields.get("list") else {
        return Ok(None);
    };

    let mut rows = Vec::new();
    let mut seen = BTreeSet::new();
    for account_value in json_array_value_slices(accounts)? {
        let account_fields = parse_json_object_value_slices(account_value)?;
        let Some(coins) = account_fields.get("coin") else {
            continue;
        };
        for coin_value in json_array_value_slices(coins)? {
            let fields = parse_json_object_value_slices(coin_value)?;
            let Some(asset) = json_scalar_string_from_fields(&fields, "coin")?
                .map(|value| portfolio_balance_asset_from_raw_field("coin", &value))
            else {
                continue;
            };
            if !portfolio_any_nonzero_json_scalar(
                &fields,
                &[
                    "walletBalance",
                    "equity",
                    "usdValue",
                    "locked",
                    "borrowAmount",
                    "totalOrderIM",
                    "totalPositionIM",
                    "totalPositionMM",
                    "spotHedgingQty",
                ],
            )? {
                continue;
            }
            let key = format!("{}\u{1f}{}", statement.account_id, asset);
            if !seen.insert(key) {
                continue;
            }
            let mut available =
                first_non_empty_json_scalar_string(&fields, &["availableToWithdraw"])?;
            if available.is_none() {
                available = funding_private_available_usd_from_unencumbered_wallet_fields(&fields)?;
            }
            let margin_balance = first_non_empty_json_scalar_string(
                &fields,
                &["walletBalance", "equity", "usdValue"],
            )?;
            let maintenance_margin =
                first_non_empty_json_scalar_string(&fields, &["totalPositionMM"])?;
            let margin_buffer =
                portfolio_margin_buffer(margin_balance.as_deref(), maintenance_margin.as_deref());
            rows.push(PortfolioBalanceRow {
                venue_family: normalize_venue_family(&statement.venue_family),
                account_id: statement.account_id.clone(),
                asset,
                available,
                margin_balance,
                maintenance_margin,
                margin_buffer,
                free: None,
                locked: first_non_empty_json_scalar_string(&fields, &["locked"])?,
                reserved: first_non_empty_json_scalar_string(&fields, &["totalOrderIM"])?,
                pending: None,
                borrowed: first_non_empty_json_scalar_string(&fields, &["borrowAmount"])?,
                lent: None,
                unsettled: first_non_empty_json_scalar_string(&fields, &["spotHedgingQty"])?,
                source_status: source_status.to_owned(),
            });
        }
    }
    Ok(Some(rows))
}

pub(crate) fn portfolio_bitget_balance_rows_from_raw_statement(
    statement: &FundingPrivateRawStatement,
    source_status: &str,
) -> RuntimeResult<Option<Vec<PortfolioBalanceRow>>> {
    let payload = statement.payload_json.trim();
    if !payload.starts_with('{') {
        return Ok(None);
    }
    let fields = parse_json_object_value_slices(payload)?;
    let Some(data) = fields.get("data") else {
        return Ok(None);
    };

    let mut rows = Vec::new();
    let mut seen = BTreeSet::new();
    for value in json_array_value_slices(data)? {
        let fields = parse_json_object_value_slices(value)?;
        let asset = if let Some(value) = json_scalar_string_from_fields(&fields, "marginCoin")? {
            portfolio_balance_asset_from_raw_field("marginCoin", &value)
        } else if let Some(value) = json_scalar_string_from_fields(&fields, "coin")? {
            portfolio_balance_asset_from_raw_field("coin", &value)
        } else {
            continue;
        };
        let has_owned_amount = if fields.contains_key("marginCoin") {
            portfolio_any_nonzero_json_scalar(
                &fields,
                &[
                    "accountEquity",
                    "usdtEquity",
                    "equity",
                    "locked",
                    "unrealizedPL",
                    "unrealizedPnl",
                    "crossedRiskRate",
                ],
            )?
        } else {
            portfolio_any_nonzero_json_scalar(&fields, &["available", "frozen", "locked"])?
        };
        if !has_owned_amount {
            continue;
        }
        let key = format!("{}\u{1f}{}", statement.account_id, asset);
        if !seen.insert(key) {
            continue;
        }
        rows.push(PortfolioBalanceRow {
            venue_family: normalize_venue_family(&statement.venue_family),
            account_id: statement.account_id.clone(),
            asset,
            available: first_json_scalar_string(
                &fields,
                &[
                    "available",
                    "crossedMaxAvailable",
                    "isolatedMaxAvailable",
                    "maxTransferOut",
                ],
            )?,
            margin_balance: first_json_scalar_string(
                &fields,
                &["accountEquity", "usdtEquity", "equity"],
            )?,
            maintenance_margin: None,
            margin_buffer: None,
            free: None,
            locked: first_json_scalar_string(&fields, &["locked", "frozen"])?,
            reserved: None,
            pending: None,
            borrowed: None,
            lent: None,
            unsettled: first_json_scalar_string(&fields, &["unrealizedPL", "unrealizedPnl"])?,
            source_status: source_status.to_owned(),
        });
    }
    Ok(Some(rows))
}

pub(crate) fn portfolio_okx_balance_rows_from_raw_statement(
    statement: &FundingPrivateRawStatement,
    source_status: &str,
) -> RuntimeResult<Option<Vec<PortfolioBalanceRow>>> {
    let payload = statement.payload_json.trim();
    if !payload.starts_with('{') {
        return Ok(None);
    }
    let fields = parse_json_object_value_slices(payload)?;
    let Some(data) = fields.get("data") else {
        return Ok(None);
    };

    let mut rows = Vec::new();
    let mut seen = BTreeSet::new();
    for account_value in json_array_value_slices(data)? {
        let account_fields = parse_json_object_value_slices(account_value)?;
        let Some(details) = account_fields.get("details") else {
            continue;
        };
        for detail_value in json_array_value_slices(details)? {
            let fields = parse_json_object_value_slices(detail_value)?;
            let Some(asset) = json_scalar_string_from_fields(&fields, "ccy")?
                .map(|value| portfolio_balance_asset_from_raw_field("ccy", &value))
            else {
                continue;
            };
            if !portfolio_any_nonzero_json_scalar(
                &fields,
                &[
                    "cashBal",
                    "eq",
                    "availBal",
                    "availEq",
                    "frozenBal",
                    "ordFrozen",
                    "liab",
                    "upl",
                ],
            )? {
                continue;
            }
            let key = format!("{}\u{1f}{}", statement.account_id, asset);
            if !seen.insert(key) {
                continue;
            }
            rows.push(PortfolioBalanceRow {
                venue_family: normalize_venue_family(&statement.venue_family),
                account_id: statement.account_id.clone(),
                asset,
                available: first_json_scalar_string(&fields, &["availBal", "availEq"])?,
                margin_balance: first_json_scalar_string(&fields, &["eq", "cashBal"])?,
                maintenance_margin: None,
                margin_buffer: None,
                free: first_json_scalar_string(&fields, &["cashBal"])?,
                locked: first_json_scalar_string(&fields, &["frozenBal"])?,
                reserved: first_json_scalar_string(&fields, &["ordFrozen"])?,
                pending: None,
                borrowed: first_json_scalar_string(&fields, &["liab"])?,
                lent: None,
                unsettled: first_json_scalar_string(&fields, &["upl"])?,
                source_status: source_status.to_owned(),
            });
        }
    }
    Ok(Some(rows))
}

pub(crate) fn portfolio_aster_balance_rows_from_raw_statement(
    statement: &FundingPrivateRawStatement,
    source_status: &str,
) -> RuntimeResult<Option<Vec<PortfolioBalanceRow>>> {
    let payload = statement.payload_json.trim();
    if !payload.starts_with('[') {
        return Ok(None);
    }

    let mut rows = Vec::new();
    let mut seen = BTreeSet::new();
    for value in json_array_value_slices(payload)? {
        let fields = parse_json_object_value_slices(value)?;
        let Some(asset) = json_scalar_string_from_fields(&fields, "asset")?
            .map(|value| portfolio_balance_asset_from_raw_field("asset", &value))
        else {
            continue;
        };
        if !portfolio_any_nonzero_json_scalar(
            &fields,
            &[
                "balance",
                "crossWalletBalance",
                "initialMargin",
                "positionInitialMargin",
                "openOrderInitialMargin",
                "maintMargin",
                "crossUnPnl",
            ],
        )? {
            continue;
        }
        let key = format!("{}\u{1f}{}", statement.account_id, asset);
        if !seen.insert(key) {
            continue;
        }
        let margin_balance = first_json_scalar_string(&fields, &["balance", "crossWalletBalance"])?;
        let maintenance_margin = first_json_scalar_string(&fields, &["maintMargin"])?;
        rows.push(PortfolioBalanceRow {
            venue_family: normalize_venue_family(&statement.venue_family),
            account_id: statement.account_id.clone(),
            asset,
            available: first_json_scalar_string(
                &fields,
                &["availableBalance", "maxWithdrawAmount"],
            )?,
            margin_balance: margin_balance.clone(),
            maintenance_margin: maintenance_margin.clone(),
            margin_buffer: portfolio_margin_buffer(
                margin_balance.as_deref(),
                maintenance_margin.as_deref(),
            ),
            free: None,
            locked: first_json_scalar_string(&fields, &["initialMargin"])?,
            reserved: first_json_scalar_string(&fields, &["openOrderInitialMargin"])?,
            pending: None,
            borrowed: None,
            lent: None,
            unsettled: first_json_scalar_string(&fields, &["crossUnPnl"])?,
            source_status: source_status.to_owned(),
        });
    }
    Ok(Some(rows))
}

pub(crate) fn portfolio_hyperliquid_balance_rows_from_raw_statement(
    statement: &FundingPrivateRawStatement,
    source_status: &str,
) -> RuntimeResult<Option<Vec<PortfolioBalanceRow>>> {
    let payload = statement.payload_json.trim();
    if !payload.starts_with('{') {
        return Ok(None);
    }
    let root_fields = parse_json_object_value_slices(payload)?;
    let spot_payload = hyperliquid_spot_state_payload(payload, &root_fields);
    let mut spot_rows = if let Some(spot_payload) = spot_payload {
        portfolio_hyperliquid_spot_balance_rows_from_state(statement, source_status, spot_payload)?
    } else {
        Vec::new()
    };
    let perp_payload = hyperliquid_clearinghouse_state_payload(payload, &root_fields);
    let perp_row = if let Some(perp_payload) = perp_payload {
        portfolio_hyperliquid_perp_balance_row_from_state(statement, source_status, perp_payload)?
    } else {
        None
    };

    let mut rows = Vec::new();
    if let Some(perp_row) = perp_row {
        if spot_rows.is_empty() || portfolio_balance_row_has_nonzero_amount(&perp_row)? {
            rows.push(perp_row);
        }
    }
    rows.append(&mut spot_rows);
    if rows.is_empty() {
        return Ok(None);
    }
    Ok(Some(rows))
}

pub(crate) fn portfolio_hyperliquid_perp_balance_row_from_state(
    statement: &FundingPrivateRawStatement,
    source_status: &str,
    payload: &str,
) -> RuntimeResult<Option<PortfolioBalanceRow>> {
    let fields = parse_json_object_value_slices(payload)?;
    let available = first_json_scalar_string(&fields, &["withdrawable", "available"])?;
    let margin_balance = hyperliquid_margin_balance_from_clearinghouse_fields(&fields)?;
    let maintenance_margin = first_json_scalar_string(
        &fields,
        &["crossMaintenanceMarginUsed", "maintenanceMargin"],
    )?;
    let explicit_margin_buffer =
        first_json_scalar_string(&fields, &["available_margin_usd", "availableMargin"])?;
    if available.is_none()
        && margin_balance.is_none()
        && maintenance_margin.is_none()
        && explicit_margin_buffer.is_none()
    {
        return Ok(None);
    }
    let margin_buffer = explicit_margin_buffer.or_else(|| {
        portfolio_margin_buffer(margin_balance.as_deref(), maintenance_margin.as_deref())
    });
    Ok(Some(PortfolioBalanceRow {
        venue_family: normalize_venue_family(&statement.venue_family),
        account_id: statement.account_id.clone(),
        asset: "USDC".to_owned(),
        available,
        margin_balance,
        maintenance_margin,
        margin_buffer,
        free: None,
        locked: None,
        reserved: None,
        pending: None,
        borrowed: None,
        lent: None,
        unsettled: None,
        source_status: source_status.to_owned(),
    }))
}

pub(crate) fn portfolio_hyperliquid_spot_balance_rows_from_state(
    statement: &FundingPrivateRawStatement,
    source_status: &str,
    payload: &str,
) -> RuntimeResult<Vec<PortfolioBalanceRow>> {
    let fields = parse_json_object_value_slices(payload)?;
    let Some(balances) = fields.get("balances") else {
        return Ok(Vec::new());
    };

    let mut rows = Vec::new();
    let mut seen = BTreeSet::new();
    for value in json_array_value_slices(balances)? {
        let fields = parse_json_object_value_slices(value)?;
        let Some(asset) = json_scalar_string_from_fields(&fields, "coin")?
            .map(|value| portfolio_balance_asset_from_raw_field("coin", &value))
        else {
            continue;
        };
        if !portfolio_any_nonzero_json_scalar(&fields, &["total", "hold", "entryNtl"])? {
            continue;
        }
        let key = format!("{}\u{1f}{}", statement.account_id, asset);
        if !seen.insert(key) {
            continue;
        }
        let total = first_non_empty_json_scalar_string(&fields, &["total"])?;
        let hold = first_non_empty_json_scalar_string(&fields, &["hold"])?;
        rows.push(PortfolioBalanceRow {
            venue_family: normalize_venue_family(&statement.venue_family),
            account_id: statement.account_id.clone(),
            asset,
            available: portfolio_hyperliquid_spot_available(total.as_deref(), hold.as_deref())?,
            margin_balance: total.clone(),
            maintenance_margin: None,
            margin_buffer: None,
            free: total,
            locked: hold,
            reserved: None,
            pending: None,
            borrowed: None,
            lent: None,
            unsettled: first_non_empty_json_scalar_string(&fields, &["entryNtl"])?,
            source_status: source_status.to_owned(),
        });
    }
    Ok(rows)
}

pub(crate) fn portfolio_hyperliquid_spot_available(
    total: Option<&str>,
    hold: Option<&str>,
) -> RuntimeResult<Option<String>> {
    let Some(total) = total else {
        return Ok(None);
    };
    let Some(hold) = hold else {
        return Ok(Some(total.to_owned()));
    };
    Ok(Some(
        MonitorDecimal::parse("hyperliquid.spot.total", total)?
            .checked_sub(
                MonitorDecimal::parse("hyperliquid.spot.hold", hold)?,
                "hyperliquid spot available balance",
            )?
            .format_trimmed(),
    ))
}

pub(crate) fn portfolio_balance_row_has_nonzero_amount(
    row: &PortfolioBalanceRow,
) -> RuntimeResult<bool> {
    for (field, value) in [
        ("available", row.available.as_deref()),
        ("margin_balance", row.margin_balance.as_deref()),
        ("maintenance_margin", row.maintenance_margin.as_deref()),
        ("margin_buffer", row.margin_buffer.as_deref()),
        ("free", row.free.as_deref()),
        ("locked", row.locked.as_deref()),
        ("reserved", row.reserved.as_deref()),
        ("pending", row.pending.as_deref()),
        ("borrowed", row.borrowed.as_deref()),
        ("lent", row.lent.as_deref()),
        ("unsettled", row.unsettled.as_deref()),
    ] {
        let Some(value) = value else {
            continue;
        };
        if MonitorDecimal::parse(field, value)?.raw != 0 {
            return Ok(true);
        }
    }
    Ok(false)
}

pub(crate) fn portfolio_any_nonzero_json_scalar(
    fields: &BTreeMap<String, &str>,
    field_names: &[&'static str],
) -> RuntimeResult<bool> {
    for field in field_names {
        let Some(value) = json_scalar_string_from_fields(fields, field)? else {
            continue;
        };
        if MonitorDecimal::parse(field, &value)?.raw != 0 {
            return Ok(true);
        }
    }
    Ok(false)
}

pub(crate) fn portfolio_funding_contexts_from_snapshot_json(
    input: &str,
) -> RuntimeResult<(Option<String>, Vec<PortfolioFundingContext>)> {
    let snapshot = parse_funding_arb_monitor_snapshot_json(input)?;
    let mut contexts = Vec::new();
    for row in &snapshot.rows {
        contexts.push(PortfolioFundingContext {
            venue_family: normalize_venue_family(&row.venue_a_family),
            symbol: funding_display_symbol(&funding_base_asset_from_symbol(&row.symbol)),
            funding_rate: row.venue_a_native_funding_rate.clone(),
            next_funding_time_ms: row.venue_a_next_funding_time_ms.clone(),
            funding_interval_hours: row.venue_a_native_funding_interval_hours.clone(),
        });
        contexts.push(PortfolioFundingContext {
            venue_family: normalize_venue_family(&row.venue_b_family),
            symbol: funding_display_symbol(&funding_base_asset_from_symbol(&row.symbol)),
            funding_rate: row.venue_b_native_funding_rate.clone(),
            next_funding_time_ms: row.venue_b_next_funding_time_ms.clone(),
            funding_interval_hours: row.venue_b_native_funding_interval_hours.clone(),
        });
    }
    Ok((Some(snapshot.updated_at), contexts))
}

pub(crate) fn portfolio_position_rows_from_snapshot_json(
    input: &str,
    funding_contexts: &[PortfolioFundingContext],
) -> RuntimeResult<(Option<String>, Vec<PortfolioPositionRow>)> {
    let fields = parse_json_object_value_slices(input)?;
    let updated_at =
        optional_json_value_string(&fields, "updated_at", "portfolio position snapshot")?;
    let source_status =
        optional_json_value_string(&fields, "status", "portfolio position snapshot")?
            .unwrap_or_else(|| "healthy".to_owned());
    let positions_value = fields
        .get("positions")
        .or_else(|| fields.get("rows"))
        .ok_or_else(|| RuntimeError::Module {
            module: "arb-runtime",
            message: "portfolio position snapshot is missing positions/rows".to_owned(),
        })?;
    let mut rows = Vec::new();
    for object in json_object_slices(positions_value)? {
        let fields = parse_json_object_value_slices(object)?;
        if let Some(row) = portfolio_position_row_from_fields(
            &fields,
            None,
            None,
            "position-snapshot",
            Some(source_status.as_str()),
            funding_contexts,
        )? {
            rows.push(row);
        }
    }
    Ok((updated_at, rows))
}

pub(crate) fn portfolio_position_rows_from_raw_snapshot_json(
    input: &str,
    funding_contexts: &[PortfolioFundingContext],
) -> RuntimeResult<(Option<String>, Vec<PortfolioPositionRow>)> {
    let snapshot =
        parse_funding_private_raw_snapshot_json(input, "portfolio position raw snapshot")?;
    let mut rows = Vec::new();
    for statement in &snapshot.statements {
        rows.extend(portfolio_position_rows_from_raw_statement(
            statement,
            snapshot.status.as_str(),
            funding_contexts,
        )?);
    }
    Ok((snapshot.updated_at, rows))
}

pub(crate) fn portfolio_position_rows_from_raw_statement(
    statement: &FundingPrivateRawStatement,
    source_status: &str,
    funding_contexts: &[PortfolioFundingContext],
) -> RuntimeResult<Vec<PortfolioPositionRow>> {
    if normalize_venue_family(&statement.venue_family) == "hyperliquid" {
        return portfolio_hyperliquid_position_rows_from_raw_statement(
            statement,
            source_status,
            funding_contexts,
        );
    }
    let payload = portfolio_position_payload_scope(&statement.payload_json)?;
    portfolio_position_rows_from_raw_payload(
        payload,
        statement.venue_family.as_str(),
        statement.account_id.as_str(),
        source_status,
        funding_contexts,
    )
}

pub(crate) fn portfolio_position_payload_scope(payload: &str) -> RuntimeResult<&str> {
    let trimmed = payload.trim();
    if !trimmed.starts_with('{') {
        return Ok(trimmed);
    }
    let fields = parse_json_object_value_slices(trimmed)?;
    Ok(fields
        .get("positions")
        .map(|value| value.trim())
        .unwrap_or(trimmed))
}

pub(crate) fn portfolio_hyperliquid_position_rows_from_raw_statement(
    statement: &FundingPrivateRawStatement,
    source_status: &str,
    funding_contexts: &[PortfolioFundingContext],
) -> RuntimeResult<Vec<PortfolioPositionRow>> {
    let payload = statement.payload_json.trim();
    if !payload.starts_with('{') {
        return Ok(Vec::new());
    }
    let root_fields = parse_json_object_value_slices(payload)?;
    let Some(clearinghouse_payload) =
        hyperliquid_clearinghouse_state_payload(payload, &root_fields)
    else {
        return Ok(Vec::new());
    };
    portfolio_position_rows_from_raw_payload(
        clearinghouse_payload,
        statement.venue_family.as_str(),
        statement.account_id.as_str(),
        source_status,
        funding_contexts,
    )
}

pub(crate) fn portfolio_position_rows_from_raw_payload(
    payload: &str,
    default_venue_family: &str,
    default_account_id: &str,
    source_status: &str,
    funding_contexts: &[PortfolioFundingContext],
) -> RuntimeResult<Vec<PortfolioPositionRow>> {
    let mut rows = Vec::new();
    let mut object_slices = Vec::new();
    collect_json_objects_recursive(payload, &mut object_slices)?;
    for object in object_slices {
        let fields = parse_json_object_value_slices(object)?;
        if let Some(row) = portfolio_position_row_from_fields(
            &fields,
            Some(default_venue_family),
            Some(default_account_id),
            "position-raw-snapshot",
            Some(source_status),
            funding_contexts,
        )? {
            rows.push(row);
        }
    }
    Ok(rows)
}

pub(crate) fn portfolio_position_row_from_fields(
    fields: &BTreeMap<String, &str>,
    default_venue_family: Option<&str>,
    default_account_id: Option<&str>,
    source: &str,
    source_status: Option<&str>,
    funding_contexts: &[PortfolioFundingContext],
) -> RuntimeResult<Option<PortfolioPositionRow>> {
    let Some(raw_symbol) = first_json_scalar_string(
        fields,
        &[
            "symbol",
            "instId",
            "instrument",
            "coin",
            "asset",
            "base_asset",
        ],
    )?
    else {
        return Ok(None);
    };
    let Some(position_quantity) = first_json_scalar_string(
        fields,
        &[
            "position_quantity",
            "quantity",
            "position_size",
            "size",
            "positionAmt",
            "szi",
            "total",
            "pos",
        ],
    )?
    else {
        return Ok(None);
    };
    let symbol = funding_settlement_display_symbol_from_raw(&raw_symbol);
    let venue_family = first_json_scalar_string(
        fields,
        &[
            "venue_family",
            "exchange",
            "venue",
            "venueFamily",
            "platform",
        ],
    )?
    .or_else(|| default_venue_family.map(str::to_owned))
    .or_else(|| {
        first_json_scalar_string(fields, &["venue_id", "spot_venue_id", "perp_venue_id"])
            .ok()
            .flatten()
            .and_then(|value| portfolio_venue_family_from_venue_id(&value))
    })
    .map(|value| normalize_venue_family(&value))
    .unwrap_or_else(|| "unknown".to_owned());
    let account_id = first_json_scalar_string(
        fields,
        &[
            "account_id",
            "account",
            "accountId",
            "uid",
            "user",
            "spot_account_id",
            "perp_account_id",
        ],
    )?
    .or_else(|| default_account_id.map(str::to_owned))
    .unwrap_or_else(|| "unknown".to_owned());
    let strategy = first_json_scalar_string(
        fields,
        &[
            "arbitrage_strategy",
            "strategy",
            "strategy_id",
            "strategyId",
            "basis_strategy",
        ],
    )?
    .or_else(|| portfolio_strategy_from_account_id(&account_id))
    .unwrap_or_else(|| "unknown".to_owned());
    let fee = first_json_scalar_string(
        fields,
        &[
            "fee",
            "fees",
            "fee_bps",
            "taker_fee_bps",
            "entry_total_cost_bps",
            "total_cost_bps",
            "deductedFee",
            "cumExecFee",
            "commission",
            "totalFee",
        ],
    )?
    .map(|value| portfolio_fee_display(&value));
    let fee_rate_bps = first_json_scalar_string(fields, &["fee_rate_bps", "taker_fee_bps"])?;
    let settled_funding_usd = first_json_scalar_string(
        fields,
        &[
            "settled_funding_usd",
            "actual_funding_usd",
            "funding_settlement_usd",
            "funding_fee_usd",
        ],
    )?;
    let accumulated_position = first_json_scalar_string(
        fields,
        &[
            "accumulated_position",
            "cumulative_position",
            "notional_usd",
            "notional_usdt",
            "notional",
            "marginSize",
            "position_value",
            "positionValue",
        ],
    )?
    .or_else(|| Some(position_quantity.clone()));
    let open_average_price = first_json_scalar_string(
        fields,
        &[
            "open_average_price",
            "open_avg_price",
            "entry_average_price",
            "entry_price",
            "entryPrice",
            "avgEntryPrice",
            "avgPx",
            "avgPrice",
            "openPriceAvg",
            "averageOpenPrice",
        ],
    )?;
    let close_average_price = first_json_scalar_string(
        fields,
        &[
            "close_average_price",
            "close_avg_price",
            "exit_average_price",
            "exit_price",
            "closePrice",
            "close_px",
            "markPrice",
            "markPx",
            "lastPrice",
        ],
    )?;
    let open_close_spread_pct = first_json_scalar_string(
        fields,
        &[
            "open_close_spread_pct",
            "spread_pct",
            "basis_pct",
            "entry_spread_pct",
        ],
    )?
    .or_else(|| {
        portfolio_price_spread_pct(
            open_average_price.as_deref(),
            close_average_price.as_deref(),
        )
    });
    let funding_context = portfolio_find_funding_context(funding_contexts, &venue_family, &symbol);
    let realtime_funding_rate = first_json_scalar_string(
        fields,
        &[
            "realtime_funding_rate",
            "funding_rate",
            "lastFundingRate",
            "fundingRate",
        ],
    )?
    .or_else(|| funding_context.map(|context| context.funding_rate.clone()));
    let realtime_funding_interval_hours = funding_context
        .map(|context| context.funding_interval_hours.clone())
        .or_else(|| {
            first_json_scalar_string(
                fields,
                &[
                    "realtime_funding_interval_hours",
                    "funding_interval_hours",
                    "fundingIntervalHours",
                    "fundingIntervalHour",
                ],
            )
            .ok()
            .flatten()
        });
    let funding_settlement_time = first_json_scalar_string(
        fields,
        &[
            "funding_settlement_time",
            "next_funding_time",
            "nextFundingTime",
            "next_funding_time_ms",
        ],
    )?
    .map(|value| value.trim().to_owned())
    .or_else(|| funding_context.map(|context| context.next_funding_time_ms.clone()));
    let opened_at = first_json_scalar_string(
        fields,
        &[
            "opened_at",
            "open_time",
            "openTime",
            "created_at",
            "createdAt",
            "cTime",
        ],
    )?;
    let closed_at = first_json_scalar_string(
        fields,
        &["closed_at", "close_time", "closeTime", "closedAt"],
    )?;
    let open_close_condition = first_json_scalar_string(
        fields,
        &[
            "open_close_condition",
            "condition",
            "reason",
            "entry_exit_condition",
        ],
    )?
    .or_else(|| {
        Some("开仓按套利信号与风控通过；清仓按价差收敛、止盈、止损和风险信号。".to_owned())
    });
    let explicit_status = first_json_scalar_string(
        fields,
        &["position_status", "status", "state", "positionState"],
    )?
    .or_else(|| source_status.map(str::to_owned));
    let position_status = portfolio_position_status(&position_quantity, explicit_status.as_deref());
    let position_limit = first_json_scalar_string(
        fields,
        &[
            "position_limit",
            "limit",
            "max_position",
            "riskLimitValue",
            "max_notional_usdt",
            "maxNotionalValue",
            "maxNotional",
        ],
    )?;
    let position_group_id = first_json_scalar_string(
        fields,
        &[
            "position_group_id",
            "group_id",
            "batch_id",
            "resident_position_id",
        ],
    )?;
    let position_group_label =
        first_json_scalar_string(fields, &["position_group_label", "pair_id", "batch_label"])?;
    let position_leg_role = first_json_scalar_string(
        fields,
        &[
            "position_leg_role",
            "leg_role",
            "basis_leg_role",
            "role",
            "positionSide",
            "holdSide",
            "side",
        ],
    )?;
    Ok(Some(PortfolioPositionRow {
        position_id: None,
        position_kind: None,
        coin: funding_base_asset_from_symbol(&symbol),
        symbol,
        strategy,
        venue_family,
        account_id,
        fee,
        fee_rate_bps,
        settled_funding_usd,
        accumulated_position,
        open_average_price,
        close_average_price,
        open_close_spread_pct,
        realtime_funding_rate,
        realtime_funding_interval_hours,
        funding_settlement_time,
        opened_at,
        closed_at,
        open_close_condition,
        position_status,
        position_quantity,
        position_group_id,
        position_group_label,
        position_leg_role,
        position_limit,
        manual_close_request_id: None,
        manual_close_request_status: None,
        manual_close_request_detail: None,
        source: source.to_owned(),
    }))
}

pub(crate) fn portfolio_position_rows_from_resident_root(
    root: &Path,
    funding_contexts: &[PortfolioFundingContext],
) -> RuntimeResult<(Option<String>, Vec<PortfolioPositionRow>)> {
    if let Some((updated_at, rows)) =
        portfolio_position_rows_from_resident_private_snapshots(root, funding_contexts)?
    {
        let mut rows = rows;
        let terminal_rows =
            portfolio_terminal_resident_position_rows_from_root(root, funding_contexts)?;
        portfolio_apply_terminal_resident_position_overrides(&mut rows, &terminal_rows);
        return Ok((updated_at, rows));
    }

    let mut rows = Vec::new();
    for position_ref in portfolio_resident_position_refs_from_root(root)? {
        rows.push(portfolio_position_row_from_resident_ref(
            &position_ref,
            funding_contexts,
        )?);
    }
    Ok((None, rows))
}

pub(crate) fn portfolio_resident_position_refs_from_root(
    root: &Path,
) -> RuntimeResult<Vec<PortfolioResidentPositionRef>> {
    let mut registry_dirs = Vec::new();
    portfolio_collect_resident_registry_dirs(root, 4, &mut registry_dirs)?;
    let mut position_refs = Vec::new();
    let mut seen_dirs = BTreeSet::new();
    for registry in registry_dirs {
        let key = format!("{}::{:?}", registry.path.display(), registry.kind);
        if !seen_dirs.insert(key) {
            continue;
        }
        let limit = portfolio_position_limit_from_config(&registry.path)?;
        let taker_fee_bps = portfolio_taker_fee_bps_from_config(&registry.path)?;
        for position_ref in portfolio_resident_position_refs_from_dir(
            &registry.path,
            registry.kind,
            limit,
            taker_fee_bps.clone(),
        )? {
            position_refs.push(position_ref);
        }
    }
    Ok(position_refs)
}

pub(crate) fn portfolio_terminal_resident_position_rows_from_root(
    root: &Path,
    funding_contexts: &[PortfolioFundingContext],
) -> RuntimeResult<Vec<PortfolioPositionRow>> {
    let mut rows = Vec::new();
    for position_ref in portfolio_resident_position_refs_from_root(root)? {
        if portfolio_resident_position_status_is_terminal(&position_ref.status) {
            rows.push(portfolio_position_row_from_resident_ref(
                &position_ref,
                funding_contexts,
            )?);
        }
    }
    Ok(rows)
}

pub(crate) fn portfolio_apply_terminal_resident_position_overrides(
    rows: &mut Vec<PortfolioPositionRow>,
    terminal_rows: &[PortfolioPositionRow],
) {
    let mut replacements = Vec::new();
    for terminal_row in terminal_rows {
        if rows.iter().any(|row| {
            portfolio_position_row_matches_terminal_resident(row, terminal_row)
                && row.source == "position-raw-snapshot"
                && !portfolio_position_row_is_flat(row)
        }) {
            continue;
        }
        let before = rows.len();
        rows.retain(|row| !portfolio_position_row_matches_terminal_resident(row, terminal_row));
        if rows.len() != before {
            replacements.push(terminal_row.clone());
        }
    }
    rows.extend(replacements);
}

pub(crate) fn portfolio_position_row_is_flat(row: &PortfolioPositionRow) -> bool {
    MonitorDecimal::parse("portfolio.position_quantity", &row.position_quantity)
        .is_ok_and(|quantity| quantity.raw == 0)
        || row.position_status == "flat"
}

pub(crate) fn portfolio_position_row_matches_terminal_resident(
    row: &PortfolioPositionRow,
    terminal_row: &PortfolioPositionRow,
) -> bool {
    if funding_settlement_display_symbol_from_raw(&row.symbol)
        != funding_settlement_display_symbol_from_raw(&terminal_row.symbol)
    {
        return false;
    }
    let row_venue = normalize_venue_family(&row.venue_family);
    terminal_row
        .venue_family
        .split('/')
        .map(normalize_venue_family)
        .any(|venue| venue == row_venue)
        || normalize_venue_family(&terminal_row.venue_family) == row_venue
}

pub(crate) fn portfolio_position_rows_from_resident_private_snapshots(
    root: &Path,
    funding_contexts: &[PortfolioFundingContext],
) -> RuntimeResult<Option<(Option<String>, Vec<PortfolioPositionRow>)>> {
    let mut paths = Vec::new();
    portfolio_collect_named_files(
        root,
        "funding_arb_private_position_raw_snapshot.json",
        8,
        &mut paths,
    )?;
    if paths.is_empty() {
        return Ok(None);
    }

    let resident_metadata_rows =
        portfolio_nonterminal_resident_position_rows_from_root(root, funding_contexts)?;
    let mut latest_updated_at = None::<String>;
    let mut statements_by_key =
        BTreeMap::<String, (String, String, FundingPrivateRawStatement)>::new();
    let mut latest_source_error_by_venue = BTreeMap::<String, String>::new();
    for path in paths {
        let input = read_utf8(&path)?;
        let snapshot =
            parse_funding_private_raw_snapshot_json(&input, "portfolio position raw snapshot")?;
        let sort_key = format!(
            "{}\u{1f}{}",
            snapshot.updated_at.as_deref().unwrap_or_default(),
            path.display()
        );
        if let Some(updated_at) = &snapshot.updated_at {
            if latest_updated_at
                .as_ref()
                .is_none_or(|current| updated_at > current)
            {
                latest_updated_at = Some(updated_at.clone());
            }
        }
        for source_error in &snapshot.source_errors {
            let Some(venue_family) = portfolio_source_error_venue_family(source_error) else {
                continue;
            };
            if latest_source_error_by_venue
                .get(&venue_family)
                .is_none_or(|current_key| &sort_key > current_key)
            {
                latest_source_error_by_venue.insert(venue_family, sort_key.clone());
            }
        }
        for statement in snapshot.statements {
            let statement_key = format!(
                "{}\u{1f}{}",
                normalize_venue_family(&statement.venue_family),
                statement.account_id
            );
            if statements_by_key
                .get(&statement_key)
                .is_none_or(|(current_key, _, _)| &sort_key >= current_key)
            {
                statements_by_key.insert(
                    statement_key,
                    (sort_key.clone(), snapshot.status.clone(), statement),
                );
            }
        }
    }

    let mut rows = Vec::new();
    for (statement_sort_key, source_status, statement) in statements_by_key.into_values() {
        let venue_family = normalize_venue_family(&statement.venue_family);
        let is_stale = latest_source_error_by_venue
            .get(&venue_family)
            .is_some_and(|source_error_sort_key| source_error_sort_key > &statement_sort_key);
        let mut statement_rows = portfolio_position_rows_from_raw_statement(
            &statement,
            &source_status,
            funding_contexts,
        )?;
        statement_rows.retain(portfolio_position_row_is_current_exchange_position);
        if is_stale {
            for row in &mut statement_rows {
                row.position_status = portfolio_stale_position_status(&row.position_status);
            }
        }
        portfolio_apply_resident_position_metadata(&mut statement_rows, &resident_metadata_rows);
        rows.extend(statement_rows);
    }
    Ok(Some((latest_updated_at, rows)))
}

pub(crate) fn portfolio_nonterminal_resident_position_rows_from_root(
    root: &Path,
    funding_contexts: &[PortfolioFundingContext],
) -> RuntimeResult<Vec<PortfolioPositionRow>> {
    let mut rows = Vec::new();
    for position_ref in portfolio_resident_position_refs_from_root(root)? {
        if portfolio_resident_position_status_is_terminal(&position_ref.status) {
            continue;
        }
        if let Some(mut metadata_rows) =
            portfolio_resident_position_metadata_rows(&position_ref, funding_contexts)?
        {
            rows.append(&mut metadata_rows);
            continue;
        }
        rows.push(portfolio_position_row_from_resident_ref(
            &position_ref,
            funding_contexts,
        )?);
    }
    Ok(rows)
}

pub(crate) fn portfolio_resident_position_metadata_rows(
    position_ref: &PortfolioResidentPositionRef,
    funding_contexts: &[PortfolioFundingContext],
) -> RuntimeResult<Option<Vec<PortfolioPositionRow>>> {
    if position_ref.kind != PortfolioResidentPositionKind::CrossExchangeFundingArb {
        return Ok(None);
    }
    let Ok(input) = read_utf8(&position_ref.position_state_path) else {
        return Ok(None);
    };
    let fields = parse_json_object_value_slices(&input)?;
    let symbol = required_json_value_string(&fields, "symbol", "funding arb position state")
        .map(|value| funding_settlement_display_symbol_from_raw(&value))?;
    let notional_usd =
        optional_json_value_string(&fields, "notional_usd", "funding arb position state")?
            .unwrap_or_else(|| position_ref.notional_usdt.clone());
    let entry_net_funding_bps = portfolio_optional_i128_field(
        &fields,
        "entry_net_funding_bps",
        "funding arb position state",
    )?;
    let opened_at = optional_json_value_string(&fields, "opened_at", "funding arb position state")?
        .or_else(|| position_ref.opened_at.clone());
    let mut rows = Vec::new();
    for leg in [
        portfolio_funding_arb_leg_display(&fields, "leg_a")?,
        portfolio_funding_arb_leg_display(&fields, "leg_b")?,
    ]
    .into_iter()
    .flatten()
    {
        rows.push(portfolio_funding_arb_leg_metadata_row(
            position_ref,
            funding_contexts,
            &symbol,
            &notional_usd,
            entry_net_funding_bps,
            opened_at.as_ref(),
            &leg,
        ));
    }
    Ok((!rows.is_empty()).then_some(rows))
}

pub(crate) fn portfolio_funding_arb_leg_metadata_row(
    position_ref: &PortfolioResidentPositionRef,
    funding_contexts: &[PortfolioFundingContext],
    symbol: &str,
    notional_usd: &str,
    entry_net_funding_bps: Option<i128>,
    opened_at: Option<&String>,
    leg: &PortfolioFundingArbLegDisplay,
) -> PortfolioPositionRow {
    let venue_family = normalize_venue_family(&leg.venue_family);
    let funding_context = portfolio_find_funding_context(funding_contexts, &venue_family, symbol);
    PortfolioPositionRow {
        position_id: Some(position_ref.position_id.clone()),
        position_kind: Some(portfolio_resident_position_kind_label(position_ref.kind).to_owned()),
        coin: funding_base_asset_from_symbol(symbol),
        symbol: symbol.to_owned(),
        strategy: "cross-exchange-funding-arb-resident-live".to_owned(),
        venue_family,
        account_id: leg.account_id.clone(),
        fee: None,
        fee_rate_bps: position_ref.taker_fee_bps.clone(),
        settled_funding_usd: None,
        accumulated_position: Some(format!("{notional_usd} USDT")),
        open_average_price: Some(leg.entry_limit_price.clone()),
        close_average_price: None,
        open_close_spread_pct: entry_net_funding_bps.map(portfolio_bps_to_percent_string),
        realtime_funding_rate: funding_context.map(|context| context.funding_rate.clone()),
        realtime_funding_interval_hours: funding_context
            .map(|context| context.funding_interval_hours.clone()),
        funding_settlement_time: funding_context.map(|context| context.next_funding_time_ms.clone()),
        opened_at: opened_at.cloned(),
        closed_at: position_ref.closed_at.clone(),
        open_close_condition: Some(
            "开仓按资金费率价差信号；清仓由 funding-arb resident 在资金费结算完成或仓位失衡时用 reduce-only 订单退出/降风险。"
                .to_owned(),
        ),
        position_status: position_ref.status.clone(),
        position_quantity: leg.quantity.clone(),
        position_group_id: portfolio_resident_position_group_id(position_ref),
        position_group_label: portfolio_resident_position_group_label(position_ref),
        position_leg_role: Some(portfolio_funding_arb_leg_role_label(leg)),
        position_limit: position_ref.position_limit.clone(),
        manual_close_request_id: position_ref.manual_close_request_id.clone(),
        manual_close_request_status: position_ref.manual_close_request_status.clone(),
        manual_close_request_detail: position_ref.manual_close_request_detail.clone(),
        source: "funding-arb-resident-live".to_owned(),
    }
}

pub(crate) fn portfolio_funding_arb_leg_role_label(leg: &PortfolioFundingArbLegDisplay) -> String {
    if !portfolio_string_is_missing_or_unknown(&leg.role) {
        return leg.role.clone();
    }
    leg.side.clone()
}

pub(crate) fn portfolio_apply_resident_position_metadata(
    rows: &mut [PortfolioPositionRow],
    metadata_rows: &[PortfolioPositionRow],
) {
    for row in rows {
        let Some(metadata) = metadata_rows
            .iter()
            .find(|metadata| portfolio_position_row_matches_resident_metadata(row, metadata))
        else {
            continue;
        };
        if portfolio_string_is_missing_or_unknown(&row.strategy) {
            row.strategy = metadata.strategy.clone();
        }
        portfolio_fill_missing_option(&mut row.position_id, metadata.position_id.as_ref());
        portfolio_fill_missing_option(&mut row.position_kind, metadata.position_kind.as_ref());
        portfolio_fill_missing_option(&mut row.fee_rate_bps, metadata.fee_rate_bps.as_ref());
        portfolio_fill_missing_option(
            &mut row.settled_funding_usd,
            metadata.settled_funding_usd.as_ref(),
        );
        portfolio_fill_missing_option(
            &mut row.accumulated_position,
            metadata.accumulated_position.as_ref(),
        );
        portfolio_fill_missing_option(
            &mut row.open_average_price,
            metadata.open_average_price.as_ref(),
        );
        portfolio_fill_missing_option(
            &mut row.close_average_price,
            metadata.close_average_price.as_ref(),
        );
        portfolio_fill_missing_option(
            &mut row.open_close_spread_pct,
            metadata.open_close_spread_pct.as_ref(),
        );
        portfolio_fill_missing_option(
            &mut row.realtime_funding_rate,
            metadata.realtime_funding_rate.as_ref(),
        );
        portfolio_fill_missing_option(
            &mut row.funding_settlement_time,
            metadata.funding_settlement_time.as_ref(),
        );
        portfolio_fill_missing_option(&mut row.opened_at, metadata.opened_at.as_ref());
        portfolio_fill_missing_option(&mut row.closed_at, metadata.closed_at.as_ref());
        portfolio_fill_missing_option(
            &mut row.open_close_condition,
            metadata.open_close_condition.as_ref(),
        );
        portfolio_fill_missing_option(
            &mut row.position_group_id,
            metadata.position_group_id.as_ref(),
        );
        portfolio_fill_missing_option(
            &mut row.position_group_label,
            metadata.position_group_label.as_ref(),
        );
        portfolio_fill_missing_or_neutral_leg_role(
            &mut row.position_leg_role,
            metadata.position_leg_role.as_ref(),
        );
        portfolio_fill_missing_option(&mut row.position_limit, metadata.position_limit.as_ref());
        portfolio_fill_missing_option(
            &mut row.manual_close_request_id,
            metadata.manual_close_request_id.as_ref(),
        );
        portfolio_fill_missing_option(
            &mut row.manual_close_request_status,
            metadata.manual_close_request_status.as_ref(),
        );
        portfolio_fill_missing_option(
            &mut row.manual_close_request_detail,
            metadata.manual_close_request_detail.as_ref(),
        );
    }
}

pub(crate) fn portfolio_fill_missing_or_neutral_leg_role(
    target: &mut Option<String>,
    fallback: Option<&String>,
) {
    let should_fill = target.as_ref().is_none_or(|value| {
        portfolio_string_is_missing_or_unknown(value) || value.trim().eq_ignore_ascii_case("both")
    });
    if should_fill {
        *target = fallback.cloned();
    }
}

pub(crate) fn portfolio_fill_missing_option(
    target: &mut Option<String>,
    fallback: Option<&String>,
) {
    if target
        .as_ref()
        .is_none_or(|value| portfolio_string_is_missing_or_unknown(value))
    {
        *target = fallback.cloned();
    }
}

pub(crate) fn portfolio_string_is_missing_or_unknown(value: &str) -> bool {
    let value = value.trim();
    value.is_empty()
        || value == "-"
        || value.eq_ignore_ascii_case("unknown")
        || value.eq_ignore_ascii_case("null")
}

pub(crate) fn portfolio_position_row_matches_resident_metadata(
    row: &PortfolioPositionRow,
    metadata: &PortfolioPositionRow,
) -> bool {
    if funding_settlement_display_symbol_from_raw(&row.symbol)
        != funding_settlement_display_symbol_from_raw(&metadata.symbol)
    {
        return false;
    }
    let row_venue = normalize_venue_family(&row.venue_family);
    let venue_matches = metadata
        .venue_family
        .split('/')
        .map(normalize_venue_family)
        .any(|venue| venue == row_venue)
        || normalize_venue_family(&metadata.venue_family) == row_venue;
    let row_account = row.account_id.trim();
    let account_matches = !row_account.is_empty()
        && metadata
            .account_id
            .split('/')
            .map(str::trim)
            .any(|account_id| account_id == row_account);
    venue_matches || account_matches
}

pub(crate) fn portfolio_source_error_venue_family(source_error: &str) -> Option<String> {
    let (prefix, _) = source_error.split_once(':')?;
    let venue_family = normalize_venue_family(prefix);
    arb_venue_capability_profile(&venue_family).map(|_| venue_family)
}

pub(crate) fn portfolio_stale_position_status(status: &str) -> String {
    let status = status.trim();
    if status.is_empty() {
        return "stale_unknown".to_owned();
    }
    if status.starts_with("stale_") {
        return status.to_owned();
    }
    format!("stale_{status}")
}

pub(crate) fn portfolio_position_row_is_current_exchange_position(
    row: &PortfolioPositionRow,
) -> bool {
    if row.position_status != "flat" {
        return true;
    }
    MonitorDecimal::parse("portfolio.position_quantity", &row.position_quantity)
        .is_ok_and(|quantity| quantity.raw != 0)
}

pub(crate) fn portfolio_collect_resident_registry_dirs(
    root: &Path,
    remaining_depth: usize,
    registry_dirs: &mut Vec<PortfolioResidentRegistryDir>,
) -> RuntimeResult<()> {
    if root.join("resident_live_positions.jsonl").exists() {
        registry_dirs.push(PortfolioResidentRegistryDir {
            path: root.to_path_buf(),
            kind: PortfolioResidentPositionKind::SpotPerpBasis,
        });
    }
    if root.join("funding_arb_resident_positions.jsonl").exists() {
        registry_dirs.push(PortfolioResidentRegistryDir {
            path: root.to_path_buf(),
            kind: PortfolioResidentPositionKind::CrossExchangeFundingArb,
        });
    }
    if remaining_depth == 0 || !root.is_dir() {
        return Ok(());
    }
    for entry in fs::read_dir(root).map_err(|error| RuntimeError::Io {
        path: root.to_path_buf(),
        message: error.to_string(),
    })? {
        let entry = entry.map_err(|error| RuntimeError::Io {
            path: root.to_path_buf(),
            message: error.to_string(),
        })?;
        let path = entry.path();
        if path.is_dir() {
            portfolio_collect_resident_registry_dirs(&path, remaining_depth - 1, registry_dirs)?;
        }
    }
    Ok(())
}

pub(crate) fn portfolio_position_limit_from_config(dir: &Path) -> RuntimeResult<Option<String>> {
    for name in [
        "resident_live_config.json",
        "multi_venue_resident_live_config.json",
        "funding_arb_resident_live_config.json",
        "stack_config.json",
        "multi_venue_stack_config.json",
    ] {
        let path = dir.join(name);
        if !path.exists() {
            continue;
        }
        let input = read_utf8(&path)?;
        let fields = parse_json_object_value_slices(&input)?;
        let max_total =
            optional_json_value_string(&fields, "max_total_notional_usdt", "resident live config")?;
        let notional_usd =
            optional_json_value_string(&fields, "notional_usd", "resident live config")?;
        let max_concurrent = optional_json_value_string(
            &fields,
            "max_concurrent_positions",
            "resident live config",
        )?;
        let mut parts = Vec::new();
        if let Some(max_total) = max_total {
            parts.push(format!("总名义上限 {max_total} USDT"));
        }
        if let Some(notional_usd) = notional_usd {
            parts.push(format!("单仓名义 {notional_usd} USDT"));
        }
        if let Some(max_concurrent) = max_concurrent {
            parts.push(format!("并发仓位上限 {max_concurrent}"));
        }
        if !parts.is_empty() {
            return Ok(Some(parts.join("；")));
        }
    }
    Ok(None)
}

pub(crate) fn portfolio_taker_fee_bps_from_config(dir: &Path) -> RuntimeResult<Option<String>> {
    for name in [
        "resident_live_config.json",
        "multi_venue_resident_live_config.json",
        "funding_arb_resident_live_config.json",
        "stack_config.json",
        "multi_venue_stack_config.json",
    ] {
        let path = dir.join(name);
        if !path.exists() {
            continue;
        }
        let input = read_utf8(&path)?;
        let fields = parse_json_object_value_slices(&input)?;
        if let Some(value) =
            optional_json_value_string(&fields, "taker_fee_bps", "resident live config")?
        {
            return Ok(Some(value));
        }
    }
    Ok(None)
}

pub(crate) fn portfolio_resident_position_refs_from_dir(
    dir: &Path,
    kind: PortfolioResidentPositionKind,
    position_limit: Option<String>,
    taker_fee_bps: Option<String>,
) -> RuntimeResult<Vec<PortfolioResidentPositionRef>> {
    let path = match kind {
        PortfolioResidentPositionKind::SpotPerpBasis => dir.join("resident_live_positions.jsonl"),
        PortfolioResidentPositionKind::CrossExchangeFundingArb => {
            dir.join("funding_arb_resident_positions.jsonl")
        }
    };
    let input = read_utf8(&path)?;
    let mut positions = BTreeMap::<String, PortfolioResidentPositionRef>::new();
    for line in input.lines().map(str::trim).filter(|line| !line.is_empty()) {
        let fields = parse_json_object_value_slices(line)?;
        let event_type =
            required_json_value_string(&fields, "event_type", "resident position registry")?;
        match event_type.as_str() {
            "position_opened" => {
                let position_id = required_json_value_string(
                    &fields,
                    "position_id",
                    "resident position registry",
                )?;
                let position_state_path = required_json_value_string(
                    &fields,
                    "position_state_path",
                    "resident position registry",
                )?;
                let position_state_path =
                    portfolio_resolve_resident_path(dir, &position_state_path);
                let notional_usdt = required_json_value_string(
                    &fields,
                    "notional_usdt",
                    "resident position registry",
                )?;
                positions.insert(
                    position_id.clone(),
                    PortfolioResidentPositionRef {
                        kind,
                        position_id,
                        position_state_path,
                        pair_id: optional_json_value_string(
                            &fields,
                            "pair_id",
                            "resident position registry",
                        )?,
                        symbol: optional_json_value_string(
                            &fields,
                            "symbol",
                            "resident position registry",
                        )?,
                        notional_usdt,
                        status: "open".to_owned(),
                        net_funding_bps: optional_json_value_string(
                            &fields,
                            "net_funding_bps",
                            "resident position registry",
                        )?,
                        taker_fee_bps: taker_fee_bps.clone(),
                        position_limit: position_limit.clone(),
                        opened_at: optional_json_value_string(
                            &fields,
                            "opened_at",
                            "resident position registry",
                        )?,
                        closed_at: optional_json_value_string(
                            &fields,
                            "closed_at",
                            "resident position registry",
                        )?,
                        manual_close_request_id: None,
                        manual_close_request_status: None,
                        manual_close_request_detail: None,
                    },
                );
            }
            "position_closed" => {
                let position_id = required_json_value_string(
                    &fields,
                    "position_id",
                    "resident position registry",
                )?;
                if let Some(position) = positions.get_mut(&position_id) {
                    position.status = "closed".to_owned();
                    portfolio_update_resident_position_ref_from_fields(position, &fields)?;
                } else if let Some(position) = portfolio_resident_position_ref_from_registry_fields(
                    kind,
                    position_id,
                    "closed",
                    PathBuf::new(),
                    PortfolioResidentPositionRefDefaults {
                        default_notional: "0",
                        position_limit: &position_limit,
                        taker_fee_bps: taker_fee_bps.as_ref(),
                    },
                    &fields,
                )? {
                    positions.insert(position.position_id.clone(), position);
                }
            }
            "position_flat_cancelled" => {
                let position_id = required_json_value_string(
                    &fields,
                    "position_id",
                    "resident position registry",
                )?;
                if let Some(position) = portfolio_resident_position_ref_from_registry_fields(
                    kind,
                    position_id,
                    "flat_cancelled",
                    PathBuf::new(),
                    PortfolioResidentPositionRefDefaults {
                        default_notional: "0",
                        position_limit: &position_limit,
                        taker_fee_bps: taker_fee_bps.as_ref(),
                    },
                    &fields,
                )? {
                    positions.insert(position.position_id.clone(), position);
                }
            }
            "position_unknown" => {
                let position_id = required_json_value_string(
                    &fields,
                    "position_id",
                    "resident position registry",
                )?;
                if let Some(position) = positions.get_mut(&position_id) {
                    position.status = "unknown".to_owned();
                    portfolio_update_resident_position_ref_from_fields(position, &fields)?;
                } else {
                    let position_state_path = optional_json_value_string(
                        &fields,
                        "position_state_path",
                        "resident position registry",
                    )?
                    .map(|path| portfolio_resolve_resident_path(dir, &path))
                    .unwrap_or_default();
                    if let Some(position) = portfolio_resident_position_ref_from_registry_fields(
                        kind,
                        position_id,
                        "unknown",
                        position_state_path,
                        PortfolioResidentPositionRefDefaults {
                            default_notional: "unknown",
                            position_limit: &position_limit,
                            taker_fee_bps: taker_fee_bps.as_ref(),
                        },
                        &fields,
                    )? {
                        positions.insert(position.position_id.clone(), position);
                    }
                }
            }
            _ => {}
        }
    }
    if kind == PortfolioResidentPositionKind::CrossExchangeFundingArb {
        let manual_close_states = load_funding_arb_manual_close_request_states(dir)?;
        for position in positions.values_mut() {
            if let Some(state) = manual_close_states.get(&position.position_id) {
                position.manual_close_request_id = Some(state.request.request_id.clone());
                position.manual_close_request_status = Some(state.status.clone());
                position.manual_close_request_detail = state.detail.clone();
            }
        }
    }
    Ok(positions.into_values().collect())
}

pub(crate) fn portfolio_resident_position_ref_from_registry_fields(
    kind: PortfolioResidentPositionKind,
    position_id: String,
    status: &str,
    position_state_path: PathBuf,
    defaults: PortfolioResidentPositionRefDefaults<'_>,
    fields: &BTreeMap<String, &str>,
) -> RuntimeResult<Option<PortfolioResidentPositionRef>> {
    let pair_id = optional_json_value_string(fields, "pair_id", "resident position registry")?;
    let symbol = optional_json_value_string(fields, "symbol", "resident position registry")?;
    if pair_id.is_none() && symbol.is_none() {
        return Ok(None);
    }
    let notional_usdt =
        optional_json_value_string(fields, "notional_usdt", "resident position registry")?
            .unwrap_or_else(|| defaults.default_notional.to_owned());
    Ok(Some(PortfolioResidentPositionRef {
        kind,
        position_id,
        position_state_path,
        pair_id,
        symbol,
        notional_usdt,
        status: status.to_owned(),
        net_funding_bps: optional_json_value_string(
            fields,
            "net_funding_bps",
            "resident position registry",
        )?,
        taker_fee_bps: defaults.taker_fee_bps.cloned(),
        position_limit: defaults.position_limit.clone(),
        opened_at: optional_json_value_string(fields, "opened_at", "resident position registry")?,
        closed_at: optional_json_value_string(fields, "closed_at", "resident position registry")?,
        manual_close_request_id: None,
        manual_close_request_status: None,
        manual_close_request_detail: None,
    }))
}

pub(crate) struct PortfolioResidentPositionRefDefaults<'a> {
    default_notional: &'a str,
    position_limit: &'a Option<String>,
    taker_fee_bps: Option<&'a String>,
}

pub(crate) fn portfolio_update_resident_position_ref_from_fields(
    position: &mut PortfolioResidentPositionRef,
    fields: &BTreeMap<String, &str>,
) -> RuntimeResult<()> {
    if let Some(pair_id) =
        optional_json_value_string(fields, "pair_id", "resident position registry")?
    {
        position.pair_id = Some(pair_id);
    }
    if let Some(symbol) =
        optional_json_value_string(fields, "symbol", "resident position registry")?
    {
        position.symbol = Some(symbol);
    }
    if let Some(notional_usdt) =
        optional_json_value_string(fields, "notional_usdt", "resident position registry")?
    {
        position.notional_usdt = notional_usdt;
    }
    if let Some(net_funding_bps) =
        optional_json_value_string(fields, "net_funding_bps", "resident position registry")?
    {
        position.net_funding_bps = Some(net_funding_bps);
    }
    if let Some(opened_at) =
        optional_json_value_string(fields, "opened_at", "resident position registry")?
    {
        position.opened_at = Some(opened_at);
    }
    if let Some(closed_at) =
        optional_json_value_string(fields, "closed_at", "resident position registry")?
    {
        position.closed_at = Some(closed_at);
    }
    Ok(())
}

pub(crate) fn portfolio_resident_position_symbol(
    position_ref: &PortfolioResidentPositionRef,
) -> String {
    position_ref
        .symbol
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .map(funding_settlement_display_symbol_from_raw)
        .unwrap_or_else(|| position_ref.position_id.clone())
}

pub(crate) fn portfolio_resident_pair_venue_family(pair_id: Option<&str>) -> Option<String> {
    let pair_id = pair_id?.trim();
    let mut parts = pair_id.split(':');
    let venue_a = parts.next()?.trim();
    let venue_b = parts.next()?.trim();
    if venue_a.is_empty() || venue_b.is_empty() {
        return None;
    }
    Some(format!(
        "{} / {}",
        normalize_venue_family(venue_a),
        normalize_venue_family(venue_b)
    ))
}

pub(crate) fn portfolio_resident_position_venue_family(
    position_ref: &PortfolioResidentPositionRef,
) -> String {
    portfolio_resident_pair_venue_family(position_ref.pair_id.as_deref())
        .unwrap_or_else(|| "unknown".to_owned())
}

pub(crate) fn portfolio_resident_position_kind_label(
    kind: PortfolioResidentPositionKind,
) -> &'static str {
    match kind {
        PortfolioResidentPositionKind::SpotPerpBasis => "spot_perp_basis",
        PortfolioResidentPositionKind::CrossExchangeFundingArb => "cross_exchange_funding_arb",
    }
}

pub(crate) fn portfolio_resident_position_account_id(
    position_ref: &PortfolioResidentPositionRef,
) -> String {
    let Some(pair_id) = position_ref.pair_id.as_deref().map(str::trim) else {
        return "unknown".to_owned();
    };
    let accounts = pair_id
        .split(':')
        .take(2)
        .map(str::trim)
        .filter(|venue| !venue.is_empty())
        .map(|venue| {
            format!(
                "acct:{}-funding-arb-readonly",
                normalize_venue_family(venue)
            )
        })
        .collect::<Vec<_>>();
    if accounts.is_empty() {
        "unknown".to_owned()
    } else {
        accounts.join(" / ")
    }
}

pub(crate) fn portfolio_resident_position_group_id(
    position_ref: &PortfolioResidentPositionRef,
) -> Option<String> {
    (!position_ref.position_id.trim().is_empty()).then(|| position_ref.position_id.clone())
}

pub(crate) fn portfolio_resident_position_group_label(
    position_ref: &PortfolioResidentPositionRef,
) -> Option<String> {
    position_ref
        .pair_id
        .as_ref()
        .filter(|value| !value.trim().is_empty())
        .cloned()
        .or_else(|| portfolio_resident_position_group_id(position_ref))
}

pub(crate) fn portfolio_resident_notional_display(notional: &str) -> String {
    let trimmed = notional.trim();
    if trimmed.is_empty() {
        return "unknown".to_owned();
    }
    let upper = trimmed.to_ascii_uppercase();
    if upper == "UNKNOWN" || upper.ends_with(" USDT") || upper.ends_with(" USD") {
        trimmed.to_owned()
    } else {
        format!("{trimmed} USDT")
    }
}

pub(crate) fn portfolio_resident_position_quantity_display(
    position_ref: &PortfolioResidentPositionRef,
) -> String {
    if portfolio_resident_position_status_is_terminal(&position_ref.status) {
        "0 USDT".to_owned()
    } else {
        portfolio_resident_notional_display(&position_ref.notional_usdt)
    }
}

pub(crate) fn portfolio_resident_position_status_is_terminal(status: &str) -> bool {
    matches!(
        status.trim().to_ascii_lowercase().as_str(),
        "closed" | "flat_cancelled" | "flat"
    )
}

pub(crate) fn portfolio_resident_missing_state_status(
    position_ref: &PortfolioResidentPositionRef,
) -> String {
    if position_ref.status == "open" {
        "unknown".to_owned()
    } else {
        position_ref.status.clone()
    }
}

pub(crate) fn portfolio_position_row_from_resident_ref(
    position_ref: &PortfolioResidentPositionRef,
    funding_contexts: &[PortfolioFundingContext],
) -> RuntimeResult<PortfolioPositionRow> {
    if portfolio_resident_position_status_is_terminal(&position_ref.status) {
        return Ok(portfolio_terminal_position_row_from_resident_ref(
            position_ref,
        ));
    }
    if position_ref.kind == PortfolioResidentPositionKind::CrossExchangeFundingArb {
        return portfolio_funding_arb_position_row_from_resident_ref(
            position_ref,
            funding_contexts,
        );
    }

    let input = match read_utf8(&position_ref.position_state_path) {
        Ok(input) => input,
        Err(_) => {
            let symbol = portfolio_resident_position_symbol(position_ref);
            let position_status = portfolio_resident_missing_state_status(position_ref);
            let position_quantity = portfolio_resident_position_quantity_display(position_ref);
            return Ok(PortfolioPositionRow {
                position_id: Some(position_ref.position_id.clone()),
                position_kind: Some(portfolio_resident_position_kind_label(position_ref.kind).to_owned()),
                coin: funding_base_asset_from_symbol(&symbol),
                symbol,
                strategy: "resident-live".to_owned(),
                venue_family: portfolio_resident_position_venue_family(position_ref),
                account_id: portfolio_resident_position_account_id(position_ref),
                fee: None,
                fee_rate_bps: position_ref.taker_fee_bps.clone(),
                settled_funding_usd: None,
                accumulated_position: Some(position_quantity.clone()),
                open_average_price: None,
                close_average_price: None,
                open_close_spread_pct: None,
                realtime_funding_rate: None,
                realtime_funding_interval_hours: None,
                funding_settlement_time: None,
                opened_at: position_ref.opened_at.clone(),
                closed_at: position_ref.closed_at.clone(),
                open_close_condition: Some(
                    "仓位状态文件不可读；已使用 resident registry 降级展示，外部真实状态仍需私有快照或交易所确认。".to_owned(),
                ),
                position_status,
                position_quantity,
                position_group_id: portfolio_resident_position_group_id(position_ref),
                position_group_label: portfolio_resident_position_group_label(position_ref),
                position_leg_role: None,
                position_limit: position_ref.position_limit.clone(),
                manual_close_request_id: position_ref.manual_close_request_id.clone(),
                manual_close_request_status: position_ref.manual_close_request_status.clone(),
                manual_close_request_detail: position_ref.manual_close_request_detail.clone(),
                source: "resident-live".to_owned(),
            });
        }
    };
    let fields = parse_json_object_value_slices(&input)?;
    let symbol = required_json_value_string(&fields, "symbol", "resident position state")
        .map(|value| funding_settlement_display_symbol_from_raw(&value))?;
    let strategy = optional_json_value_string(&fields, "strategy_id", "resident position state")?
        .or_else(|| {
            optional_json_value_string(&fields, "strategy", "resident position state")
                .ok()
                .flatten()
        })
        .unwrap_or_else(|| "basis-resident-live".to_owned());
    let venue_family =
        optional_json_value_string(&fields, "venue_family", "resident position state")?
            .or_else(|| {
                optional_json_value_string(&fields, "spot_venue_id", "resident position state")
                    .ok()
                    .flatten()
                    .and_then(|value| portfolio_venue_family_from_venue_id(&value))
            })
            .map(|value| normalize_venue_family(&value))
            .unwrap_or_else(|| "unknown".to_owned());
    let spot_account =
        optional_json_value_string(&fields, "spot_account_id", "resident position state")?;
    let perp_account =
        optional_json_value_string(&fields, "perp_account_id", "resident position state")?;
    let account_id = portfolio_account_pair(spot_account.as_deref(), perp_account.as_deref());
    let notional_usd =
        optional_json_value_string(&fields, "notional_usd", "resident position state")?
            .unwrap_or_else(|| position_ref.notional_usdt.clone());
    let spot_quantity =
        optional_json_value_string(&fields, "spot_quantity", "resident position state")?;
    let perp_quantity =
        optional_json_value_string(&fields, "perp_quantity", "resident position state")?;
    let entry_gross_basis_bps =
        portfolio_optional_i128_field(&fields, "entry_gross_basis_bps", "resident position state")?;
    let entry_total_cost_bps =
        portfolio_optional_i128_field(&fields, "entry_total_cost_bps", "resident position state")?;
    let expected_next_funding_bps = portfolio_optional_i128_field(
        &fields,
        "expected_next_funding_bps",
        "resident position state",
    )?;
    let funding_context = portfolio_find_funding_context(funding_contexts, &venue_family, &symbol);
    let open_average_price =
        optional_json_value_string(&fields, "open_average_price", "resident position state")?
            .or_else(|| {
                optional_json_value_string(
                    &fields,
                    "entry_average_price",
                    "resident position state",
                )
                .ok()
                .flatten()
            })
            .or_else(|| {
                portfolio_average_price_from_notional_quantity(
                    &notional_usd,
                    spot_quantity.as_deref(),
                )
            });
    let realtime_funding_rate = funding_context
        .map(|context| context.funding_rate.clone())
        .or_else(|| expected_next_funding_bps.map(portfolio_bps_to_percent_string));
    let realtime_funding_interval_hours =
        funding_context.map(|context| context.funding_interval_hours.clone());
    let funding_settlement_time =
        funding_context.map(|context| context.next_funding_time_ms.clone());
    let opened_at = optional_json_value_string(&fields, "opened_at", "resident position state")?
        .or_else(|| position_ref.opened_at.clone());
    Ok(PortfolioPositionRow {
        position_id: Some(position_ref.position_id.clone()),
        position_kind: Some(portfolio_resident_position_kind_label(position_ref.kind).to_owned()),
        coin: funding_base_asset_from_symbol(&symbol),
        symbol,
        strategy,
        venue_family,
        account_id,
        fee: entry_total_cost_bps.map(|value| format!("{value} bps")),
        fee_rate_bps: position_ref.taker_fee_bps.clone(),
        settled_funding_usd: None,
        accumulated_position: Some(format!("{notional_usd} USDT")),
        open_average_price,
        close_average_price: None,
        open_close_spread_pct: entry_gross_basis_bps.map(portfolio_bps_to_percent_string),
        realtime_funding_rate,
        realtime_funding_interval_hours,
        funding_settlement_time,
        opened_at,
        closed_at: position_ref.closed_at.clone(),
        open_close_condition: Some(
            "开仓按 basis 入场信号；清仓由 basis-exit-supervisor 按收敛、止盈、止损和风险信号判断。".to_owned(),
        ),
        position_status: position_ref.status.clone(),
        position_quantity: portfolio_spot_perp_quantity(spot_quantity.as_deref(), perp_quantity.as_deref()),
        position_group_id: portfolio_resident_position_group_id(position_ref),
        position_group_label: portfolio_resident_position_group_label(position_ref),
        position_leg_role: None,
        position_limit: position_ref.position_limit.clone(),
        manual_close_request_id: position_ref.manual_close_request_id.clone(),
        manual_close_request_status: position_ref.manual_close_request_status.clone(),
        manual_close_request_detail: position_ref.manual_close_request_detail.clone(),
        source: "resident-live".to_owned(),
    })
}

pub(crate) fn portfolio_terminal_position_row_from_resident_ref(
    position_ref: &PortfolioResidentPositionRef,
) -> PortfolioPositionRow {
    let symbol = portfolio_resident_position_symbol(position_ref);
    let position_quantity = portfolio_resident_position_quantity_display(position_ref);
    PortfolioPositionRow {
        position_id: Some(position_ref.position_id.clone()),
        position_kind: Some(portfolio_resident_position_kind_label(position_ref.kind).to_owned()),
        coin: funding_base_asset_from_symbol(&symbol),
        symbol,
        strategy: if position_ref.kind == PortfolioResidentPositionKind::CrossExchangeFundingArb {
            "cross-exchange-funding-arb-resident-live".to_owned()
        } else {
            "resident-live".to_owned()
        },
        venue_family: portfolio_resident_position_venue_family(position_ref),
        account_id: portfolio_resident_position_account_id(position_ref),
        fee: None,
        fee_rate_bps: position_ref.taker_fee_bps.clone(),
        settled_funding_usd: None,
        accumulated_position: Some(position_quantity.clone()),
        open_average_price: None,
        close_average_price: None,
        open_close_spread_pct: None,
        realtime_funding_rate: None,
        realtime_funding_interval_hours: None,
        funding_settlement_time: None,
        opened_at: position_ref.opened_at.clone(),
        closed_at: position_ref.closed_at.clone(),
        open_close_condition: Some(
            "resident registry 已记录终态；数量按 0 展示，外部真实状态仍以最新私有快照或交易所为准。".to_owned(),
        ),
        position_status: position_ref.status.clone(),
        position_quantity,
        position_group_id: portfolio_resident_position_group_id(position_ref),
        position_group_label: portfolio_resident_position_group_label(position_ref),
        position_leg_role: None,
        position_limit: position_ref.position_limit.clone(),
        manual_close_request_id: position_ref.manual_close_request_id.clone(),
        manual_close_request_status: position_ref.manual_close_request_status.clone(),
        manual_close_request_detail: position_ref.manual_close_request_detail.clone(),
        source: if position_ref.kind == PortfolioResidentPositionKind::CrossExchangeFundingArb {
            "funding-arb-resident-live".to_owned()
        } else {
            "resident-live".to_owned()
        },
    }
}

pub(crate) fn portfolio_funding_arb_position_row_from_resident_ref(
    position_ref: &PortfolioResidentPositionRef,
    funding_contexts: &[PortfolioFundingContext],
) -> RuntimeResult<PortfolioPositionRow> {
    let input = match read_utf8(&position_ref.position_state_path) {
        Ok(input) => input,
        Err(_) => {
            let symbol = portfolio_resident_position_symbol(position_ref);
            let position_status = portfolio_resident_missing_state_status(position_ref);
            let position_quantity = portfolio_resident_position_quantity_display(position_ref);
            return Ok(PortfolioPositionRow {
                position_id: Some(position_ref.position_id.clone()),
                position_kind: Some(portfolio_resident_position_kind_label(position_ref.kind).to_owned()),
                coin: funding_base_asset_from_symbol(&symbol),
                symbol,
                strategy: "cross-exchange-funding-arb-resident-live".to_owned(),
                venue_family: portfolio_resident_position_venue_family(position_ref),
                account_id: portfolio_resident_position_account_id(position_ref),
                fee: None,
                fee_rate_bps: position_ref.taker_fee_bps.clone(),
                settled_funding_usd: None,
                accumulated_position: Some(position_quantity.clone()),
                open_average_price: None,
                close_average_price: None,
                open_close_spread_pct: None,
                realtime_funding_rate: None,
                realtime_funding_interval_hours: None,
                funding_settlement_time: None,
                opened_at: position_ref.opened_at.clone(),
                closed_at: position_ref.closed_at.clone(),
                open_close_condition: Some(
                    "资金费率套利仓位状态文件不可读；已使用 resident registry 降级展示，外部真实状态仍需私有快照或交易所确认。".to_owned(),
                ),
                position_status,
                position_quantity,
                position_group_id: portfolio_resident_position_group_id(position_ref),
                position_group_label: portfolio_resident_position_group_label(position_ref),
                position_leg_role: None,
                position_limit: position_ref.position_limit.clone(),
                manual_close_request_id: position_ref.manual_close_request_id.clone(),
                manual_close_request_status: position_ref.manual_close_request_status.clone(),
                manual_close_request_detail: position_ref.manual_close_request_detail.clone(),
                source: "funding-arb-resident-live".to_owned(),
            });
        }
    };
    let fields = parse_json_object_value_slices(&input)?;
    let symbol = required_json_value_string(&fields, "symbol", "funding arb position state")
        .map(|value| funding_settlement_display_symbol_from_raw(&value))?;
    let notional_usd =
        optional_json_value_string(&fields, "notional_usd", "funding arb position state")?
            .unwrap_or_else(|| position_ref.notional_usdt.clone());
    let entry_net_funding_bps = portfolio_optional_i128_field(
        &fields,
        "entry_net_funding_bps",
        "funding arb position state",
    )?
    .or_else(|| {
        position_ref
            .net_funding_bps
            .as_deref()
            .and_then(|value| value.trim().parse::<i128>().ok())
    });
    let leg_a = portfolio_funding_arb_leg_display(&fields, "leg_a")?;
    let leg_b = portfolio_funding_arb_leg_display(&fields, "leg_b")?;
    let venue_family = portfolio_funding_arb_leg_pair(&leg_a, &leg_b, |leg| {
        normalize_venue_family(&leg.venue_family)
    });
    let account_id = portfolio_funding_arb_leg_pair(&leg_a, &leg_b, |leg| leg.account_id.clone());
    let position_quantity = portfolio_funding_arb_leg_pair(&leg_a, &leg_b, |leg| {
        format!(
            "{} {} {} {}",
            normalize_venue_family(&leg.venue_family),
            leg.role,
            leg.side,
            leg.quantity
        )
    });
    let open_average_price = Some(portfolio_funding_arb_leg_pair(&leg_a, &leg_b, |leg| {
        leg.entry_limit_price.clone()
    }));
    let realtime_funding_rate =
        portfolio_funding_arb_realtime_funding_rate(funding_contexts, &symbol, &leg_a, &leg_b);
    let realtime_funding_interval_hours = portfolio_funding_arb_realtime_funding_interval_hours(
        funding_contexts,
        &symbol,
        &leg_a,
        &leg_b,
    );
    let funding_settlement_time =
        portfolio_funding_arb_settlement_time(funding_contexts, &symbol, &leg_a, &leg_b);
    let opened_at = optional_json_value_string(&fields, "opened_at", "funding arb position state")?
        .or_else(|| position_ref.opened_at.clone());
    Ok(PortfolioPositionRow {
        position_id: Some(position_ref.position_id.clone()),
        position_kind: Some(portfolio_resident_position_kind_label(position_ref.kind).to_owned()),
        coin: funding_base_asset_from_symbol(&symbol),
        symbol,
        strategy: "cross-exchange-funding-arb-resident-live".to_owned(),
        venue_family,
        account_id,
        fee: None,
        fee_rate_bps: position_ref.taker_fee_bps.clone(),
        settled_funding_usd: None,
        accumulated_position: Some(format!("{notional_usd} USDT")),
        open_average_price,
        close_average_price: None,
        open_close_spread_pct: entry_net_funding_bps.map(portfolio_bps_to_percent_string),
        realtime_funding_rate,
        realtime_funding_interval_hours,
        funding_settlement_time,
        opened_at,
        closed_at: position_ref.closed_at.clone(),
        open_close_condition: Some(
            "开仓按资金费率价差信号；清仓由 funding-arb resident 在资金费结算完成或仓位失衡时用 reduce-only 订单退出/降风险。"
                .to_owned(),
        ),
        position_status: position_ref.status.clone(),
        position_quantity,
        position_group_id: portfolio_resident_position_group_id(position_ref),
        position_group_label: portfolio_resident_position_group_label(position_ref),
        position_leg_role: None,
        position_limit: position_ref.position_limit.clone(),
        manual_close_request_id: position_ref.manual_close_request_id.clone(),
        manual_close_request_status: position_ref.manual_close_request_status.clone(),
        manual_close_request_detail: position_ref.manual_close_request_detail.clone(),
        source: "funding-arb-resident-live".to_owned(),
    })
}

pub(crate) fn portfolio_funding_arb_leg_display(
    fields: &BTreeMap<String, &str>,
    leg_field: &'static str,
) -> RuntimeResult<Option<PortfolioFundingArbLegDisplay>> {
    let Some(leg_json) = fields.get(leg_field).copied() else {
        return portfolio_funding_arb_flat_leg_display(fields, leg_field);
    };
    let leg_fields = parse_json_object_value_slices(leg_json)?;
    Ok(Some(PortfolioFundingArbLegDisplay {
        role: optional_json_value_string(&leg_fields, "role", "funding arb position leg")?
            .unwrap_or_else(|| leg_field.to_owned()),
        venue_family: optional_json_value_string(
            &leg_fields,
            "venue_family",
            "funding arb position leg",
        )?
        .unwrap_or_else(|| "unknown".to_owned()),
        account_id: optional_json_value_string(
            &leg_fields,
            "account_id",
            "funding arb position leg",
        )?
        .unwrap_or_else(|| "unknown".to_owned()),
        side: optional_json_value_string(&leg_fields, "side", "funding arb position leg")?
            .unwrap_or_else(|| "unknown".to_owned()),
        quantity: optional_json_value_string(&leg_fields, "quantity", "funding arb position leg")?
            .unwrap_or_else(|| "unknown".to_owned()),
        entry_limit_price: optional_json_value_string(
            &leg_fields,
            "entry_limit_price",
            "funding arb position leg",
        )?
        .unwrap_or_else(|| "unknown".to_owned()),
    }))
}

pub(crate) fn portfolio_funding_arb_flat_leg_display(
    fields: &BTreeMap<String, &str>,
    leg_field: &'static str,
) -> RuntimeResult<Option<PortfolioFundingArbLegDisplay>> {
    let role_field = format!("{leg_field}_role");
    let venue_family_field = format!("{leg_field}_venue_family");
    let venue_id_field = format!("{leg_field}_venue_id");
    let account_id_field = format!("{leg_field}_account_id");
    let side_field = format!("{leg_field}_side");
    let quantity_field = format!("{leg_field}_quantity");
    let entry_limit_price_field = format!("{leg_field}_entry_limit_price");
    if [
        role_field.as_str(),
        venue_family_field.as_str(),
        venue_id_field.as_str(),
        account_id_field.as_str(),
        side_field.as_str(),
        quantity_field.as_str(),
        entry_limit_price_field.as_str(),
    ]
    .iter()
    .all(|field| !fields.contains_key(*field))
    {
        return Ok(None);
    }

    let venue_family = optional_json_value_string_dynamic(
        fields,
        venue_family_field.as_str(),
        "funding arb position leg",
    )?
    .or_else(|| {
        optional_json_value_string_dynamic(
            fields,
            venue_id_field.as_str(),
            "funding arb position leg",
        )
        .ok()
        .flatten()
        .and_then(|value| portfolio_venue_family_from_venue_id(&value))
    })
    .unwrap_or_else(|| "unknown".to_owned());

    Ok(Some(PortfolioFundingArbLegDisplay {
        role: optional_json_value_string_dynamic(
            fields,
            role_field.as_str(),
            "funding arb position leg",
        )?
        .unwrap_or_else(|| leg_field.to_owned()),
        venue_family,
        account_id: optional_json_value_string_dynamic(
            fields,
            account_id_field.as_str(),
            "funding arb position leg",
        )?
        .unwrap_or_else(|| "unknown".to_owned()),
        side: optional_json_value_string_dynamic(
            fields,
            side_field.as_str(),
            "funding arb position leg",
        )?
        .unwrap_or_else(|| "unknown".to_owned()),
        quantity: optional_json_value_string_dynamic(
            fields,
            quantity_field.as_str(),
            "funding arb position leg",
        )?
        .unwrap_or_else(|| "unknown".to_owned()),
        entry_limit_price: optional_json_value_string_dynamic(
            fields,
            entry_limit_price_field.as_str(),
            "funding arb position leg",
        )?
        .unwrap_or_else(|| "unknown".to_owned()),
    }))
}

pub(crate) fn portfolio_funding_arb_leg_pair<F>(
    leg_a: &Option<PortfolioFundingArbLegDisplay>,
    leg_b: &Option<PortfolioFundingArbLegDisplay>,
    formatter: F,
) -> String
where
    F: Fn(&PortfolioFundingArbLegDisplay) -> String,
{
    match (leg_a, leg_b) {
        (Some(a), Some(b)) => format!("{} / {}", formatter(a), formatter(b)),
        (Some(a), None) => formatter(a),
        (None, Some(b)) => formatter(b),
        (None, None) => "unknown".to_owned(),
    }
}

pub(crate) fn portfolio_funding_arb_realtime_funding_rate(
    contexts: &[PortfolioFundingContext],
    symbol: &str,
    leg_a: &Option<PortfolioFundingArbLegDisplay>,
    leg_b: &Option<PortfolioFundingArbLegDisplay>,
) -> Option<String> {
    let mut seen = BTreeSet::new();
    let mut parts = Vec::new();
    for leg in [leg_a.as_ref(), leg_b.as_ref()].into_iter().flatten() {
        if let Some(context) = portfolio_find_funding_context(contexts, &leg.venue_family, symbol) {
            let label = normalize_venue_family(&leg.venue_family);
            let part = format!("{label}:{}", context.funding_rate);
            if seen.insert(part.clone()) {
                parts.push(part);
            }
        }
    }
    (!parts.is_empty()).then(|| parts.join(" / "))
}

pub(crate) fn portfolio_funding_arb_realtime_funding_interval_hours(
    contexts: &[PortfolioFundingContext],
    symbol: &str,
    leg_a: &Option<PortfolioFundingArbLegDisplay>,
    leg_b: &Option<PortfolioFundingArbLegDisplay>,
) -> Option<String> {
    let mut seen = BTreeSet::new();
    let mut parts = Vec::new();
    for leg in [leg_a.as_ref(), leg_b.as_ref()].into_iter().flatten() {
        if let Some(context) = portfolio_find_funding_context(contexts, &leg.venue_family, symbol) {
            let label = normalize_venue_family(&leg.venue_family);
            let part = format!("{label}:{}", context.funding_interval_hours);
            if seen.insert(part.clone()) {
                parts.push(part);
            }
        }
    }
    (!parts.is_empty()).then(|| parts.join(" / "))
}

pub(crate) fn portfolio_funding_arb_settlement_time(
    contexts: &[PortfolioFundingContext],
    symbol: &str,
    leg_a: &Option<PortfolioFundingArbLegDisplay>,
    leg_b: &Option<PortfolioFundingArbLegDisplay>,
) -> Option<String> {
    let mut seen = BTreeSet::new();
    let mut parts = Vec::new();
    for leg in [leg_a.as_ref(), leg_b.as_ref()].into_iter().flatten() {
        let Some(context) = portfolio_find_funding_context(contexts, &leg.venue_family, symbol)
        else {
            continue;
        };
        if context.next_funding_time_ms.trim().is_empty() {
            continue;
        }
        let label = normalize_venue_family(&leg.venue_family);
        let part = format!("{label}:{}", context.next_funding_time_ms);
        if seen.insert(part.clone()) {
            parts.push(part);
        }
    }
    (!parts.is_empty()).then(|| parts.join(" / "))
}

fn first_portfolio_balance_asset_from_raw_fields(
    fields: &BTreeMap<String, &str>,
) -> RuntimeResult<Option<String>> {
    for field in [
        "asset",
        "marginCoin",
        "coin",
        "ccy",
        "currency",
        "token",
        "symbol",
    ] {
        if let Some(value) = json_scalar_string_from_fields(fields, field)? {
            return Ok(Some(portfolio_balance_asset_from_raw_field(field, &value)));
        }
    }
    Ok(None)
}

pub(crate) fn portfolio_balance_asset_from_raw_field(field: &str, value: &str) -> String {
    let value = value.trim().to_ascii_uppercase();
    if field == "symbol" && value.ends_with("USDT") && value.len() > 4 {
        funding_base_asset_from_symbol(&value)
    } else {
        value.replace('-', "")
    }
}

pub(crate) fn portfolio_margin_buffer(
    margin_balance: Option<&str>,
    maintenance_margin: Option<&str>,
) -> Option<String> {
    let margin_balance = MonitorDecimal::parse("portfolio.margin_balance", margin_balance?).ok()?;
    let maintenance_margin =
        MonitorDecimal::parse("portfolio.maintenance_margin", maintenance_margin?).ok()?;
    margin_balance
        .checked_sub(maintenance_margin, "portfolio margin buffer")
        .ok()
        .map(MonitorDecimal::format_trimmed)
}

pub(crate) fn portfolio_strategy_from_account_id(account_id: &str) -> Option<String> {
    let lower = account_id.to_ascii_lowercase();
    if lower.contains("funding-arb") {
        Some("cross-exchange-funding-arb-resident-live".to_owned())
    } else if lower.contains("basis") {
        Some("spot-perp-basis-resident-live".to_owned())
    } else {
        None
    }
}

pub(crate) fn portfolio_fee_display(value: &str) -> String {
    value.trim().to_owned()
}

pub(crate) fn portfolio_price_spread_pct(
    open_price: Option<&str>,
    close_price: Option<&str>,
) -> Option<String> {
    let open_price = MonitorDecimal::parse("portfolio.open_average_price", open_price?).ok()?;
    let close_price = MonitorDecimal::parse("portfolio.close_average_price", close_price?).ok()?;
    if open_price.raw == 0 {
        return None;
    }
    let diff = close_price.raw.checked_sub(open_price.raw)?;
    let scaled = diff
        .checked_mul(100)
        .and_then(|value| value.checked_mul(1_000_000))
        .and_then(|value| value.checked_div(open_price.raw))?;
    Some(format_scaled_i128(scaled, 6))
}

pub(crate) fn portfolio_average_price_from_notional_quantity(
    notional: &str,
    quantity: Option<&str>,
) -> Option<String> {
    let notional = MonitorDecimal::parse("portfolio.notional", notional).ok()?;
    let quantity = MonitorDecimal::parse("portfolio.quantity", quantity?).ok()?;
    let quantity_abs = quantity.raw.checked_abs()?;
    if quantity_abs == 0 {
        return None;
    }
    let scale = 10_i128.pow(MonitorDecimal::SCALE_DIGITS as u32);
    let price_raw = notional.raw.checked_mul(scale)?.checked_div(quantity_abs)?;
    Some(MonitorDecimal { raw: price_raw }.format_trimmed())
}

pub(crate) fn portfolio_bps_to_percent_string(value: i128) -> String {
    format_scaled_i128(value, 2)
}

fn format_scaled_i128(value: i128, scale_digits: u32) -> String {
    let negative = value < 0;
    let raw = value.checked_abs().unwrap_or(i128::MAX);
    let scale = 10_i128.pow(scale_digits);
    let whole = raw / scale;
    let mut fraction = format!(
        "{:0width$}",
        raw % scale,
        width = usize::try_from(scale_digits).unwrap_or(0)
    );
    while fraction.ends_with('0') {
        fraction.pop();
    }
    let sign = if negative { "-" } else { "" };
    if fraction.is_empty() {
        format!("{sign}{whole}")
    } else {
        format!("{sign}{whole}.{fraction}")
    }
}

pub(crate) fn portfolio_find_funding_context<'a>(
    contexts: &'a [PortfolioFundingContext],
    venue_family: &str,
    symbol: &str,
) -> Option<&'a PortfolioFundingContext> {
    let venue_family = normalize_venue_family(venue_family);
    let symbol = funding_display_symbol(&funding_base_asset_from_symbol(symbol));
    contexts.iter().find(|context| {
        normalize_venue_family(&context.venue_family) == venue_family
            && funding_display_symbol(&funding_base_asset_from_symbol(&context.symbol)) == symbol
    })
}

pub(crate) fn portfolio_position_status(quantity: &str, explicit_status: Option<&str>) -> String {
    if let Some(status) = explicit_status.filter(|value| !value.trim().is_empty()) {
        let lower = status.trim().to_ascii_lowercase();
        if !matches!(
            lower.as_str(),
            "healthy" | "complete" | "matched" | "degraded" | "missing"
        ) {
            return status.trim().to_owned();
        }
    }
    match MonitorDecimal::parse("portfolio.position_quantity", quantity) {
        Ok(quantity) if quantity.raw == 0 => "flat".to_owned(),
        Ok(_) => "open".to_owned(),
        Err(_) => "unknown".to_owned(),
    }
}

pub(crate) fn portfolio_venue_family_from_venue_id(value: &str) -> Option<String> {
    let lower = value.to_ascii_lowercase();
    for family in ["binance", "bybit", "okx", "bitget", "aster", "hyperliquid"] {
        if lower.contains(family) {
            return Some(family.to_owned());
        }
    }
    None
}

pub(crate) fn portfolio_resolve_resident_path(dir: &Path, value: &str) -> PathBuf {
    let path = PathBuf::from(value);
    if path.is_absolute() {
        path
    } else {
        dir.join(path)
    }
}

pub(crate) fn portfolio_optional_i128_field(
    fields: &BTreeMap<String, &str>,
    field: &'static str,
    source: &'static str,
) -> RuntimeResult<Option<i128>> {
    let Some(value) = optional_json_value_string(fields, field, source)? else {
        return Ok(None);
    };
    value
        .parse::<i128>()
        .map(Some)
        .map_err(|_| RuntimeError::Module {
            module: "arb-runtime",
            message: format!("{source} field `{field}` is not an integer: `{value}`"),
        })
}

pub(crate) fn portfolio_account_pair(
    spot_account: Option<&str>,
    perp_account: Option<&str>,
) -> String {
    match (spot_account, perp_account) {
        (Some(spot), Some(perp)) if spot == perp => spot.to_owned(),
        (Some(spot), Some(perp)) => format!("spot={spot}; perp={perp}"),
        (Some(spot), None) => spot.to_owned(),
        (None, Some(perp)) => perp.to_owned(),
        (None, None) => "unknown".to_owned(),
    }
}

pub(crate) fn portfolio_spot_perp_quantity(
    spot_quantity: Option<&str>,
    perp_quantity: Option<&str>,
) -> String {
    match (spot_quantity, perp_quantity) {
        (Some(spot), Some(perp)) => format!("spot={spot}; perp={perp}"),
        (Some(spot), None) => spot.to_owned(),
        (None, Some(perp)) => perp.to_owned(),
        (None, None) => "unknown".to_owned(),
    }
}

fn start_portfolio_dashboard_http_api(
    bind_addr: &str,
    options: Arc<PortfolioDashboardOptions>,
) -> RuntimeResult<thread::JoinHandle<()>> {
    let listener = TcpListener::bind(bind_addr).map_err(|error| RuntimeError::LiveMarketData {
        message: format!("cannot bind portfolio dashboard HTTP API on {bind_addr}: {error}"),
    })?;
    let cache = Arc::new(RwLock::new(initial_portfolio_dashboard_cache_state()));
    let _cache_handle =
        start_portfolio_dashboard_cache_refresher(Arc::clone(&options), Arc::clone(&cache));
    let state = Arc::new(PortfolioDashboardHttpState { options, cache });
    let handle = thread::spawn(move || {
        for stream in listener.incoming() {
            match stream {
                Ok(stream) => {
                    let state = Arc::clone(&state);
                    thread::spawn(move || {
                        handle_portfolio_dashboard_http_connection(stream, &state)
                    });
                }
                Err(error) => eprintln!("portfolio-dashboard api accept failed: {error}"),
            }
        }
    });
    Ok(handle)
}

fn handle_portfolio_dashboard_http_connection(
    mut stream: TcpStream,
    state: &Arc<PortfolioDashboardHttpState>,
) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
    let mut buffer = [0_u8; 8192];
    let read = match stream.read(&mut buffer) {
        Ok(read) => read,
        Err(_) => return,
    };
    let request = String::from_utf8_lossy(&buffer[..read]);
    let first_line = request.lines().next().unwrap_or("");
    let mut parts = first_line.split_whitespace();
    let method = parts.next().unwrap_or("");
    let path = parts.next().unwrap_or("/");
    let route = path.split('?').next().unwrap_or(path);
    if method == "POST" && route == "/api/portfolio/positions/close" {
        let body = portfolio_http_request_body(&request);
        let (status, body) = portfolio_manual_close_request_http_json(state.options.as_ref(), body);
        let _ = write_http_json(&mut stream, status, &body);
        return;
    }
    if method != "GET" {
        let _ = write_http_json(&mut stream, 405, "{\"error\":\"method_not_allowed\"}");
        return;
    }
    if route == "/" || route == "/nav" || route == "/navigation" {
        let _ = write_http_json(
            &mut stream,
            410,
            &static_dashboard_gone_json("System navigation"),
        );
        return;
    }
    if route == "/dashboard" {
        let _ = write_http_json(&mut stream, 410, &static_dashboard_gone_json("Portfolio"));
        return;
    }
    if route == "/errors" || route == "/error-logs" {
        let _ = write_http_json(&mut stream, 410, &static_dashboard_gone_json("Error logs"));
        return;
    }
    if route == "/api/navigation/pages" {
        let _ = write_http_json(
            &mut stream,
            200,
            &system_navigation_pages_json_with_wss_pid_file(
                state.options.navigation_wss_pid_file.as_deref(),
            ),
        );
        return;
    }
    if route == "/api/errors/logs" {
        let snapshot = build_error_logs_snapshot(state.options.as_ref());
        let _ = write_http_json(&mut stream, 200, &error_logs_json(&snapshot));
        return;
    }

    let cache = state
        .cache
        .read()
        .expect("portfolio dashboard cache lock poisoned")
        .clone();
    let snapshot = portfolio_dashboard_snapshot_from_cache(&cache);
    let (status, body) = if route == "/health" {
        let http_status = if snapshot.status == "healthy" {
            200
        } else {
            503
        };
        (
            http_status,
            portfolio_dashboard_health_json_from_cache(&cache),
        )
    } else if route == "/api/portfolio/status" {
        (200, portfolio_dashboard_snapshot_json(&snapshot))
    } else if route == "/api/portfolio/balances" {
        (200, portfolio_balances_json(&snapshot))
    } else if route == "/api/portfolio/positions" {
        (200, portfolio_positions_json(&snapshot))
    } else {
        (
            404,
            "{\"error\":\"not_found\",\"paths\":[\"/health\",\"/api/navigation/pages\",\"/api/errors/logs\",\"/api/portfolio/status\",\"/api/portfolio/balances\",\"/api/portfolio/positions\",\"POST /api/portfolio/positions/close\"]}".to_owned(),
        )
    };
    let _ = write_http_json(&mut stream, status, &body);
}

fn portfolio_http_request_body(request: &str) -> &str {
    request
        .split_once("\r\n\r\n")
        .map(|(_, body)| body)
        .or_else(|| request.split_once("\n\n").map(|(_, body)| body))
        .unwrap_or("")
        .trim()
}

fn portfolio_manual_close_request_http_json(
    options: &PortfolioDashboardOptions,
    body: &str,
) -> (u16, String) {
    match portfolio_manual_close_request_http_json_inner(options, body) {
        Ok(result) => result,
        Err(error) => (
            400,
            portfolio_manual_close_error_json("bad_request", &error.to_string()),
        ),
    }
}

fn portfolio_manual_close_request_http_json_inner(
    options: &PortfolioDashboardOptions,
    body: &str,
) -> RuntimeResult<(u16, String)> {
    if !options.manual_close_enabled {
        return Ok((
            403,
            portfolio_manual_close_error_json(
                "manual_close_disabled",
                "portfolio-dashboard 未启用 --enable-manual-close，拒绝写入手动平仓请求。",
            ),
        ));
    }
    let Some(resident_root) = options.resident_root.as_ref() else {
        return Ok((
            503,
            portfolio_manual_close_error_json(
                "resident_root_missing",
                "portfolio-dashboard 未配置 --resident-root，不能定位 resident 仓位。",
            ),
        ));
    };
    let fields = parse_json_object_value_slices(body)?;
    let position_id =
        required_json_value_string(&fields, "position_id", "manual close request body")?;
    let confirm_position_id =
        required_json_value_string(&fields, "confirm_position_id", "manual close request body")?;
    if confirm_position_id != position_id {
        return Ok((
            400,
            portfolio_manual_close_error_json(
                "confirmation_mismatch",
                "confirm_position_id 必须与 position_id 完全一致。",
            ),
        ));
    }
    let idempotency_key =
        optional_json_value_string(&fields, "idempotency_key", "manual close request body")?;
    let requested_by =
        optional_json_value_string(&fields, "requested_by", "manual close request body")?
            .or_else(|| Some("easy-tool".to_owned()));
    let reason = optional_json_value_string(&fields, "reason", "manual close request body")?;
    let Some((output_root, position_ref)) =
        portfolio_find_open_funding_arb_position(resident_root, &position_id)?
    else {
        return Ok((
            404,
            portfolio_manual_close_error_json(
                "position_not_found",
                "未找到匹配的 active funding-arb resident 仓位；closed、unknown、basis 仓位不会接受一键平仓请求。",
            ),
        ));
    };

    let states = load_funding_arb_manual_close_request_states(&output_root)?;
    if let Some(state) = states.get(&position_id) {
        if funding_arb_manual_close_status_is_pending(&state.status) {
            let same_idempotency = idempotency_key
                .as_deref()
                .is_some_and(|key| state.request.idempotency_key.as_deref() == Some(key));
            if same_idempotency {
                return Ok((
                    202,
                    portfolio_manual_close_success_json(
                        &output_root,
                        &state.request,
                        &state.status,
                        true,
                    ),
                ));
            }
            return Ok((
                409,
                portfolio_manual_close_error_json(
                    "manual_close_already_pending",
                    "该仓位已有未完成的手动平仓请求。",
                ),
            ));
        }
    }

    let requested_at = current_utc_timestamp_string();
    let request = FundingArbManualCloseRequest {
        request_id: portfolio_manual_close_request_id(&position_id, &requested_at),
        position_id: position_ref.position_id,
        requested_at,
        requested_by,
        reason,
        idempotency_key,
    };
    append_funding_arb_manual_close_requested(&output_root, &request)?;
    Ok((
        202,
        portfolio_manual_close_success_json(&output_root, &request, "requested", false),
    ))
}

fn portfolio_find_open_funding_arb_position(
    resident_root: &Path,
    position_id: &str,
) -> RuntimeResult<Option<(PathBuf, PortfolioResidentPositionRef)>> {
    let mut registry_dirs = Vec::new();
    portfolio_collect_resident_registry_dirs(resident_root, 4, &mut registry_dirs)?;
    for registry_dir in registry_dirs {
        if registry_dir.kind != PortfolioResidentPositionKind::CrossExchangeFundingArb {
            continue;
        }
        let refs = portfolio_resident_position_refs_from_dir(
            &registry_dir.path,
            registry_dir.kind,
            None,
            None,
        )?;
        for position_ref in refs {
            if position_ref.position_id == position_id && position_ref.status == "open" {
                return Ok(Some((registry_dir.path, position_ref)));
            }
        }
    }
    Ok(None)
}

fn portfolio_manual_close_request_id(position_id: &str, requested_at: &str) -> String {
    let position_suffix = position_id
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch
            } else {
                '-'
            }
        })
        .collect::<String>()
        .trim_matches('-')
        .to_owned();
    let time_suffix = requested_at
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .collect::<String>();
    format!("manual-close:{position_suffix}:{time_suffix}")
}

fn portfolio_manual_close_success_json(
    output_root: &Path,
    request: &FundingArbManualCloseRequest,
    status: &str,
    idempotent_replay: bool,
) -> String {
    format!(
        "{{\"idempotent_replay\":{},\"order_type\":{},\"position_id\":{},\"request_id\":{},\"resident_root\":{},\"status\":{}}}",
        idempotent_replay,
        json_string(funding_arb_manual_close_order_type()),
        json_string(&request.position_id),
        json_string(&request.request_id),
        json_string(&output_root.display().to_string()),
        json_string(status),
    )
}

fn portfolio_manual_close_error_json(error: &str, message: &str) -> String {
    format!(
        "{{\"error\":{},\"message\":{}}}",
        json_string(error),
        json_string(message),
    )
}
