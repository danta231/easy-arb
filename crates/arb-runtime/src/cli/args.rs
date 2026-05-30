#[cfg(feature = "live-exec")]
use std::collections::BTreeSet;
use std::path::PathBuf;

use crate::*;

pub(crate) struct LiveMarketSimCliOptions {
    pub(crate) fixture_root: PathBuf,
    pub(crate) symbol: String,
    pub(crate) output_dir: Option<PathBuf>,
}

pub(crate) struct BinanceBasisScanCliOptions {
    pub(crate) symbol: String,
    pub(crate) output_dir: Option<PathBuf>,
}

pub(crate) type BinanceBasisPipelineCliOptions = BinanceBasisScanCliOptions;
pub(crate) type BybitBasisScanCliOptions = BinanceBasisScanCliOptions;
pub(crate) type BybitBasisPipelineCliOptions = BinanceBasisScanCliOptions;
pub(crate) struct BinanceGuardedLivePreviewCliOptions {
    pub(crate) market_artifacts_dir: PathBuf,
    pub(crate) output_dir: Option<PathBuf>,
    pub(crate) decision_request: Option<BinanceManualApprovalDecisionRequest>,
}
pub(crate) struct BinanceManualGateReleasePreviewCliOptions {
    pub(crate) preview_dir: PathBuf,
    pub(crate) output_dir: Option<PathBuf>,
}
pub(crate) struct BinancePreDispatchDryRunCliOptions {
    pub(crate) preview_dir: PathBuf,
    pub(crate) config_path: PathBuf,
    pub(crate) output_dir: Option<PathBuf>,
}
pub(crate) type BinanceGuardedLiveDispatchCliOptions = BinanceGuardedLiveDispatchOptions;
pub(crate) type BinanceBasisLiveStackCliOptions = BinanceBasisLiveStackOptions;
pub(crate) type BinanceBasisResidentLiveCliOptions = BinanceBasisResidentLiveOptions;
pub(crate) type MultiVenueBasisResidentLiveCliOptions = MultiVenueBasisResidentLiveOptions;
pub(crate) type MultiVenueBasisLiveStackCliOptions = MultiVenueBasisLiveStackOptions;
pub(crate) type BasisExitSupervisorCliOptions = BasisExitSupervisorOptions;
pub(crate) type BinanceBasisMonitorCliOptions = BinanceBasisMonitorOptions;
pub(crate) type BinanceWssBookTickerCliOptions = BinanceWssBookTickerMonitorOptions;
pub(crate) type BybitWssBookTickerCliOptions = BybitWssBookTickerMonitorOptions;
pub(crate) type OkxWssBookTickerCliOptions = OkxWssBookTickerMonitorOptions;
pub(crate) type BitgetWssBookTickerCliOptions = BitgetWssBookTickerMonitorOptions;
pub(crate) type AsterWssBookTickerCliOptions = AsterWssBookTickerMonitorOptions;
pub(crate) type HyperliquidWssBookTickerCliOptions = HyperliquidWssBookTickerMonitorOptions;
pub(crate) type BybitBasisMonitorCliOptions = BybitBasisMonitorOptions;
pub(crate) type OkxBasisMonitorCliOptions = OkxBasisMonitorOptions;
pub(crate) type BitgetBasisMonitorCliOptions = BitgetBasisMonitorOptions;
pub(crate) type HyperliquidBasisMonitorCliOptions = HyperliquidBasisMonitorOptions;
pub(crate) type AsterBasisMonitorCliOptions = AsterBasisMonitorOptions;
pub(crate) type FundingArbMonitorCliOptions = FundingArbMonitorOptions;
pub(crate) type PortfolioDashboardCliOptions = PortfolioDashboardOptions;
pub(crate) type FundingArbPrivateReadonlySnapshotOnceCliOptions =
    FundingArbPrivateReadonlySnapshotOnceOptions;
pub(crate) type FundingArbGuardedDryRunOnceCliOptions = FundingArbGuardedDryRunOnceOptions;
pub(crate) type FundingArbGuardedLiveCanaryOnceCliOptions = FundingArbGuardedLiveCanaryOnceOptions;
pub(crate) type FundingArbResidentLiveCliOptions = FundingArbResidentLiveOptions;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum UnifiedRuntimeMode {
    Live,
    Paper,
}

impl UnifiedRuntimeMode {
    pub(crate) fn from_command(command: &str) -> Option<Self> {
        match command {
            "live" | "official-live" | "real-live" | "正式实盘" => Some(Self::Live),
            "paper" | "test" | "test-live" | "测试盘" => Some(Self::Paper),
            _ => None,
        }
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Live => "正式实盘",
            Self::Paper => "测试盘",
        }
    }

    pub(crate) fn execution_mode_name(self) -> &'static str {
        match self {
            Self::Live => "live",
            Self::Paper => "paper",
        }
    }

    pub(crate) fn execute_live(self) -> bool {
        matches!(self, Self::Live)
    }

    pub(crate) fn default_output_dir(self) -> PathBuf {
        match self {
            Self::Live => PathBuf::from(UNIFIED_RUNTIME_LIVE_DEFAULT_OUT),
            Self::Paper => PathBuf::from(UNIFIED_RUNTIME_PAPER_DEFAULT_OUT),
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct UnifiedRuntimeCliOptions {
    pub(crate) mode: UnifiedRuntimeMode,
    pub(crate) config_path: PathBuf,
    pub(crate) output_dir: PathBuf,
    pub(crate) interval_secs: u64,
    pub(crate) min_net_bps: i128,
}

#[derive(Debug, Clone)]
pub(crate) struct UnifiedRuntimeReport {
    pub(crate) mode: UnifiedRuntimeMode,
    pub(crate) output_dir: PathBuf,
    pub(crate) exit_status: String,
}

pub(crate) fn parse_unified_runtime_args(
    mode: UnifiedRuntimeMode,
    args: &[String],
) -> RuntimeResult<UnifiedRuntimeCliOptions> {
    let mut config_path = PathBuf::from(UNIFIED_RUNTIME_DEFAULT_CONFIG);
    let mut output_dir = mode.default_output_dir();
    let mut interval_secs = UNIFIED_RUNTIME_DEFAULT_INTERVAL_SECS;
    let mut min_net_bps = BASIS_MONITOR_DEFAULT_MIN_NET_BPS;
    let mut acknowledged_live_orders = false;
    let mut index = 0;

    while index < args.len() {
        match args[index].as_str() {
            "--config" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--config requires a path"));
                };
                config_path = PathBuf::from(value);
            }
            "--out" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--out requires a directory"));
                };
                output_dir = PathBuf::from(value);
            }
            "--interval-secs" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--interval-secs requires a value"));
                };
                interval_secs = value
                    .parse::<u64>()
                    .map_err(|_| cli_arg_error("--interval-secs must be a positive integer"))?;
                if interval_secs == 0 {
                    return Err(cli_arg_error("--interval-secs must be greater than zero"));
                }
            }
            "--min-net-bps" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--min-net-bps requires a value"));
                };
                min_net_bps = value
                    .parse::<i128>()
                    .map_err(|_| cli_arg_error("--min-net-bps must be an integer"))?;
                if min_net_bps < 0 {
                    return Err(cli_arg_error("--min-net-bps must be non-negative"));
                }
            }
            "--i-understand-live-orders" | "--i-understand-real-orders" => {
                acknowledged_live_orders = true;
            }
            value if value.starts_with('-') => {
                return Err(cli_arg_error(format!(
                    "unknown {} option `{value}`",
                    mode.execution_mode_name()
                )));
            }
            value => {
                return Err(cli_arg_error(format!(
                    "unexpected {} positional argument `{value}`",
                    mode.execution_mode_name()
                )));
            }
        }
        index += 1;
    }

    if mode.execute_live() && !acknowledged_live_orders {
        return Err(cli_arg_error(
            "live mode requires --i-understand-live-orders because it can submit real orders",
        ));
    }

    Ok(UnifiedRuntimeCliOptions {
        mode,
        config_path,
        output_dir,
        interval_secs,
        min_net_bps,
    })
}

pub(crate) fn parse_live_market_sim_args(
    args: &[String],
) -> RuntimeResult<LiveMarketSimCliOptions> {
    let mut fixture_root = PathBuf::from(DEFAULT_FULL_PIPELINE_FIXTURE);
    let mut fixture_seen = false;
    let mut symbol = SIM_SYMBOL.to_owned();
    let mut output_dir = None;
    let mut index = 0;

    while index < args.len() {
        match args[index].as_str() {
            "--symbol" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--symbol requires a value"));
                };
                symbol = value.clone();
            }
            "--out" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--out requires a directory"));
                };
                output_dir = Some(PathBuf::from(value));
            }
            value if value.starts_with('-') => {
                return Err(cli_arg_error(format!(
                    "unknown live-market-sim option `{value}`"
                )));
            }
            value => {
                if fixture_seen {
                    return Err(cli_arg_error(format!(
                        "unexpected extra fixture path `{value}`"
                    )));
                }
                fixture_root = PathBuf::from(value);
                fixture_seen = true;
            }
        }
        index += 1;
    }

    Ok(LiveMarketSimCliOptions {
        fixture_root,
        symbol,
        output_dir,
    })
}

pub(crate) fn parse_binance_basis_scan_args(
    args: &[String],
) -> RuntimeResult<BinanceBasisScanCliOptions> {
    let mut symbol = BASIS_SYMBOL.to_owned();
    let mut output_dir = None;
    let mut index = 0;

    while index < args.len() {
        match args[index].as_str() {
            "--symbol" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--symbol requires a value"));
                };
                symbol = value.clone();
            }
            "--out" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--out requires a directory"));
                };
                output_dir = Some(PathBuf::from(value));
            }
            value if value.starts_with('-') => {
                return Err(cli_arg_error(format!(
                    "unknown binance-basis-scan option `{value}`"
                )));
            }
            value => {
                return Err(cli_arg_error(format!(
                    "unexpected binance-basis-scan positional argument `{value}`"
                )));
            }
        }
        index += 1;
    }

    Ok(BinanceBasisScanCliOptions { symbol, output_dir })
}

pub(crate) fn parse_binance_basis_pipeline_args(
    args: &[String],
) -> RuntimeResult<BinanceBasisPipelineCliOptions> {
    let mut symbol = BASIS_SYMBOL.to_owned();
    let mut output_dir = None;
    let mut index = 0;

    while index < args.len() {
        match args[index].as_str() {
            "--symbol" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--symbol requires a value"));
                };
                symbol = value.clone();
            }
            "--out" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--out requires a directory"));
                };
                output_dir = Some(PathBuf::from(value));
            }
            value if value.starts_with('-') => {
                return Err(cli_arg_error(format!(
                    "unknown binance-basis-pipeline option `{value}`"
                )));
            }
            value => {
                return Err(cli_arg_error(format!(
                    "unexpected binance-basis-pipeline positional argument `{value}`"
                )));
            }
        }
        index += 1;
    }

    Ok(BinanceBasisScanCliOptions { symbol, output_dir })
}

pub(crate) fn parse_bybit_basis_scan_args(
    args: &[String],
) -> RuntimeResult<BybitBasisScanCliOptions> {
    let mut symbol = BASIS_SYMBOL.to_owned();
    let mut output_dir = None;
    let mut index = 0;

    while index < args.len() {
        match args[index].as_str() {
            "--symbol" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--symbol requires a value"));
                };
                symbol = value.clone();
            }
            "--out" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--out requires a directory"));
                };
                output_dir = Some(PathBuf::from(value));
            }
            value if value.starts_with('-') => {
                return Err(cli_arg_error(format!(
                    "unknown bybit-basis-scan option `{value}`"
                )));
            }
            value => {
                return Err(cli_arg_error(format!(
                    "unexpected bybit-basis-scan positional argument `{value}`"
                )));
            }
        }
        index += 1;
    }

    Ok(BinanceBasisScanCliOptions { symbol, output_dir })
}

pub(crate) fn parse_bybit_basis_pipeline_args(
    args: &[String],
) -> RuntimeResult<BybitBasisPipelineCliOptions> {
    let mut symbol = BASIS_SYMBOL.to_owned();
    let mut output_dir = None;
    let mut index = 0;

    while index < args.len() {
        match args[index].as_str() {
            "--symbol" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--symbol requires a value"));
                };
                symbol = value.clone();
            }
            "--out" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--out requires a directory"));
                };
                output_dir = Some(PathBuf::from(value));
            }
            value if value.starts_with('-') => {
                return Err(cli_arg_error(format!(
                    "unknown bybit-basis-pipeline option `{value}`"
                )));
            }
            value => {
                return Err(cli_arg_error(format!(
                    "unexpected bybit-basis-pipeline positional argument `{value}`"
                )));
            }
        }
        index += 1;
    }

    Ok(BinanceBasisScanCliOptions { symbol, output_dir })
}

pub(crate) fn parse_binance_guarded_live_preview_args(
    args: &[String],
) -> RuntimeResult<BinanceGuardedLivePreviewCliOptions> {
    let mut market_artifacts_dir = PathBuf::from(BINANCE_BASIS_PIPELINE_DEFAULT_OUT);
    let mut output_dir = None;
    let mut decision = None;
    let mut expected_plan_hash = None;
    let mut approval_event_id = None;
    let mut reviewer_id = None;
    let mut decided_at = None;
    let mut expires_at = None;
    let mut reason = None;
    let mut index = 0;

    while index < args.len() {
        match args[index].as_str() {
            "--market-artifacts" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--market-artifacts requires a directory"));
                };
                market_artifacts_dir = PathBuf::from(value);
            }
            "--out" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--out requires a directory"));
                };
                output_dir = Some(PathBuf::from(value));
            }
            "--decision" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--decision requires approve or reject"));
                };
                decision = Some(parse_manual_approval_decision(value)?);
            }
            "--expected-plan-hash" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--expected-plan-hash requires a value"));
                };
                expected_plan_hash = Some(value.clone());
            }
            "--approval-event-id" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--approval-event-id requires a value"));
                };
                approval_event_id = Some(value.clone());
            }
            "--reviewer" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--reviewer requires a value"));
                };
                reviewer_id = Some(value.clone());
            }
            "--decided-at" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error(
                        "--decided-at requires an RFC3339 UTC timestamp",
                    ));
                };
                decided_at = Some(value.clone());
            }
            "--expires-at" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error(
                        "--expires-at requires an RFC3339 UTC timestamp",
                    ));
                };
                expires_at = Some(value.clone());
            }
            "--reason" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--reason requires a value"));
                };
                reason = Some(value.clone());
            }
            value if value.starts_with('-') => {
                return Err(cli_arg_error(format!(
                    "unknown binance-guarded-live-preview option `{value}`"
                )));
            }
            value => {
                return Err(cli_arg_error(format!(
                    "unexpected binance-guarded-live-preview positional argument `{value}`"
                )));
            }
        }
        index += 1;
    }

    let decision_request = if let Some(decision) = decision {
        Some(BinanceManualApprovalDecisionRequest {
            decision,
            expected_plan_hash: expected_plan_hash.ok_or_else(|| {
                cli_arg_error("--expected-plan-hash is required when --decision is supplied")
            })?,
            approval_event_id: approval_event_id.ok_or_else(|| {
                cli_arg_error("--approval-event-id is required when --decision is supplied")
            })?,
            reviewer_id: reviewer_id
                .ok_or_else(|| cli_arg_error("--reviewer is required when --decision is supplied"))?,
            decided_at: decided_at.ok_or_else(|| {
                cli_arg_error("--decided-at is required when --decision is supplied")
            })?,
            expires_at: expires_at.ok_or_else(|| {
                cli_arg_error("--expires-at is required when --decision is supplied")
            })?,
            reason: reason.unwrap_or_else(|| {
                "Manual approval decision recorded for Binance BTCUSDT GuardedLivePersonal plan preview."
                    .to_owned()
            }),
        })
    } else {
        if expected_plan_hash.is_some()
            || approval_event_id.is_some()
            || reviewer_id.is_some()
            || decided_at.is_some()
            || expires_at.is_some()
            || reason.is_some()
        {
            return Err(cli_arg_error(
                "manual approval fields require --decision approve|reject",
            ));
        }
        None
    };

    Ok(BinanceGuardedLivePreviewCliOptions {
        market_artifacts_dir,
        output_dir,
        decision_request,
    })
}

pub(crate) fn parse_binance_manual_gate_release_preview_args(
    args: &[String],
) -> RuntimeResult<BinanceManualGateReleasePreviewCliOptions> {
    let mut preview_dir = PathBuf::from(BINANCE_GUARDED_LIVE_PREVIEW_DEFAULT_OUT);
    let mut output_dir = None;
    let mut index = 0;

    while index < args.len() {
        match args[index].as_str() {
            "--preview-dir" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--preview-dir requires a directory"));
                };
                preview_dir = PathBuf::from(value);
            }
            "--out" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--out requires a directory"));
                };
                output_dir = Some(PathBuf::from(value));
            }
            value if value.starts_with('-') => {
                return Err(cli_arg_error(format!(
                    "unknown binance-guarded-live-gate-release-preview option `{value}`"
                )));
            }
            value => {
                return Err(cli_arg_error(format!(
                    "unexpected binance-guarded-live-gate-release-preview positional argument `{value}`"
                )));
            }
        }
        index += 1;
    }

    Ok(BinanceManualGateReleasePreviewCliOptions {
        preview_dir,
        output_dir,
    })
}

pub(crate) fn parse_binance_pre_dispatch_dry_run_args(
    args: &[String],
) -> RuntimeResult<BinancePreDispatchDryRunCliOptions> {
    let mut preview_dir = PathBuf::from(BINANCE_GUARDED_LIVE_PREVIEW_DEFAULT_OUT);
    let mut config_path = PathBuf::from("templates/personal_guarded_live.preflight.yaml");
    let mut output_dir = None;
    let mut index = 0;

    while index < args.len() {
        match args[index].as_str() {
            "--preview-dir" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--preview-dir requires a directory"));
                };
                preview_dir = PathBuf::from(value);
            }
            "--config" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--config requires a config path"));
                };
                config_path = PathBuf::from(value);
            }
            "--out" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--out requires a directory"));
                };
                output_dir = Some(PathBuf::from(value));
            }
            value if value.starts_with('-') => {
                return Err(cli_arg_error(format!(
                    "unknown binance-guarded-live-pre-dispatch-dry-run option `{value}`"
                )));
            }
            value => {
                return Err(cli_arg_error(format!(
                    "unexpected binance-guarded-live-pre-dispatch-dry-run positional argument `{value}`"
                )));
            }
        }
        index += 1;
    }

    Ok(BinancePreDispatchDryRunCliOptions {
        preview_dir,
        config_path,
        output_dir,
    })
}

pub(crate) fn parse_binance_guarded_live_dispatch_args(
    args: &[String],
) -> RuntimeResult<BinanceGuardedLiveDispatchCliOptions> {
    let mut preview_dir = PathBuf::from(BINANCE_GUARDED_LIVE_PREVIEW_DEFAULT_OUT);
    let mut config_path = PathBuf::from("templates/personal_guarded_live.preflight.yaml");
    let mut output_dir = None;
    let mut acknowledge_live_orders = false;
    let mut index = 0;

    while index < args.len() {
        match args[index].as_str() {
            "--preview-dir" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--preview-dir requires a directory"));
                };
                preview_dir = PathBuf::from(value);
            }
            "--config" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--config requires a config path"));
                };
                config_path = PathBuf::from(value);
            }
            "--out" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--out requires a directory"));
                };
                output_dir = Some(PathBuf::from(value));
            }
            "--i-understand-live-orders" => {
                acknowledge_live_orders = true;
            }
            value if value.starts_with('-') => {
                return Err(cli_arg_error(format!(
                    "unknown binance-guarded-live-dispatch option `{value}`"
                )));
            }
            value => {
                return Err(cli_arg_error(format!(
                    "unexpected binance-guarded-live-dispatch positional argument `{value}`"
                )));
            }
        }
        index += 1;
    }

    Ok(BinanceGuardedLiveDispatchOptions {
        preview_dir,
        config_path,
        output_dir,
        acknowledge_live_orders,
    })
}

#[cfg(test)]
pub(crate) fn parse_basis_guarded_live_cycle_args(
    args: &[String],
    command_name: &'static str,
    default_symbol: &'static str,
) -> RuntimeResult<BasisGuardedLiveCycleOptions> {
    let mut symbol = default_symbol.to_owned();
    let mut config_path = PathBuf::from("templates/personal_guarded_live.preflight.yaml");
    let mut output_dir = None;
    let mut min_net_bps = BASIS_MONITOR_DEFAULT_MIN_NET_BPS;
    let mut max_spot_ask = None;
    let mut min_perp_bid = None;
    let mut auto_price_guard_bps = None;
    let mut spot_wss_monitor_url = None;
    let mut perp_wss_monitor_url = None;
    let mut private_order_events_dir = None;
    let mut execute_live = false;
    let mut acknowledge_basis_live_orders = false;
    let mut index = 0;

    while index < args.len() {
        match args[index].as_str() {
            "--symbol" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--symbol requires a value"));
                };
                symbol = value.clone();
            }
            "--config" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--config requires a config path"));
                };
                config_path = PathBuf::from(value);
            }
            "--out" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--out requires a directory"));
                };
                output_dir = Some(PathBuf::from(value));
            }
            "--min-net-bps" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--min-net-bps requires an integer"));
                };
                min_net_bps = value.parse::<i128>().map_err(|error| {
                    cli_arg_error(format!("--min-net-bps must be an integer: {error}"))
                })?;
            }
            "--max-spot-ask" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--max-spot-ask requires a price"));
                };
                Decimal::from_str(value)?;
                max_spot_ask = Some(value.clone());
            }
            "--min-perp-bid" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--min-perp-bid requires a price"));
                };
                Decimal::from_str(value)?;
                min_perp_bid = Some(value.clone());
            }
            "--auto-price-guard-bps" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--auto-price-guard-bps requires an integer"));
                };
                let bps = value.parse::<i128>().map_err(|error| {
                    cli_arg_error(format!(
                        "--auto-price-guard-bps must be an integer: {error}"
                    ))
                })?;
                if !(0..=BASIS_AUTO_PRICE_GUARD_MAX_BPS).contains(&bps) {
                    return Err(cli_arg_error(format!(
                        "--auto-price-guard-bps must be between 0 and {BASIS_AUTO_PRICE_GUARD_MAX_BPS}"
                    )));
                }
                auto_price_guard_bps = Some(bps);
            }
            "--spot-wss-monitor-url" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--spot-wss-monitor-url requires a URL"));
                };
                spot_wss_monitor_url = Some(value.clone());
            }
            "--perp-wss-monitor-url" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--perp-wss-monitor-url requires a URL"));
                };
                perp_wss_monitor_url = Some(value.clone());
            }
            "--private-order-events-dir" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error(
                        "--private-order-events-dir requires a directory",
                    ));
                };
                private_order_events_dir = Some(PathBuf::from(value));
            }
            "--execute-live" => {
                execute_live = true;
            }
            "--dry-run" => {
                execute_live = false;
            }
            "--i-understand-basis-live-orders" => {
                acknowledge_basis_live_orders = true;
            }
            value if value.starts_with('-') => {
                return Err(cli_arg_error(format!(
                    "unknown {command_name} option `{value}`"
                )));
            }
            value => {
                return Err(cli_arg_error(format!(
                    "unexpected {command_name} positional argument `{value}`"
                )));
            }
        }
        index += 1;
    }
    if auto_price_guard_bps.is_some() && (max_spot_ask.is_some() || min_perp_bid.is_some()) {
        return Err(cli_arg_error(
            "--auto-price-guard-bps cannot be combined with --max-spot-ask or --min-perp-bid",
        ));
    }

    Ok(BasisGuardedLiveCycleOptions {
        symbol,
        config_path,
        output_dir,
        min_net_bps,
        max_spot_ask,
        min_perp_bid,
        auto_price_guard_bps,
        spot_wss_monitor_url,
        perp_wss_monitor_url,
        private_order_events_dir,
        execute_live,
        acknowledge_basis_live_orders,
    })
}

#[cfg(test)]
pub(crate) fn parse_binance_basis_guarded_live_cycle_args(
    args: &[String],
) -> RuntimeResult<BasisGuardedLiveCycleOptions> {
    parse_basis_guarded_live_cycle_args(args, "binance-basis-guarded-live-cycle", BASIS_SYMBOL)
}

pub(crate) fn parse_binance_basis_live_stack_args(
    args: &[String],
) -> RuntimeResult<BinanceBasisLiveStackCliOptions> {
    let mut symbol = BASIS_SYMBOL.to_owned();
    let mut config_path = PathBuf::from("templates/personal_guarded_live.preflight.yaml");
    let mut output_dir = None;
    let mut min_net_bps = BASIS_MONITOR_DEFAULT_MIN_NET_BPS;
    let mut auto_price_guard_bps = Some(2_i128);
    let mut spot_wss_bind_addr = BINANCE_WSS_BOOK_TICKER_DEFAULT_BIND_ADDR.to_owned();
    let mut perp_wss_bind_addr = "127.0.0.1:8802".to_owned();
    let mut monitor_symbol = None;
    let mut monitor_reconnect_delay_secs = BINANCE_WSS_BOOK_TICKER_DEFAULT_RECONNECT_DELAY_SECS;
    let mut readiness_timeout_secs = 60_u64;
    let mut shutdown_grace_secs = 5_u64;
    let mut private_order_events_dir = None;
    let mut adl_events_dir = None;
    let mut position_state_path = None;
    let mut poll_interval_secs = 60_u64;
    let mut max_cycles = None;
    let mut max_concurrent_positions = 1_usize;
    let mut max_total_notional_usdt = BINANCE_GUARDED_LIVE_NOTIONAL_USDT.to_owned();
    let mut use_existing_monitors = false;
    let mut execute_live = false;
    let mut acknowledge_basis_live_orders = false;
    let mut index = 0;

    while index < args.len() {
        match args[index].as_str() {
            "--symbol" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--symbol requires a value"));
                };
                symbol = value.clone();
            }
            "--config" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--config requires a config path"));
                };
                config_path = PathBuf::from(value);
            }
            "--out" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--out requires a directory"));
                };
                output_dir = Some(PathBuf::from(value));
            }
            "--min-net-bps" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--min-net-bps requires an integer"));
                };
                min_net_bps = value.parse::<i128>().map_err(|error| {
                    cli_arg_error(format!("--min-net-bps must be an integer: {error}"))
                })?;
            }
            "--auto-price-guard-bps" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--auto-price-guard-bps requires an integer"));
                };
                let bps = value.parse::<i128>().map_err(|error| {
                    cli_arg_error(format!(
                        "--auto-price-guard-bps must be an integer: {error}"
                    ))
                })?;
                if !(0..=BASIS_AUTO_PRICE_GUARD_MAX_BPS).contains(&bps) {
                    return Err(cli_arg_error(format!(
                        "--auto-price-guard-bps must be between 0 and {BASIS_AUTO_PRICE_GUARD_MAX_BPS}"
                    )));
                }
                auto_price_guard_bps = Some(bps);
            }
            "--spot-wss-bind" | "--spot-wss-bind-addr" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--spot-wss-bind requires host:port"));
                };
                spot_wss_bind_addr = value.clone();
            }
            "--perp-wss-bind" | "--perp-wss-bind-addr" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--perp-wss-bind requires host:port"));
                };
                perp_wss_bind_addr = value.clone();
            }
            "--monitor-symbol" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--monitor-symbol requires a value"));
                };
                monitor_symbol = Some(value.clone());
            }
            "--monitor-reconnect-delay-secs" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error(
                        "--monitor-reconnect-delay-secs requires an integer",
                    ));
                };
                monitor_reconnect_delay_secs = value.parse::<u64>().map_err(|error| {
                    cli_arg_error(format!(
                        "--monitor-reconnect-delay-secs must be an integer: {error}"
                    ))
                })?;
            }
            "--readiness-timeout-secs" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error(
                        "--readiness-timeout-secs requires an integer",
                    ));
                };
                readiness_timeout_secs = value.parse::<u64>().map_err(|error| {
                    cli_arg_error(format!(
                        "--readiness-timeout-secs must be an integer: {error}"
                    ))
                })?;
            }
            "--shutdown-grace-secs" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--shutdown-grace-secs requires an integer"));
                };
                shutdown_grace_secs = value.parse::<u64>().map_err(|error| {
                    cli_arg_error(format!("--shutdown-grace-secs must be an integer: {error}"))
                })?;
            }
            "--private-order-events-dir" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error(
                        "--private-order-events-dir requires a directory",
                    ));
                };
                private_order_events_dir = Some(PathBuf::from(value));
            }
            "--adl-events-dir" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--adl-events-dir requires a directory"));
                };
                adl_events_dir = Some(PathBuf::from(value));
            }
            "--position-state" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--position-state requires a file path"));
                };
                position_state_path = Some(PathBuf::from(value));
            }
            "--interval-secs" | "--poll-interval-secs" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--interval-secs requires an integer"));
                };
                poll_interval_secs = value.parse::<u64>().map_err(|error| {
                    cli_arg_error(format!("--interval-secs must be an integer: {error}"))
                })?;
            }
            "--max-cycles" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--max-cycles requires an integer"));
                };
                let parsed = value.parse::<u64>().map_err(|error| {
                    cli_arg_error(format!("--max-cycles must be an integer: {error}"))
                })?;
                if parsed == 0 {
                    return Err(cli_arg_error("--max-cycles must be greater than zero"));
                }
                max_cycles = Some(parsed);
            }
            "--max-concurrent-positions" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error(
                        "--max-concurrent-positions requires an integer",
                    ));
                };
                max_concurrent_positions = value.parse::<usize>().map_err(|error| {
                    cli_arg_error(format!(
                        "--max-concurrent-positions must be an integer: {error}"
                    ))
                })?;
            }
            "--max-total-notional-usdt" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error(
                        "--max-total-notional-usdt requires an amount",
                    ));
                };
                Decimal::from_str(value)?;
                max_total_notional_usdt = value.clone();
            }
            "--use-existing-monitors" => {
                use_existing_monitors = true;
            }
            "--execute-live" => {
                execute_live = true;
            }
            "--dry-run" => {
                execute_live = false;
            }
            "--i-understand-basis-live-orders" => {
                acknowledge_basis_live_orders = true;
            }
            value if value.starts_with('-') => {
                return Err(cli_arg_error(format!(
                    "unknown binance-basis-live-stack option `{value}`"
                )));
            }
            value => {
                return Err(cli_arg_error(format!(
                    "unexpected binance-basis-live-stack positional argument `{value}`"
                )));
            }
        }
        index += 1;
    }

    if poll_interval_secs == 0 {
        return Err(cli_arg_error("--interval-secs must be greater than zero"));
    }
    if readiness_timeout_secs == 0 {
        return Err(cli_arg_error(
            "--readiness-timeout-secs must be greater than zero",
        ));
    }
    if shutdown_grace_secs == 0 {
        return Err(cli_arg_error(
            "--shutdown-grace-secs must be greater than zero",
        ));
    }
    if monitor_reconnect_delay_secs == 0 {
        return Err(cli_arg_error(
            "--monitor-reconnect-delay-secs must be greater than zero",
        ));
    }
    if max_concurrent_positions == 0 {
        return Err(cli_arg_error(
            "--max-concurrent-positions must be greater than zero",
        ));
    }
    let max_total = Decimal::from_str(&max_total_notional_usdt)?;
    if max_total.is_negative() || max_total.is_zero() {
        return Err(cli_arg_error(
            "--max-total-notional-usdt must be greater than zero",
        ));
    }

    Ok(BinanceBasisLiveStackOptions {
        symbol,
        config_path,
        output_dir,
        min_net_bps,
        auto_price_guard_bps,
        spot_wss_bind_addr,
        perp_wss_bind_addr,
        monitor_symbol,
        monitor_reconnect_delay_secs,
        readiness_timeout_secs,
        shutdown_grace_secs,
        private_order_events_dir,
        adl_events_dir,
        position_state_path,
        poll_interval_secs,
        max_cycles,
        max_concurrent_positions,
        max_total_notional_usdt,
        use_existing_monitors,
        execute_live,
        acknowledge_basis_live_orders,
    })
}

pub(crate) fn parse_binance_basis_resident_live_args(
    args: &[String],
) -> RuntimeResult<BinanceBasisResidentLiveCliOptions> {
    let mut symbol = BASIS_SYMBOL.to_owned();
    let mut config_path = PathBuf::from("templates/personal_guarded_live.preflight.yaml");
    let mut output_dir = None;
    let mut min_net_bps = BASIS_MONITOR_DEFAULT_MIN_NET_BPS;
    let mut auto_price_guard_bps = Some(2_i128);
    let mut spot_wss_monitor_url = "http://127.0.0.1:8801".to_owned();
    let mut perp_wss_monitor_url = "http://127.0.0.1:8802".to_owned();
    let mut private_order_events_dir = None;
    let mut adl_events_dir = None;
    let mut position_state_path = None;
    let mut poll_interval_secs = 60_u64;
    let mut max_cycles = None;
    let mut max_concurrent_positions = 1_usize;
    let mut max_total_notional_usdt = BINANCE_GUARDED_LIVE_NOTIONAL_USDT.to_owned();
    let mut execute_live = false;
    let mut acknowledge_basis_live_orders = false;
    let mut index = 0;

    while index < args.len() {
        match args[index].as_str() {
            "--symbol" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--symbol requires a value"));
                };
                symbol = value.clone();
            }
            "--config" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--config requires a config path"));
                };
                config_path = PathBuf::from(value);
            }
            "--out" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--out requires a directory"));
                };
                output_dir = Some(PathBuf::from(value));
            }
            "--min-net-bps" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--min-net-bps requires an integer"));
                };
                min_net_bps = value.parse::<i128>().map_err(|error| {
                    cli_arg_error(format!("--min-net-bps must be an integer: {error}"))
                })?;
            }
            "--auto-price-guard-bps" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--auto-price-guard-bps requires an integer"));
                };
                let bps = value.parse::<i128>().map_err(|error| {
                    cli_arg_error(format!(
                        "--auto-price-guard-bps must be an integer: {error}"
                    ))
                })?;
                if !(0..=BASIS_AUTO_PRICE_GUARD_MAX_BPS).contains(&bps) {
                    return Err(cli_arg_error(format!(
                        "--auto-price-guard-bps must be between 0 and {BASIS_AUTO_PRICE_GUARD_MAX_BPS}"
                    )));
                }
                auto_price_guard_bps = Some(bps);
            }
            "--spot-wss-monitor-url" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--spot-wss-monitor-url requires a URL"));
                };
                spot_wss_monitor_url = value.clone();
            }
            "--perp-wss-monitor-url" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--perp-wss-monitor-url requires a URL"));
                };
                perp_wss_monitor_url = value.clone();
            }
            "--private-order-events-dir" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error(
                        "--private-order-events-dir requires a directory",
                    ));
                };
                private_order_events_dir = Some(PathBuf::from(value));
            }
            "--adl-events-dir" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--adl-events-dir requires a directory"));
                };
                adl_events_dir = Some(PathBuf::from(value));
            }
            "--position-state" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--position-state requires a file path"));
                };
                position_state_path = Some(PathBuf::from(value));
            }
            "--interval-secs" | "--poll-interval-secs" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--interval-secs requires an integer"));
                };
                poll_interval_secs = value.parse::<u64>().map_err(|error| {
                    cli_arg_error(format!("--interval-secs must be an integer: {error}"))
                })?;
            }
            "--max-cycles" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--max-cycles requires an integer"));
                };
                let parsed = value.parse::<u64>().map_err(|error| {
                    cli_arg_error(format!("--max-cycles must be an integer: {error}"))
                })?;
                if parsed == 0 {
                    return Err(cli_arg_error("--max-cycles must be greater than zero"));
                }
                max_cycles = Some(parsed);
            }
            "--max-concurrent-positions" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error(
                        "--max-concurrent-positions requires an integer",
                    ));
                };
                max_concurrent_positions = value.parse::<usize>().map_err(|error| {
                    cli_arg_error(format!(
                        "--max-concurrent-positions must be an integer: {error}"
                    ))
                })?;
            }
            "--max-total-notional-usdt" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error(
                        "--max-total-notional-usdt requires an amount",
                    ));
                };
                Decimal::from_str(value)?;
                max_total_notional_usdt = value.clone();
            }
            "--execute-live" => {
                execute_live = true;
            }
            "--dry-run" => {
                execute_live = false;
            }
            "--i-understand-basis-live-orders" => {
                acknowledge_basis_live_orders = true;
            }
            value if value.starts_with('-') => {
                return Err(cli_arg_error(format!(
                    "unknown binance-basis-resident-live option `{value}`"
                )));
            }
            value => {
                return Err(cli_arg_error(format!(
                    "unexpected binance-basis-resident-live positional argument `{value}`"
                )));
            }
        }
        index += 1;
    }

    if poll_interval_secs == 0 {
        return Err(cli_arg_error("--interval-secs must be greater than zero"));
    }
    if max_concurrent_positions == 0 {
        return Err(cli_arg_error(
            "--max-concurrent-positions must be greater than zero",
        ));
    }
    let max_total = Decimal::from_str(&max_total_notional_usdt)?;
    if max_total.is_negative() || max_total.is_zero() {
        return Err(cli_arg_error(
            "--max-total-notional-usdt must be greater than zero",
        ));
    }

    Ok(BinanceBasisResidentLiveOptions {
        symbol,
        config_path,
        output_dir,
        min_net_bps,
        auto_price_guard_bps,
        spot_wss_monitor_url,
        perp_wss_monitor_url,
        private_order_events_dir,
        adl_events_dir,
        position_state_path,
        poll_interval_secs,
        max_cycles,
        max_concurrent_positions,
        max_total_notional_usdt,
        execute_live,
        acknowledge_basis_live_orders,
    })
}

fn default_multi_venue_basis_live_venues() -> Vec<BasisLiveVenue> {
    vec![
        BasisLiveVenue::Binance,
        BasisLiveVenue::Bybit,
        BasisLiveVenue::Okx,
        BasisLiveVenue::Bitget,
    ]
}

fn parse_basis_live_venue(value: &str) -> RuntimeResult<BasisLiveVenue> {
    match value.trim().to_ascii_lowercase().as_str() {
        "binance" => Ok(BasisLiveVenue::Binance),
        "bybit" => Ok(BasisLiveVenue::Bybit),
        "okx" => Ok(BasisLiveVenue::Okx),
        "bitget" => Ok(BasisLiveVenue::Bitget),
        other => Err(cli_arg_error(format!(
            "unsupported basis live venue `{other}`; expected binance,bybit,okx,bitget"
        ))),
    }
}

fn parse_basis_live_venues(value: &str) -> RuntimeResult<Vec<BasisLiveVenue>> {
    let mut venues = Vec::new();
    for part in value.split(',') {
        if part.trim().is_empty() {
            continue;
        }
        let venue = parse_basis_live_venue(part)?;
        if !venues.contains(&venue) {
            venues.push(venue);
        }
    }
    if venues.is_empty() {
        return Err(cli_arg_error("--venues requires at least one venue"));
    }
    Ok(venues)
}

pub(crate) fn parse_multi_venue_basis_resident_live_args(
    args: &[String],
) -> RuntimeResult<MultiVenueBasisResidentLiveCliOptions> {
    let mut venues = default_multi_venue_basis_live_venues();
    let mut symbols = BTreeMap::new();
    let mut opportunity_urls = BTreeMap::new();
    let mut config_path = PathBuf::from("templates/personal_guarded_live.preflight.yaml");
    let mut output_dir = None;
    let mut min_net_bps = BASIS_MONITOR_DEFAULT_MIN_NET_BPS;
    let mut auto_price_guard_bps = Some(2_i128);
    let mut spot_wss_monitor_urls = BTreeMap::new();
    let mut perp_wss_monitor_urls = BTreeMap::new();
    let mut private_order_events_dir = None;
    let mut adl_events_dir = None;
    let mut poll_interval_secs = 60_u64;
    let mut max_cycles = None;
    let mut max_concurrent_positions = 1_usize;
    let mut max_total_notional_usdt = BINANCE_GUARDED_LIVE_NOTIONAL_USDT.to_owned();
    let mut execute_live = false;
    let mut acknowledge_basis_live_orders = false;
    let mut index = 0;

    while index < args.len() {
        match args[index].as_str() {
            "--venues" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error(
                        "--venues requires a comma-separated venue list",
                    ));
                };
                venues = parse_basis_live_venues(value)?;
            }
            "--binance-symbol" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--binance-symbol requires a value"));
                };
                symbols.insert(BasisLiveVenue::Binance, value.clone());
            }
            "--bybit-symbol" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--bybit-symbol requires a value"));
                };
                symbols.insert(BasisLiveVenue::Bybit, value.clone());
            }
            "--okx-symbol" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--okx-symbol requires a value"));
                };
                symbols.insert(BasisLiveVenue::Okx, value.clone());
            }
            "--bitget-symbol" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--bitget-symbol requires a value"));
                };
                symbols.insert(BasisLiveVenue::Bitget, value.clone());
            }
            "--binance-opportunities-url" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--binance-opportunities-url requires a URL"));
                };
                opportunity_urls.insert(BasisLiveVenue::Binance, value.clone());
            }
            "--bybit-opportunities-url" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--bybit-opportunities-url requires a URL"));
                };
                opportunity_urls.insert(BasisLiveVenue::Bybit, value.clone());
            }
            "--okx-opportunities-url" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--okx-opportunities-url requires a URL"));
                };
                opportunity_urls.insert(BasisLiveVenue::Okx, value.clone());
            }
            "--bitget-opportunities-url" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--bitget-opportunities-url requires a URL"));
                };
                opportunity_urls.insert(BasisLiveVenue::Bitget, value.clone());
            }
            "--config" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--config requires a config path"));
                };
                config_path = PathBuf::from(value);
            }
            "--out" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--out requires a directory"));
                };
                output_dir = Some(PathBuf::from(value));
            }
            "--min-net-bps" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--min-net-bps requires an integer"));
                };
                min_net_bps = value.parse::<i128>().map_err(|error| {
                    cli_arg_error(format!("--min-net-bps must be an integer: {error}"))
                })?;
            }
            "--auto-price-guard-bps" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--auto-price-guard-bps requires an integer"));
                };
                let bps = value.parse::<i128>().map_err(|error| {
                    cli_arg_error(format!(
                        "--auto-price-guard-bps must be an integer: {error}"
                    ))
                })?;
                if !(0..=BASIS_AUTO_PRICE_GUARD_MAX_BPS).contains(&bps) {
                    return Err(cli_arg_error(format!(
                        "--auto-price-guard-bps must be between 0 and {BASIS_AUTO_PRICE_GUARD_MAX_BPS}"
                    )));
                }
                auto_price_guard_bps = Some(bps);
            }
            "--binance-spot-wss-monitor-url" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error(
                        "--binance-spot-wss-monitor-url requires a URL",
                    ));
                };
                spot_wss_monitor_urls.insert(BasisLiveVenue::Binance, value.clone());
            }
            "--binance-perp-wss-monitor-url" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error(
                        "--binance-perp-wss-monitor-url requires a URL",
                    ));
                };
                perp_wss_monitor_urls.insert(BasisLiveVenue::Binance, value.clone());
            }
            "--bybit-spot-wss-monitor-url" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--bybit-spot-wss-monitor-url requires a URL"));
                };
                spot_wss_monitor_urls.insert(BasisLiveVenue::Bybit, value.clone());
            }
            "--bybit-perp-wss-monitor-url" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--bybit-perp-wss-monitor-url requires a URL"));
                };
                perp_wss_monitor_urls.insert(BasisLiveVenue::Bybit, value.clone());
            }
            "--okx-spot-wss-monitor-url" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--okx-spot-wss-monitor-url requires a URL"));
                };
                spot_wss_monitor_urls.insert(BasisLiveVenue::Okx, value.clone());
            }
            "--okx-perp-wss-monitor-url" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--okx-perp-wss-monitor-url requires a URL"));
                };
                perp_wss_monitor_urls.insert(BasisLiveVenue::Okx, value.clone());
            }
            "--bitget-spot-wss-monitor-url" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error(
                        "--bitget-spot-wss-monitor-url requires a URL",
                    ));
                };
                spot_wss_monitor_urls.insert(BasisLiveVenue::Bitget, value.clone());
            }
            "--bitget-perp-wss-monitor-url" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error(
                        "--bitget-perp-wss-monitor-url requires a URL",
                    ));
                };
                perp_wss_monitor_urls.insert(BasisLiveVenue::Bitget, value.clone());
            }
            "--private-order-events-dir" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error(
                        "--private-order-events-dir requires a directory",
                    ));
                };
                private_order_events_dir = Some(PathBuf::from(value));
            }
            "--adl-events-dir" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--adl-events-dir requires a directory"));
                };
                adl_events_dir = Some(PathBuf::from(value));
            }
            "--interval-secs" | "--poll-interval-secs" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--interval-secs requires an integer"));
                };
                poll_interval_secs = value.parse::<u64>().map_err(|error| {
                    cli_arg_error(format!("--interval-secs must be an integer: {error}"))
                })?;
            }
            "--max-cycles" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--max-cycles requires an integer"));
                };
                let parsed = value.parse::<u64>().map_err(|error| {
                    cli_arg_error(format!("--max-cycles must be an integer: {error}"))
                })?;
                if parsed == 0 {
                    return Err(cli_arg_error("--max-cycles must be greater than zero"));
                }
                max_cycles = Some(parsed);
            }
            "--max-concurrent-positions" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error(
                        "--max-concurrent-positions requires an integer",
                    ));
                };
                max_concurrent_positions = value.parse::<usize>().map_err(|error| {
                    cli_arg_error(format!(
                        "--max-concurrent-positions must be an integer: {error}"
                    ))
                })?;
            }
            "--max-total-notional-usdt" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error(
                        "--max-total-notional-usdt requires an amount",
                    ));
                };
                Decimal::from_str(value)?;
                max_total_notional_usdt = value.clone();
            }
            "--execute-live" => {
                execute_live = true;
            }
            "--dry-run" => {
                execute_live = false;
            }
            "--i-understand-basis-live-orders" => {
                acknowledge_basis_live_orders = true;
            }
            value if value.starts_with('-') => {
                return Err(cli_arg_error(format!(
                    "unknown multi-venue-basis-resident-live option `{value}`"
                )));
            }
            value => {
                return Err(cli_arg_error(format!(
                    "unexpected multi-venue-basis-resident-live positional argument `{value}`"
                )));
            }
        }
        index += 1;
    }

    if poll_interval_secs == 0 {
        return Err(cli_arg_error("--interval-secs must be greater than zero"));
    }
    if max_concurrent_positions == 0 {
        return Err(cli_arg_error(
            "--max-concurrent-positions must be greater than zero",
        ));
    }
    let max_total = Decimal::from_str(&max_total_notional_usdt)?;
    if max_total.is_negative() || max_total.is_zero() {
        return Err(cli_arg_error(
            "--max-total-notional-usdt must be greater than zero",
        ));
    }

    Ok(MultiVenueBasisResidentLiveOptions {
        venues,
        symbols,
        opportunity_urls,
        config_path,
        output_dir,
        min_net_bps,
        auto_price_guard_bps,
        spot_wss_monitor_urls,
        perp_wss_monitor_urls,
        private_order_events_dir,
        adl_events_dir,
        poll_interval_secs,
        max_cycles,
        max_concurrent_positions,
        max_total_notional_usdt,
        execute_live,
        acknowledge_basis_live_orders,
    })
}

pub(crate) fn parse_multi_venue_basis_live_stack_args(
    args: &[String],
) -> RuntimeResult<MultiVenueBasisLiveStackCliOptions> {
    let mut venues = default_multi_venue_basis_live_venues();
    let mut symbols = BTreeMap::new();
    let mut opportunity_urls = BTreeMap::new();
    let mut config_path = PathBuf::from("templates/personal_guarded_live.preflight.yaml");
    let mut output_dir = None;
    let mut min_net_bps = BASIS_MONITOR_DEFAULT_MIN_NET_BPS;
    let mut auto_price_guard_bps = Some(2_i128);
    let mut spot_wss_bind_addrs = BTreeMap::new();
    let mut perp_wss_bind_addrs = BTreeMap::new();
    let mut monitor_symbol = None;
    let mut monitor_reconnect_delay_secs = BINANCE_WSS_BOOK_TICKER_DEFAULT_RECONNECT_DELAY_SECS;
    let mut readiness_timeout_secs = 60_u64;
    let mut shutdown_grace_secs = 5_u64;
    let mut private_order_events_dir = None;
    let mut adl_events_dir = None;
    let mut poll_interval_secs = 60_u64;
    let mut max_cycles = None;
    let mut max_concurrent_positions = 1_usize;
    let mut max_total_notional_usdt = BINANCE_GUARDED_LIVE_NOTIONAL_USDT.to_owned();
    let mut use_existing_monitors = false;
    let mut execute_live = false;
    let mut acknowledge_basis_live_orders = false;
    let mut index = 0;

    while index < args.len() {
        match args[index].as_str() {
            "--venues" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error(
                        "--venues requires a comma-separated venue list",
                    ));
                };
                venues = parse_basis_live_venues(value)?;
            }
            "--binance-symbol" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--binance-symbol requires a value"));
                };
                symbols.insert(BasisLiveVenue::Binance, value.clone());
            }
            "--bybit-symbol" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--bybit-symbol requires a value"));
                };
                symbols.insert(BasisLiveVenue::Bybit, value.clone());
            }
            "--okx-symbol" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--okx-symbol requires a value"));
                };
                symbols.insert(BasisLiveVenue::Okx, value.clone());
            }
            "--bitget-symbol" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--bitget-symbol requires a value"));
                };
                symbols.insert(BasisLiveVenue::Bitget, value.clone());
            }
            "--binance-opportunities-url" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--binance-opportunities-url requires a URL"));
                };
                opportunity_urls.insert(BasisLiveVenue::Binance, value.clone());
            }
            "--bybit-opportunities-url" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--bybit-opportunities-url requires a URL"));
                };
                opportunity_urls.insert(BasisLiveVenue::Bybit, value.clone());
            }
            "--okx-opportunities-url" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--okx-opportunities-url requires a URL"));
                };
                opportunity_urls.insert(BasisLiveVenue::Okx, value.clone());
            }
            "--bitget-opportunities-url" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--bitget-opportunities-url requires a URL"));
                };
                opportunity_urls.insert(BasisLiveVenue::Bitget, value.clone());
            }
            "--config" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--config requires a config path"));
                };
                config_path = PathBuf::from(value);
            }
            "--out" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--out requires a directory"));
                };
                output_dir = Some(PathBuf::from(value));
            }
            "--min-net-bps" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--min-net-bps requires an integer"));
                };
                min_net_bps = value.parse::<i128>().map_err(|error| {
                    cli_arg_error(format!("--min-net-bps must be an integer: {error}"))
                })?;
            }
            "--auto-price-guard-bps" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--auto-price-guard-bps requires an integer"));
                };
                let bps = value.parse::<i128>().map_err(|error| {
                    cli_arg_error(format!(
                        "--auto-price-guard-bps must be an integer: {error}"
                    ))
                })?;
                if !(0..=BASIS_AUTO_PRICE_GUARD_MAX_BPS).contains(&bps) {
                    return Err(cli_arg_error(format!(
                        "--auto-price-guard-bps must be between 0 and {BASIS_AUTO_PRICE_GUARD_MAX_BPS}"
                    )));
                }
                auto_price_guard_bps = Some(bps);
            }
            "--monitor-symbol" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--monitor-symbol requires a value"));
                };
                monitor_symbol = Some(value.clone());
            }
            "--monitor-reconnect-delay-secs" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error(
                        "--monitor-reconnect-delay-secs requires an integer",
                    ));
                };
                monitor_reconnect_delay_secs = value.parse::<u64>().map_err(|error| {
                    cli_arg_error(format!(
                        "--monitor-reconnect-delay-secs must be an integer: {error}"
                    ))
                })?;
            }
            "--readiness-timeout-secs" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error(
                        "--readiness-timeout-secs requires an integer",
                    ));
                };
                readiness_timeout_secs = value.parse::<u64>().map_err(|error| {
                    cli_arg_error(format!(
                        "--readiness-timeout-secs must be an integer: {error}"
                    ))
                })?;
            }
            "--shutdown-grace-secs" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--shutdown-grace-secs requires an integer"));
                };
                shutdown_grace_secs = value.parse::<u64>().map_err(|error| {
                    cli_arg_error(format!("--shutdown-grace-secs must be an integer: {error}"))
                })?;
            }
            "--binance-spot-wss-bind" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error(
                        "--binance-spot-wss-bind requires host:port or URL",
                    ));
                };
                spot_wss_bind_addrs.insert(BasisLiveVenue::Binance, value.clone());
            }
            "--binance-perp-wss-bind" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error(
                        "--binance-perp-wss-bind requires host:port or URL",
                    ));
                };
                perp_wss_bind_addrs.insert(BasisLiveVenue::Binance, value.clone());
            }
            "--bybit-spot-wss-bind" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error(
                        "--bybit-spot-wss-bind requires host:port or URL",
                    ));
                };
                spot_wss_bind_addrs.insert(BasisLiveVenue::Bybit, value.clone());
            }
            "--bybit-perp-wss-bind" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error(
                        "--bybit-perp-wss-bind requires host:port or URL",
                    ));
                };
                perp_wss_bind_addrs.insert(BasisLiveVenue::Bybit, value.clone());
            }
            "--okx-spot-wss-bind" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error(
                        "--okx-spot-wss-bind requires host:port or URL",
                    ));
                };
                spot_wss_bind_addrs.insert(BasisLiveVenue::Okx, value.clone());
            }
            "--okx-perp-wss-bind" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error(
                        "--okx-perp-wss-bind requires host:port or URL",
                    ));
                };
                perp_wss_bind_addrs.insert(BasisLiveVenue::Okx, value.clone());
            }
            "--bitget-spot-wss-bind" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error(
                        "--bitget-spot-wss-bind requires host:port or URL",
                    ));
                };
                spot_wss_bind_addrs.insert(BasisLiveVenue::Bitget, value.clone());
            }
            "--bitget-perp-wss-bind" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error(
                        "--bitget-perp-wss-bind requires host:port or URL",
                    ));
                };
                perp_wss_bind_addrs.insert(BasisLiveVenue::Bitget, value.clone());
            }
            "--private-order-events-dir" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error(
                        "--private-order-events-dir requires a directory",
                    ));
                };
                private_order_events_dir = Some(PathBuf::from(value));
            }
            "--adl-events-dir" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--adl-events-dir requires a directory"));
                };
                adl_events_dir = Some(PathBuf::from(value));
            }
            "--interval-secs" | "--poll-interval-secs" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--interval-secs requires an integer"));
                };
                poll_interval_secs = value.parse::<u64>().map_err(|error| {
                    cli_arg_error(format!("--interval-secs must be an integer: {error}"))
                })?;
            }
            "--max-cycles" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--max-cycles requires an integer"));
                };
                let parsed = value.parse::<u64>().map_err(|error| {
                    cli_arg_error(format!("--max-cycles must be an integer: {error}"))
                })?;
                if parsed == 0 {
                    return Err(cli_arg_error("--max-cycles must be greater than zero"));
                }
                max_cycles = Some(parsed);
            }
            "--max-concurrent-positions" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error(
                        "--max-concurrent-positions requires an integer",
                    ));
                };
                max_concurrent_positions = value.parse::<usize>().map_err(|error| {
                    cli_arg_error(format!(
                        "--max-concurrent-positions must be an integer: {error}"
                    ))
                })?;
            }
            "--max-total-notional-usdt" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error(
                        "--max-total-notional-usdt requires an amount",
                    ));
                };
                Decimal::from_str(value)?;
                max_total_notional_usdt = value.clone();
            }
            "--use-existing-monitors" => {
                use_existing_monitors = true;
            }
            "--execute-live" => {
                execute_live = true;
            }
            "--dry-run" => {
                execute_live = false;
            }
            "--i-understand-basis-live-orders" => {
                acknowledge_basis_live_orders = true;
            }
            value if value.starts_with('-') => {
                return Err(cli_arg_error(format!(
                    "unknown multi-venue-basis-live-stack option `{value}`"
                )));
            }
            value => {
                return Err(cli_arg_error(format!(
                    "unexpected multi-venue-basis-live-stack positional argument `{value}`"
                )));
            }
        }
        index += 1;
    }

    if poll_interval_secs == 0 {
        return Err(cli_arg_error("--interval-secs must be greater than zero"));
    }
    if readiness_timeout_secs == 0 {
        return Err(cli_arg_error(
            "--readiness-timeout-secs must be greater than zero",
        ));
    }
    if shutdown_grace_secs == 0 {
        return Err(cli_arg_error(
            "--shutdown-grace-secs must be greater than zero",
        ));
    }
    if monitor_reconnect_delay_secs == 0 {
        return Err(cli_arg_error(
            "--monitor-reconnect-delay-secs must be greater than zero",
        ));
    }
    if max_concurrent_positions == 0 {
        return Err(cli_arg_error(
            "--max-concurrent-positions must be greater than zero",
        ));
    }
    let max_total = Decimal::from_str(&max_total_notional_usdt)?;
    if max_total.is_negative() || max_total.is_zero() {
        return Err(cli_arg_error(
            "--max-total-notional-usdt must be greater than zero",
        ));
    }

    Ok(MultiVenueBasisLiveStackOptions {
        venues,
        symbols,
        opportunity_urls,
        config_path,
        output_dir,
        min_net_bps,
        auto_price_guard_bps,
        spot_wss_bind_addrs,
        perp_wss_bind_addrs,
        monitor_symbol,
        monitor_reconnect_delay_secs,
        readiness_timeout_secs,
        shutdown_grace_secs,
        private_order_events_dir,
        adl_events_dir,
        poll_interval_secs,
        max_cycles,
        max_concurrent_positions,
        max_total_notional_usdt,
        use_existing_monitors,
        execute_live,
        acknowledge_basis_live_orders,
    })
}

pub(crate) fn parse_basis_exit_supervisor_args(
    args: &[String],
) -> RuntimeResult<BasisExitSupervisorCliOptions> {
    let mut position_state_path = None;
    let mut config_path = PathBuf::from("templates/personal_guarded_live.preflight.yaml");
    let mut output_dir = None;
    let mut spot_wss_monitor_url = None;
    let mut perp_wss_monitor_url = None;
    let mut adl_events_dir = None;
    let mut execute_live = false;
    let mut acknowledge_basis_live_orders = false;
    let mut once = false;
    let mut poll_interval_secs = BASIS_MONITOR_DEFAULT_POLL_INTERVAL_SECS;
    let mut index = 0;

    while index < args.len() {
        match args[index].as_str() {
            "--position-state" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--position-state requires a file path"));
                };
                position_state_path = Some(PathBuf::from(value));
            }
            "--config" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--config requires a config path"));
                };
                config_path = PathBuf::from(value);
            }
            "--out" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--out requires a directory"));
                };
                output_dir = Some(PathBuf::from(value));
            }
            "--spot-wss-monitor-url" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--spot-wss-monitor-url requires a URL"));
                };
                spot_wss_monitor_url = Some(value.clone());
            }
            "--perp-wss-monitor-url" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--perp-wss-monitor-url requires a URL"));
                };
                perp_wss_monitor_url = Some(value.clone());
            }
            "--adl-events-dir" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--adl-events-dir requires a directory"));
                };
                adl_events_dir = Some(PathBuf::from(value));
            }
            "--interval-secs" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--interval-secs requires an integer"));
                };
                poll_interval_secs = value.parse::<u64>().map_err(|error| {
                    cli_arg_error(format!("--interval-secs must be an integer: {error}"))
                })?;
                if poll_interval_secs == 0 {
                    return Err(cli_arg_error("--interval-secs must be greater than zero"));
                }
            }
            "--once" => {
                once = true;
            }
            "--execute-live" => {
                execute_live = true;
            }
            "--dry-run" => {
                execute_live = false;
            }
            "--i-understand-basis-live-orders" => {
                acknowledge_basis_live_orders = true;
            }
            value if value.starts_with('-') => {
                return Err(cli_arg_error(format!(
                    "unknown basis-exit-supervisor option `{value}`"
                )));
            }
            value => {
                return Err(cli_arg_error(format!(
                    "unexpected basis-exit-supervisor positional argument `{value}`"
                )));
            }
        }
        index += 1;
    }

    Ok(BasisExitSupervisorOptions {
        position_state_path: position_state_path.ok_or_else(|| {
            cli_arg_error("--position-state is required for basis-exit-supervisor")
        })?,
        config_path,
        output_dir,
        spot_wss_monitor_url,
        perp_wss_monitor_url,
        adl_events_dir,
        execute_live,
        acknowledge_basis_live_orders,
        once,
        poll_interval_secs,
    })
}

pub(crate) fn parse_live_wss_symbol_resolver_args(
    args: &[String],
) -> RuntimeResult<LiveWssSymbolResolverOptions> {
    let mut options = LiveWssSymbolResolverOptions::default();
    let mut index = 0;

    while index < args.len() {
        match args[index].as_str() {
            "--strategies" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--strategies requires a value"));
                };
                options.strategies = value.clone();
            }
            "--monitors" | "--venues" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--monitors requires a value"));
                };
                options.monitors = value.clone();
            }
            "--format" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--format requires shell or json"));
                };
                options.output_format = match value.trim().to_ascii_lowercase().as_str() {
                    "shell" | "env" => LiveWssSymbolResolverOutputFormat::Shell,
                    "json" => LiveWssSymbolResolverOutputFormat::Json,
                    _ => return Err(cli_arg_error("--format must be shell or json")),
                };
            }
            value if value.starts_with('-') => {
                return Err(cli_arg_error(format!(
                    "unknown resolve-live-wss-symbols option `{value}`"
                )));
            }
            value => {
                return Err(cli_arg_error(format!(
                    "unexpected resolve-live-wss-symbols positional argument `{value}`"
                )));
            }
        }
        index += 1;
    }

    Ok(options)
}

pub(crate) fn parse_binance_wss_book_ticker_args(
    args: &[String],
) -> RuntimeResult<BinanceWssBookTickerCliOptions> {
    let mut options = BinanceWssBookTickerProbeOptions::default();
    let mut index = 0;

    while index < args.len() {
        match args[index].as_str() {
            "--bind" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--bind requires host:port"));
                };
                options.bind_addr = value.clone();
            }
            "--symbol" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--symbol requires a value"));
                };
                options.symbol = value.clone();
            }
            "--market" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--market requires spot or usdm-perp"));
                };
                options.market = parse_binance_public_wss_market(value)?;
            }
            "--updates" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--updates requires a value"));
                };
                options.updates = value
                    .parse::<usize>()
                    .map_err(|_| cli_arg_error("--updates must be an integer"))?;
            }
            "--reconnect-delay-secs" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--reconnect-delay-secs requires a value"));
                };
                options.reconnect_delay_secs = value
                    .parse::<u64>()
                    .map_err(|_| cli_arg_error("--reconnect-delay-secs must be an integer"))?;
            }
            "--once" => {
                options.once = true;
            }
            value if value.starts_with('-') => {
                return Err(cli_arg_error(format!(
                    "unknown binance-wss-book-ticker option `{value}`"
                )));
            }
            value => {
                return Err(cli_arg_error(format!(
                    "unexpected binance-wss-book-ticker positional argument `{value}`"
                )));
            }
        }
        index += 1;
    }

    validate_binance_wss_probe_options(&options)?;
    Ok(options)
}

pub(crate) fn parse_bybit_wss_book_ticker_args(
    args: &[String],
) -> RuntimeResult<BybitWssBookTickerCliOptions> {
    let mut options = BybitWssBookTickerProbeOptions::default();
    let mut index = 0;

    while index < args.len() {
        match args[index].as_str() {
            "--bind" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--bind requires host:port"));
                };
                options.bind_addr = value.clone();
            }
            "--symbol" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--symbol requires a value"));
                };
                options.symbol = value.clone();
            }
            "--market" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--market requires spot or linear-perp"));
                };
                options.market = parse_bybit_public_wss_market(value)?;
            }
            "--updates" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--updates requires a value"));
                };
                options.updates = value
                    .parse::<usize>()
                    .map_err(|_| cli_arg_error("--updates must be an integer"))?;
            }
            "--reconnect-delay-secs" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--reconnect-delay-secs requires a value"));
                };
                options.reconnect_delay_secs = value
                    .parse::<u64>()
                    .map_err(|_| cli_arg_error("--reconnect-delay-secs must be an integer"))?;
            }
            "--once" => {
                options.once = true;
            }
            value if value.starts_with('-') => {
                return Err(cli_arg_error(format!(
                    "unknown bybit-wss-book-ticker option `{value}`"
                )));
            }
            value => {
                return Err(cli_arg_error(format!(
                    "unexpected bybit-wss-book-ticker positional argument `{value}`"
                )));
            }
        }
        index += 1;
    }

    validate_bybit_wss_probe_options(&options)?;
    Ok(options)
}

pub(crate) fn parse_okx_wss_book_ticker_args(
    args: &[String],
) -> RuntimeResult<OkxWssBookTickerCliOptions> {
    let mut options = OkxWssBookTickerMonitorOptions::default();
    let mut index = 0;

    while index < args.len() {
        match args[index].as_str() {
            "--bind" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--bind requires host:port"));
                };
                options.bind_addr = value.clone();
            }
            "--symbol" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--symbol requires a value"));
                };
                options.symbol = value.clone();
            }
            "--market" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--market requires spot or swap"));
                };
                options.market = parse_okx_public_wss_market(value)?;
            }
            "--updates" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--updates requires a value"));
                };
                options.updates = value
                    .parse::<usize>()
                    .map_err(|_| cli_arg_error("--updates must be an integer"))?;
            }
            "--reconnect-delay-secs" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--reconnect-delay-secs requires a value"));
                };
                options.reconnect_delay_secs = value
                    .parse::<u64>()
                    .map_err(|_| cli_arg_error("--reconnect-delay-secs must be an integer"))?;
            }
            "--once" => {
                options.once = true;
            }
            value if value.starts_with('-') => {
                return Err(cli_arg_error(format!(
                    "unknown okx-wss-book-ticker option `{value}`"
                )));
            }
            value => {
                return Err(cli_arg_error(format!(
                    "unexpected okx-wss-book-ticker positional argument `{value}`"
                )));
            }
        }
        index += 1;
    }

    validate_okx_wss_probe_options(&options)?;
    Ok(options)
}

pub(crate) fn parse_bitget_wss_book_ticker_args(
    args: &[String],
) -> RuntimeResult<BitgetWssBookTickerCliOptions> {
    let mut options = BitgetWssBookTickerMonitorOptions::default();
    let mut index = 0;

    while index < args.len() {
        match args[index].as_str() {
            "--bind" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--bind requires host:port"));
                };
                options.bind_addr = value.clone();
            }
            "--symbol" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--symbol requires a value"));
                };
                options.symbol = value.clone();
            }
            "--market" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--market requires spot or usdt-futures"));
                };
                options.market = parse_bitget_public_wss_market(value)?;
            }
            "--updates" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--updates requires a value"));
                };
                options.updates = value
                    .parse::<usize>()
                    .map_err(|_| cli_arg_error("--updates must be an integer"))?;
            }
            "--reconnect-delay-secs" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--reconnect-delay-secs requires a value"));
                };
                options.reconnect_delay_secs = value
                    .parse::<u64>()
                    .map_err(|_| cli_arg_error("--reconnect-delay-secs must be an integer"))?;
            }
            "--once" => {
                options.once = true;
            }
            value if value.starts_with('-') => {
                return Err(cli_arg_error(format!(
                    "unknown bitget-wss-book-ticker option `{value}`"
                )));
            }
            value => {
                return Err(cli_arg_error(format!(
                    "unexpected bitget-wss-book-ticker positional argument `{value}`"
                )));
            }
        }
        index += 1;
    }

    validate_bitget_wss_probe_options(&options)?;
    Ok(options)
}

pub(crate) fn parse_aster_wss_book_ticker_args(
    args: &[String],
) -> RuntimeResult<AsterWssBookTickerCliOptions> {
    let mut options = AsterWssBookTickerMonitorOptions::default();
    let mut index = 0;

    while index < args.len() {
        match args[index].as_str() {
            "--bind" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--bind requires host:port"));
                };
                options.bind_addr = value.clone();
            }
            "--symbol" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--symbol requires a value"));
                };
                options.symbol = value.clone();
            }
            "--market" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--market requires usdt-futures"));
                };
                options.market = parse_aster_public_wss_market(value)?;
            }
            "--updates" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--updates requires a value"));
                };
                options.updates = value
                    .parse::<usize>()
                    .map_err(|_| cli_arg_error("--updates must be an integer"))?;
            }
            "--reconnect-delay-secs" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--reconnect-delay-secs requires a value"));
                };
                options.reconnect_delay_secs = value
                    .parse::<u64>()
                    .map_err(|_| cli_arg_error("--reconnect-delay-secs must be an integer"))?;
            }
            "--once" => {
                options.once = true;
            }
            value if value.starts_with('-') => {
                return Err(cli_arg_error(format!(
                    "unknown aster-wss-book-ticker option `{value}`"
                )));
            }
            value => {
                return Err(cli_arg_error(format!(
                    "unexpected aster-wss-book-ticker positional argument `{value}`"
                )));
            }
        }
        index += 1;
    }

    validate_aster_wss_probe_options(&options)?;
    Ok(options)
}

pub(crate) fn parse_hyperliquid_wss_book_ticker_args(
    args: &[String],
) -> RuntimeResult<HyperliquidWssBookTickerCliOptions> {
    let mut options = HyperliquidWssBookTickerMonitorOptions::default();
    let mut index = 0;

    while index < args.len() {
        match args[index].as_str() {
            "--bind" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--bind requires host:port"));
                };
                options.bind_addr = value.clone();
            }
            "--symbol" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--symbol requires a value"));
                };
                options.symbol = value.clone();
            }
            "--market" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--market requires perp"));
                };
                options.market = parse_hyperliquid_public_wss_market(value)?;
            }
            "--updates" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--updates requires a value"));
                };
                options.updates = value
                    .parse::<usize>()
                    .map_err(|_| cli_arg_error("--updates must be an integer"))?;
            }
            "--reconnect-delay-secs" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--reconnect-delay-secs requires a value"));
                };
                options.reconnect_delay_secs = value
                    .parse::<u64>()
                    .map_err(|_| cli_arg_error("--reconnect-delay-secs must be an integer"))?;
            }
            "--once" => {
                options.once = true;
            }
            value if value.starts_with('-') => {
                return Err(cli_arg_error(format!(
                    "unknown hyperliquid-wss-book-ticker option `{value}`"
                )));
            }
            value => {
                return Err(cli_arg_error(format!(
                    "unexpected hyperliquid-wss-book-ticker positional argument `{value}`"
                )));
            }
        }
        index += 1;
    }

    validate_hyperliquid_wss_probe_options(&options)?;
    Ok(options)
}

pub(crate) fn parse_binance_basis_monitor_args(
    args: &[String],
) -> RuntimeResult<BinanceBasisMonitorCliOptions> {
    let mut options = BinanceBasisMonitorOptions::default();
    let mut index = 0;

    while index < args.len() {
        match args[index].as_str() {
            "--bind" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--bind requires an address"));
                };
                options.bind_addr = value.clone();
            }
            "--interval-secs" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--interval-secs requires a value"));
                };
                options.poll_interval_secs = value
                    .parse::<u64>()
                    .map_err(|_| cli_arg_error("--interval-secs must be an integer"))?;
            }
            "--min-abs-funding-rate" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--min-abs-funding-rate requires a decimal"));
                };
                options.min_abs_funding_rate = value.clone();
            }
            "--min-net-bps" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--min-net-bps requires a value"));
                };
                options.min_net_bps = value
                    .parse::<i128>()
                    .map_err(|_| cli_arg_error("--min-net-bps must be an integer"))?;
            }
            "--notional-usd" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--notional-usd requires a decimal"));
                };
                options.notional_usd = value.clone();
            }
            "--spot-fee-bps" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--spot-fee-bps requires a value"));
                };
                options.spot_taker_fee_bps = value.clone();
            }
            "--perp-fee-bps" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--perp-fee-bps requires a value"));
                };
                options.perp_taker_fee_bps = value.clone();
            }
            "--slippage-bps" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--slippage-bps requires a value"));
                };
                options.slippage_buffer_bps = value
                    .parse::<i128>()
                    .map_err(|_| cli_arg_error("--slippage-bps must be an integer"))?;
            }
            "--once" => options.once = true,
            "--out" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--out requires a directory"));
                };
                options.output_dir = Some(PathBuf::from(value));
            }
            value if value.starts_with('-') => {
                return Err(cli_arg_error(format!(
                    "unknown binance-basis-monitor option `{value}`"
                )));
            }
            value => {
                return Err(cli_arg_error(format!(
                    "unexpected binance-basis-monitor positional argument `{value}`"
                )));
            }
        }
        index += 1;
    }

    validate_monitor_options(&options)?;
    Ok(options)
}

pub(crate) fn parse_bybit_basis_monitor_args(
    args: &[String],
) -> RuntimeResult<BybitBasisMonitorCliOptions> {
    let mut options = BybitBasisMonitorOptions::default();
    let mut index = 0;

    while index < args.len() {
        match args[index].as_str() {
            "--bind" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--bind requires an address"));
                };
                options.bind_addr = value.clone();
            }
            "--interval-secs" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--interval-secs requires a value"));
                };
                options.poll_interval_secs = value
                    .parse::<u64>()
                    .map_err(|_| cli_arg_error("--interval-secs must be an integer"))?;
            }
            "--min-abs-funding-rate" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--min-abs-funding-rate requires a decimal"));
                };
                options.min_abs_funding_rate = value.clone();
            }
            "--min-net-bps" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--min-net-bps requires a value"));
                };
                options.min_net_bps = value
                    .parse::<i128>()
                    .map_err(|_| cli_arg_error("--min-net-bps must be an integer"))?;
            }
            "--notional-usd" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--notional-usd requires a decimal"));
                };
                options.notional_usd = value.clone();
            }
            "--spot-fee-bps" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--spot-fee-bps requires a value"));
                };
                options.spot_taker_fee_bps = value.clone();
            }
            "--perp-fee-bps" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--perp-fee-bps requires a value"));
                };
                options.perp_taker_fee_bps = value.clone();
            }
            "--slippage-bps" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--slippage-bps requires a value"));
                };
                options.slippage_buffer_bps = value
                    .parse::<i128>()
                    .map_err(|_| cli_arg_error("--slippage-bps must be an integer"))?;
            }
            "--once" => options.once = true,
            "--out" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--out requires a directory"));
                };
                options.output_dir = Some(PathBuf::from(value));
            }
            value if value.starts_with('-') => {
                return Err(cli_arg_error(format!(
                    "unknown bybit-basis-monitor option `{value}`"
                )));
            }
            value => {
                return Err(cli_arg_error(format!(
                    "unexpected bybit-basis-monitor positional argument `{value}`"
                )));
            }
        }
        index += 1;
    }

    validate_bybit_monitor_options(&options)?;
    Ok(options)
}

pub(crate) fn parse_okx_basis_monitor_args(
    args: &[String],
) -> RuntimeResult<OkxBasisMonitorCliOptions> {
    let mut options = OkxBasisMonitorOptions::default();
    let mut index = 0;

    while index < args.len() {
        match args[index].as_str() {
            "--bind" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--bind requires an address"));
                };
                options.bind_addr = value.clone();
            }
            "--interval-secs" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--interval-secs requires a value"));
                };
                options.poll_interval_secs = value
                    .parse::<u64>()
                    .map_err(|_| cli_arg_error("--interval-secs must be an integer"))?;
            }
            "--min-abs-funding-rate" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--min-abs-funding-rate requires a decimal"));
                };
                options.min_abs_funding_rate = value.clone();
            }
            "--min-net-bps" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--min-net-bps requires a value"));
                };
                options.min_net_bps = value
                    .parse::<i128>()
                    .map_err(|_| cli_arg_error("--min-net-bps must be an integer"))?;
            }
            "--notional-usd" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--notional-usd requires a decimal"));
                };
                options.notional_usd = value.clone();
            }
            "--spot-fee-bps" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--spot-fee-bps requires a value"));
                };
                options.spot_taker_fee_bps = value.clone();
            }
            "--perp-fee-bps" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--perp-fee-bps requires a value"));
                };
                options.perp_taker_fee_bps = value.clone();
            }
            "--slippage-bps" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--slippage-bps requires a value"));
                };
                options.slippage_buffer_bps = value
                    .parse::<i128>()
                    .map_err(|_| cli_arg_error("--slippage-bps must be an integer"))?;
            }
            "--once" => options.once = true,
            "--out" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--out requires a directory"));
                };
                options.output_dir = Some(PathBuf::from(value));
            }
            value if value.starts_with('-') => {
                return Err(cli_arg_error(format!(
                    "unknown okx-basis-monitor option `{value}`"
                )));
            }
            value => {
                return Err(cli_arg_error(format!(
                    "unexpected okx-basis-monitor positional argument `{value}`"
                )));
            }
        }
        index += 1;
    }

    validate_okx_monitor_options(&options)?;
    Ok(options)
}

pub(crate) fn parse_bitget_basis_monitor_args(
    args: &[String],
) -> RuntimeResult<BitgetBasisMonitorCliOptions> {
    let mut options = BitgetBasisMonitorOptions::default();
    let mut index = 0;

    while index < args.len() {
        match args[index].as_str() {
            "--bind" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--bind requires an address"));
                };
                options.bind_addr = value.clone();
            }
            "--interval-secs" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--interval-secs requires a value"));
                };
                options.poll_interval_secs = value
                    .parse::<u64>()
                    .map_err(|_| cli_arg_error("--interval-secs must be an integer"))?;
            }
            "--min-abs-funding-rate" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--min-abs-funding-rate requires a decimal"));
                };
                options.min_abs_funding_rate = value.clone();
            }
            "--min-net-bps" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--min-net-bps requires a value"));
                };
                options.min_net_bps = value
                    .parse::<i128>()
                    .map_err(|_| cli_arg_error("--min-net-bps must be an integer"))?;
            }
            "--notional-usd" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--notional-usd requires a decimal"));
                };
                options.notional_usd = value.clone();
            }
            "--spot-fee-bps" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--spot-fee-bps requires a value"));
                };
                options.spot_taker_fee_bps = value.clone();
            }
            "--perp-fee-bps" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--perp-fee-bps requires a value"));
                };
                options.perp_taker_fee_bps = value.clone();
            }
            "--slippage-bps" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--slippage-bps requires a value"));
                };
                options.slippage_buffer_bps = value
                    .parse::<i128>()
                    .map_err(|_| cli_arg_error("--slippage-bps must be an integer"))?;
            }
            "--once" => options.once = true,
            "--out" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--out requires a directory"));
                };
                options.output_dir = Some(PathBuf::from(value));
            }
            value if value.starts_with('-') => {
                return Err(cli_arg_error(format!(
                    "unknown bitget-basis-monitor option `{value}`"
                )));
            }
            value => {
                return Err(cli_arg_error(format!(
                    "unexpected bitget-basis-monitor positional argument `{value}`"
                )));
            }
        }
        index += 1;
    }

    validate_bitget_monitor_options(&options)?;
    Ok(options)
}

pub(crate) fn parse_hyperliquid_basis_monitor_args(
    args: &[String],
) -> RuntimeResult<HyperliquidBasisMonitorCliOptions> {
    let mut options = HyperliquidBasisMonitorOptions::default();
    let mut index = 0;

    while index < args.len() {
        match args[index].as_str() {
            "--bind" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--bind requires an address"));
                };
                options.bind_addr = value.clone();
            }
            "--interval-secs" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--interval-secs requires a value"));
                };
                options.poll_interval_secs = value
                    .parse::<u64>()
                    .map_err(|_| cli_arg_error("--interval-secs must be an integer"))?;
            }
            "--min-abs-funding-rate" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--min-abs-funding-rate requires a decimal"));
                };
                options.min_abs_funding_rate = value.clone();
            }
            "--min-net-bps" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--min-net-bps requires a value"));
                };
                options.min_net_bps = value
                    .parse::<i128>()
                    .map_err(|_| cli_arg_error("--min-net-bps must be an integer"))?;
            }
            "--notional-usd" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--notional-usd requires a decimal"));
                };
                options.notional_usd = value.clone();
            }
            "--spot-fee-bps" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--spot-fee-bps requires a value"));
                };
                options.spot_taker_fee_bps = value.clone();
            }
            "--perp-fee-bps" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--perp-fee-bps requires a value"));
                };
                options.perp_taker_fee_bps = value.clone();
            }
            "--slippage-bps" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--slippage-bps requires a value"));
                };
                options.slippage_buffer_bps = value
                    .parse::<i128>()
                    .map_err(|_| cli_arg_error("--slippage-bps must be an integer"))?;
            }
            "--perp-wss-monitor-url" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--perp-wss-monitor-url requires a URL"));
                };
                options.perp_wss_monitor_url = Some(value.clone());
            }
            "--once" => options.once = true,
            "--out" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--out requires a directory"));
                };
                options.output_dir = Some(PathBuf::from(value));
            }
            value if value.starts_with('-') => {
                return Err(cli_arg_error(format!(
                    "unknown hyperliquid-basis-monitor option `{value}`"
                )));
            }
            value => {
                return Err(cli_arg_error(format!(
                    "unexpected hyperliquid-basis-monitor positional argument `{value}`"
                )));
            }
        }
        index += 1;
    }

    validate_hyperliquid_monitor_options(&options)?;
    Ok(options)
}

pub(crate) fn parse_aster_basis_monitor_args(
    args: &[String],
) -> RuntimeResult<AsterBasisMonitorCliOptions> {
    let mut options = AsterBasisMonitorOptions::default();
    let mut index = 0;

    while index < args.len() {
        match args[index].as_str() {
            "--bind" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--bind requires an address"));
                };
                options.bind_addr = value.clone();
            }
            "--interval-secs" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--interval-secs requires a value"));
                };
                options.poll_interval_secs = value
                    .parse::<u64>()
                    .map_err(|_| cli_arg_error("--interval-secs must be an integer"))?;
            }
            "--min-abs-funding-rate" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--min-abs-funding-rate requires a decimal"));
                };
                options.min_abs_funding_rate = value.clone();
            }
            "--min-net-bps" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--min-net-bps requires a value"));
                };
                options.min_net_bps = value
                    .parse::<i128>()
                    .map_err(|_| cli_arg_error("--min-net-bps must be an integer"))?;
            }
            "--notional-usd" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--notional-usd requires a decimal"));
                };
                options.notional_usd = value.clone();
            }
            "--spot-fee-bps" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--spot-fee-bps requires a value"));
                };
                options.spot_taker_fee_bps = value.clone();
            }
            "--perp-fee-bps" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--perp-fee-bps requires a value"));
                };
                options.perp_taker_fee_bps = value.clone();
            }
            "--slippage-bps" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--slippage-bps requires a value"));
                };
                options.slippage_buffer_bps = value
                    .parse::<i128>()
                    .map_err(|_| cli_arg_error("--slippage-bps must be an integer"))?;
            }
            "--perp-wss-monitor-url" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--perp-wss-monitor-url requires a URL"));
                };
                options.perp_wss_monitor_url = Some(value.clone());
            }
            "--once" => options.once = true,
            "--out" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--out requires a directory"));
                };
                options.output_dir = Some(PathBuf::from(value));
            }
            value if value.starts_with('-') => {
                return Err(cli_arg_error(format!(
                    "unknown aster-basis-monitor option `{value}`"
                )));
            }
            value => {
                return Err(cli_arg_error(format!(
                    "unexpected aster-basis-monitor positional argument `{value}`"
                )));
            }
        }
        index += 1;
    }

    validate_aster_monitor_options(&options)?;
    Ok(options)
}

pub(crate) fn parse_funding_arb_monitor_args(
    args: &[String],
) -> RuntimeResult<FundingArbMonitorCliOptions> {
    let mut options = FundingArbMonitorOptions::default();
    let mut index = 0;

    while index < args.len() {
        match args[index].as_str() {
            "--bind" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--bind requires an address"));
                };
                options.bind_addr = value.clone();
            }
            "--interval-secs" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--interval-secs requires a value"));
                };
                options.poll_interval_secs = value
                    .parse::<u64>()
                    .map_err(|_| cli_arg_error("--interval-secs must be an integer"))?;
            }
            "--notional-usd" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--notional-usd requires a decimal"));
                };
                options.notional_usd = value.clone();
            }
            "--taker-fee-bps" | "--perp-fee-bps" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--taker-fee-bps requires a value"));
                };
                options.taker_fee_bps = value.clone();
            }
            "--slippage-bps" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--slippage-bps requires a value"));
                };
                options.slippage_buffer_bps = value
                    .parse::<i128>()
                    .map_err(|_| cli_arg_error("--slippage-bps must be an integer"))?;
            }
            "--max-entry-price-divergence-bps" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error(
                        "--max-entry-price-divergence-bps requires a value",
                    ));
                };
                options.max_entry_price_divergence_bps = value.parse::<i128>().map_err(|_| {
                    cli_arg_error("--max-entry-price-divergence-bps must be an integer")
                })?;
            }
            "--min-net-funding-bps" | "--min-net-bps" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--min-net-funding-bps requires a value"));
                };
                options.min_net_funding_bps = value
                    .parse::<i128>()
                    .map_err(|_| cli_arg_error("--min-net-funding-bps must be an integer"))?;
            }
            "--source" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--source requires venue=url"));
                };
                let (venue, status_url) = parse_funding_arb_source_arg(value)?;
                set_funding_arb_source_url(&mut options, &venue, status_url);
            }
            "--clear-sources" => {
                options.sources.clear();
            }
            "--binance-status-url" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--binance-status-url requires a URL"));
                };
                set_funding_arb_source_url(&mut options, "binance", value.clone());
            }
            "--bybit-status-url" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--bybit-status-url requires a URL"));
                };
                set_funding_arb_source_url(&mut options, "bybit", value.clone());
            }
            "--okx-status-url" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--okx-status-url requires a URL"));
                };
                set_funding_arb_source_url(&mut options, "okx", value.clone());
            }
            "--bitget-status-url" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--bitget-status-url requires a URL"));
                };
                set_funding_arb_source_url(&mut options, "bitget", value.clone());
            }
            "--aster-status-url" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--aster-status-url requires a URL"));
                };
                set_funding_arb_source_url(&mut options, "aster", value.clone());
            }
            "--hyperliquid-status-url" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--hyperliquid-status-url requires a URL"));
                };
                set_funding_arb_source_url(&mut options, "hyperliquid", value.clone());
            }
            "--once" => options.once = true,
            "--out" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--out requires a directory"));
                };
                options.output_dir = Some(PathBuf::from(value));
            }
            "--execution-reports" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--execution-reports requires a file path"));
                };
                options.execution_reports_path = Some(PathBuf::from(value));
            }
            "--resident-events" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--resident-events requires a file path"));
                };
                options.resident_events_path = Some(PathBuf::from(value));
            }
            "--opportunity-history" | "--history" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--opportunity-history requires a file path"));
                };
                options.opportunity_history_path = Some(PathBuf::from(value));
            }
            value if value.starts_with('-') => {
                return Err(cli_arg_error(format!(
                    "unknown funding-arb-monitor option `{value}`"
                )));
            }
            value => {
                return Err(cli_arg_error(format!(
                    "unexpected funding-arb-monitor positional argument `{value}`"
                )));
            }
        }
        index += 1;
    }

    validate_funding_arb_monitor_options(&options)?;
    Ok(options)
}

pub(crate) fn parse_portfolio_dashboard_args(
    args: &[String],
) -> RuntimeResult<PortfolioDashboardCliOptions> {
    let mut options = PortfolioDashboardOptions::default();
    let mut index = 0;

    while index < args.len() {
        match args[index].as_str() {
            "--bind" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--bind requires an address"));
                };
                options.bind_addr = value.clone();
            }
            "--account-snapshot" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--account-snapshot requires a file path"));
                };
                options.account_snapshot_path = Some(PathBuf::from(value));
            }
            "--account-raw-snapshot" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--account-raw-snapshot requires a file path"));
                };
                options.account_raw_snapshot_path = Some(PathBuf::from(value));
            }
            "--position-snapshot" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--position-snapshot requires a file path"));
                };
                options.position_snapshot_path = Some(PathBuf::from(value));
            }
            "--position-raw-snapshot" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error(
                        "--position-raw-snapshot requires a file path",
                    ));
                };
                options.position_raw_snapshot_path = Some(PathBuf::from(value));
            }
            "--funding-snapshot" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--funding-snapshot requires a file path"));
                };
                options.funding_snapshot_path = Some(PathBuf::from(value));
            }
            "--resident-root" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--resident-root requires a directory"));
                };
                options.resident_root = Some(PathBuf::from(value));
            }
            "--navigation-wss-pid-file" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error(
                        "--navigation-wss-pid-file requires a file path",
                    ));
                };
                options.navigation_wss_pid_file = Some(PathBuf::from(value));
            }
            "--once" => options.once = true,
            value if value.starts_with('-') => {
                return Err(cli_arg_error(format!(
                    "unknown portfolio-dashboard option `{value}`"
                )));
            }
            value => {
                return Err(cli_arg_error(format!(
                    "unexpected portfolio-dashboard positional argument `{value}`"
                )));
            }
        }
        index += 1;
    }

    validate_portfolio_dashboard_options(&options)?;
    Ok(options)
}

fn parse_funding_arb_source_arg(value: &str) -> RuntimeResult<(String, String)> {
    let Some((venue, status_url)) = value.split_once('=') else {
        return Err(cli_arg_error("--source must use venue=url"));
    };
    if venue.trim().is_empty() || status_url.trim().is_empty() {
        return Err(cli_arg_error("--source requires non-empty venue and URL"));
    }
    Ok((venue.trim().to_owned(), status_url.trim().to_owned()))
}

fn set_funding_arb_source_url(
    options: &mut FundingArbMonitorOptions,
    venue_family: &str,
    status_url: String,
) {
    set_funding_arb_source_url_raw(&mut options.sources, venue_family, status_url);
}

fn set_funding_arb_source_url_raw(
    sources: &mut Vec<FundingArbVenueSource>,
    venue_family: &str,
    status_url: String,
) {
    let family = normalize_venue_family(venue_family);
    if let Some(source) = sources
        .iter_mut()
        .find(|source| normalize_venue_family(&source.venue_family) == family)
    {
        source.status_url = status_url;
    } else {
        sources.push(FundingArbVenueSource {
            venue_family: family,
            status_url,
        });
    }
}

pub(crate) fn parse_wallet_signer_preflight_args(
    args: &[String],
) -> RuntimeResult<WalletSignerPreflightOptions> {
    let mut output_dir = None;
    let mut aster_signer = None;
    let mut aster_signer_cmd_env = ASTER_EIP712_SIGNER_CMD_ENV_DEFAULT.to_owned();
    let mut skip_aster = false;
    let mut check_hyperliquid = false;
    let mut hyperliquid_agent = None;
    let mut hyperliquid_source = "a".to_owned();
    let mut index = 0;

    while index < args.len() {
        match args[index].as_str() {
            "--aster-signer" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--aster-signer requires an address"));
                };
                aster_signer = Some(value.clone());
            }
            "--aster-signer-cmd-env" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error(
                        "--aster-signer-cmd-env requires an env var name",
                    ));
                };
                aster_signer_cmd_env = value.clone();
            }
            "--skip-aster" => {
                skip_aster = true;
            }
            "--check-hyperliquid" => {
                check_hyperliquid = true;
            }
            "--hyperliquid-agent" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--hyperliquid-agent requires an address"));
                };
                hyperliquid_agent = Some(value.clone());
                check_hyperliquid = true;
            }
            "--hyperliquid-source" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--hyperliquid-source requires `a` or `b`"));
                };
                hyperliquid_source = value.clone();
            }
            "--out" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--out requires a directory"));
                };
                output_dir = Some(PathBuf::from(value));
            }
            value if value.starts_with('-') => {
                return Err(cli_arg_error(format!(
                    "unknown wallet-signer-preflight option `{value}`"
                )));
            }
            value => {
                return Err(cli_arg_error(format!(
                    "unexpected wallet-signer-preflight positional argument `{value}`"
                )));
            }
        }
        index += 1;
    }

    let options = WalletSignerPreflightOptions {
        output_dir,
        aster_signer,
        aster_signer_cmd_env,
        skip_aster,
        check_hyperliquid,
        hyperliquid_agent,
        hyperliquid_source,
    };
    validate_wallet_signer_preflight_options(&options)?;
    Ok(options)
}

pub(crate) fn parse_funding_arb_private_readonly_snapshot_once_args(
    args: &[String],
) -> RuntimeResult<FundingArbPrivateReadonlySnapshotOnceCliOptions> {
    let mut config_path = PathBuf::from("templates/personal_guarded_live.preflight.yaml");
    let mut snapshot_path = None;
    let mut pair_id = None;
    let mut output_dir = None;
    let mut funding_settlement_raw_snapshot_path = None;
    let mut hyperliquid_user = None;
    let mut aster_user = None;
    let mut aster_signer = None;
    let mut aster_signer_cmd_env = ASTER_EIP712_SIGNER_CMD_ENV_DEFAULT.to_owned();
    let mut index = 0;

    while index < args.len() {
        match args[index].as_str() {
            "--config" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--config requires a config path"));
                };
                config_path = PathBuf::from(value);
            }
            "--snapshot" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--snapshot requires a file path"));
                };
                snapshot_path = Some(PathBuf::from(value));
            }
            "--pair-id" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--pair-id requires a value"));
                };
                pair_id = Some(value.clone());
            }
            "--hyperliquid-user" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--hyperliquid-user requires an address"));
                };
                hyperliquid_user = Some(value.clone());
            }
            "--aster-user" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--aster-user requires an address"));
                };
                aster_user = Some(value.clone());
            }
            "--aster-signer" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--aster-signer requires an address"));
                };
                aster_signer = Some(value.clone());
            }
            "--aster-signer-cmd-env" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error(
                        "--aster-signer-cmd-env requires an env var name",
                    ));
                };
                aster_signer_cmd_env = value.clone();
            }
            "--out" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--out requires a directory"));
                };
                output_dir = Some(PathBuf::from(value));
            }
            "--funding-settlement-raw-snapshot" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error(
                        "--funding-settlement-raw-snapshot requires a file path",
                    ));
                };
                funding_settlement_raw_snapshot_path = Some(PathBuf::from(value));
            }
            value if value.starts_with('-') => {
                return Err(cli_arg_error(format!(
                    "unknown funding-arb-private-readonly-snapshot-once option `{value}`"
                )));
            }
            value => {
                return Err(cli_arg_error(format!(
                    "unexpected funding-arb-private-readonly-snapshot-once positional argument `{value}`"
                )));
            }
        }
        index += 1;
    }

    let options = FundingArbPrivateReadonlySnapshotOnceOptions {
        config_path,
        snapshot_path: snapshot_path.ok_or_else(|| cli_arg_error("--snapshot is required"))?,
        pair_id: pair_id.ok_or_else(|| cli_arg_error("--pair-id is required"))?,
        output_dir,
        funding_settlement_raw_snapshot_path,
        hyperliquid_user,
        aster_user,
        aster_signer,
        aster_signer_cmd_env,
    };
    validate_funding_arb_private_readonly_snapshot_once_options(&options)?;
    Ok(options)
}

pub(crate) fn parse_portfolio_private_readonly_snapshot_args(
    args: &[String],
    force_once: bool,
) -> RuntimeResult<PortfolioPrivateReadonlySnapshotOptions> {
    let mut options = PortfolioPrivateReadonlySnapshotOptions {
        once: force_once,
        ..PortfolioPrivateReadonlySnapshotOptions::default()
    };
    let mut index = 0;

    while index < args.len() {
        match args[index].as_str() {
            "--config" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--config requires a config path"));
                };
                options.config_path = PathBuf::from(value);
            }
            "--out" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--out requires a directory"));
                };
                options.output_dir = Some(PathBuf::from(value));
            }
            "--interval-secs" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--interval-secs requires a value"));
                };
                options.interval_secs = value
                    .parse::<u64>()
                    .map_err(|_| cli_arg_error("--interval-secs must be an integer"))?;
            }
            "--clear-venues" => {
                options.venue_families.clear();
            }
            "--venue" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--venue requires a venue family"));
                };
                options.venue_families.push(normalize_venue_family(value));
            }
            "--hyperliquid-user" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--hyperliquid-user requires an address"));
                };
                options.hyperliquid_user = Some(value.clone());
            }
            "--aster-user" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--aster-user requires an address"));
                };
                options.aster_user = Some(value.clone());
            }
            "--aster-signer" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--aster-signer requires an address"));
                };
                options.aster_signer = Some(value.clone());
            }
            "--aster-signer-cmd-env" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error(
                        "--aster-signer-cmd-env requires an env var name",
                    ));
                };
                options.aster_signer_cmd_env = value.clone();
            }
            "--once" => options.once = true,
            value if value.starts_with('-') => {
                return Err(cli_arg_error(format!(
                    "unknown portfolio-private-readonly-snapshot option `{value}`"
                )));
            }
            value => {
                return Err(cli_arg_error(format!(
                    "unexpected portfolio-private-readonly-snapshot positional argument `{value}`"
                )));
            }
        }
        index += 1;
    }

    validate_portfolio_private_readonly_snapshot_options(&options)?;
    Ok(options)
}

pub(crate) fn parse_funding_arb_guarded_dry_run_once_args(
    args: &[String],
) -> RuntimeResult<FundingArbGuardedDryRunOnceCliOptions> {
    let mut config_path = PathBuf::from("templates/personal_guarded_live.preflight.yaml");
    let mut snapshot_path = None;
    let mut pair_id = None;
    let mut funding_settlement_ledger_path = None;
    let mut funding_settlement_raw_snapshot_path = None;
    let mut private_account_snapshot_path = None;
    let mut private_account_raw_snapshot_path = None;
    let mut private_position_snapshot_path = None;
    let mut private_position_raw_snapshot_path = None;
    let mut private_execution_snapshot_path = None;
    let mut output_dir = None;
    let mut notional_usd = BASIS_MONITOR_DEFAULT_NOTIONAL_USD.to_owned();
    let mut taker_fee_bps = BASIS_MONITOR_DEFAULT_PERP_TAKER_FEE_BPS.to_owned();
    let mut slippage_buffer_bps = BASIS_MONITOR_DEFAULT_SLIPPAGE_BUFFER_BPS;
    let mut max_entry_price_divergence_bps = 20;
    let mut min_net_funding_bps = BASIS_MONITOR_DEFAULT_MIN_NET_BPS;
    let mut index = 0;

    while index < args.len() {
        match args[index].as_str() {
            "--config" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--config requires a config path"));
                };
                config_path = PathBuf::from(value);
            }
            "--snapshot" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--snapshot requires a file path"));
                };
                snapshot_path = Some(PathBuf::from(value));
            }
            "--pair-id" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--pair-id requires a value"));
                };
                pair_id = Some(value.clone());
            }
            "--funding-settlement-ledger" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error(
                        "--funding-settlement-ledger requires a file path",
                    ));
                };
                funding_settlement_ledger_path = Some(PathBuf::from(value));
            }
            "--funding-settlement-raw-snapshot" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error(
                        "--funding-settlement-raw-snapshot requires a file path",
                    ));
                };
                funding_settlement_raw_snapshot_path = Some(PathBuf::from(value));
            }
            "--private-account-snapshot" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error(
                        "--private-account-snapshot requires a file path",
                    ));
                };
                private_account_snapshot_path = Some(PathBuf::from(value));
            }
            "--private-account-raw-snapshot" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error(
                        "--private-account-raw-snapshot requires a file path",
                    ));
                };
                private_account_raw_snapshot_path = Some(PathBuf::from(value));
            }
            "--private-position-snapshot" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error(
                        "--private-position-snapshot requires a file path",
                    ));
                };
                private_position_snapshot_path = Some(PathBuf::from(value));
            }
            "--private-position-raw-snapshot" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error(
                        "--private-position-raw-snapshot requires a file path",
                    ));
                };
                private_position_raw_snapshot_path = Some(PathBuf::from(value));
            }
            "--private-execution-snapshot" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error(
                        "--private-execution-snapshot requires a file path",
                    ));
                };
                private_execution_snapshot_path = Some(PathBuf::from(value));
            }
            "--out" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--out requires a directory"));
                };
                output_dir = Some(PathBuf::from(value));
            }
            "--notional-usd" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--notional-usd requires a decimal"));
                };
                notional_usd = value.clone();
            }
            "--taker-fee-bps" | "--perp-fee-bps" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--taker-fee-bps requires a value"));
                };
                taker_fee_bps = value.clone();
            }
            "--slippage-bps" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--slippage-bps requires a value"));
                };
                slippage_buffer_bps = value
                    .parse::<i128>()
                    .map_err(|_| cli_arg_error("--slippage-bps must be an integer"))?;
            }
            "--max-entry-price-divergence-bps" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error(
                        "--max-entry-price-divergence-bps requires a value",
                    ));
                };
                max_entry_price_divergence_bps = value.parse::<i128>().map_err(|_| {
                    cli_arg_error("--max-entry-price-divergence-bps must be an integer")
                })?;
            }
            "--min-net-funding-bps" | "--min-net-bps" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--min-net-funding-bps requires a value"));
                };
                min_net_funding_bps = value
                    .parse::<i128>()
                    .map_err(|_| cli_arg_error("--min-net-funding-bps must be an integer"))?;
            }
            "--dry-run" => {}
            value if value.starts_with('-') => {
                return Err(cli_arg_error(format!(
                    "unknown funding-arb-guarded-dry-run-once option `{value}`"
                )));
            }
            value => {
                return Err(cli_arg_error(format!(
                    "unexpected funding-arb-guarded-dry-run-once positional argument `{value}`"
                )));
            }
        }
        index += 1;
    }

    let options = FundingArbGuardedDryRunOnceOptions {
        config_path,
        snapshot_path: snapshot_path.ok_or_else(|| cli_arg_error("--snapshot is required"))?,
        pair_id: pair_id.ok_or_else(|| cli_arg_error("--pair-id is required"))?,
        funding_settlement_ledger_path,
        funding_settlement_raw_snapshot_path,
        private_account_snapshot_path,
        private_account_raw_snapshot_path,
        private_position_snapshot_path,
        private_position_raw_snapshot_path,
        private_execution_snapshot_path,
        output_dir,
        notional_usd,
        taker_fee_bps,
        slippage_buffer_bps,
        max_entry_price_divergence_bps,
        min_net_funding_bps,
    };
    validate_funding_arb_guarded_dry_run_once_options(&options)?;
    Ok(options)
}

pub(crate) fn parse_funding_arb_guarded_live_canary_once_args(
    args: &[String],
) -> RuntimeResult<FundingArbGuardedLiveCanaryOnceCliOptions> {
    let mut dry_run_args = Vec::new();
    let mut execute_live = false;
    let mut acknowledge_funding_arb_live_orders = false;
    let mut private_order_events_dir = None;
    let mut hyperliquid_user = None;
    let mut hyperliquid_source =
        std::env::var("HYPERLIQUID_SOURCE").unwrap_or_else(|_| "a".to_owned());
    let mut hyperliquid_vault_address = None;
    let mut hyperliquid_expires_after_ms = None;
    let mut hyperliquid_asset_ids = BTreeMap::new();
    let mut aster_user = None;
    let mut aster_signer = None;
    let mut aster_signer_cmd_env = ASTER_EIP712_SIGNER_CMD_ENV_DEFAULT.to_owned();
    let mut index = 0;

    while index < args.len() {
        match args[index].as_str() {
            "--execute-live" => execute_live = true,
            "--i-understand-funding-arb-live-orders" => acknowledge_funding_arb_live_orders = true,
            "--private-order-events-dir" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error(
                        "--private-order-events-dir requires a directory",
                    ));
                };
                private_order_events_dir = Some(PathBuf::from(value));
            }
            "--hyperliquid-user" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--hyperliquid-user requires an address"));
                };
                hyperliquid_user = Some(value.clone());
            }
            "--hyperliquid-source" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--hyperliquid-source requires a|b"));
                };
                hyperliquid_source = value.clone();
            }
            "--hyperliquid-vault-address" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error(
                        "--hyperliquid-vault-address requires an address",
                    ));
                };
                hyperliquid_vault_address = Some(value.clone());
            }
            "--hyperliquid-expires-after-ms" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error(
                        "--hyperliquid-expires-after-ms requires a millisecond timestamp",
                    ));
                };
                hyperliquid_expires_after_ms =
                    Some(value.parse::<u64>().map_err(|_| {
                        cli_arg_error("--hyperliquid-expires-after-ms must be a u64")
                    })?);
            }
            "--hyperliquid-asset-id" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--hyperliquid-asset-id requires SYMBOL=ID"));
                };
                let (symbol, asset_id) = parse_hyperliquid_asset_id_arg(value)?;
                hyperliquid_asset_ids.insert(symbol, asset_id);
            }
            "--aster-user" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--aster-user requires an address"));
                };
                aster_user = Some(value.clone());
            }
            "--aster-signer" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--aster-signer requires an address"));
                };
                aster_signer = Some(value.clone());
            }
            "--aster-signer-cmd-env" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error(
                        "--aster-signer-cmd-env requires an env var name",
                    ));
                };
                aster_signer_cmd_env = value.clone();
            }
            value => dry_run_args.push(value.to_owned()),
        }
        index += 1;
    }

    let options = FundingArbGuardedLiveCanaryOnceOptions {
        dry_run: parse_funding_arb_guarded_dry_run_once_args(&dry_run_args)?,
        execute_live,
        acknowledge_funding_arb_live_orders,
        private_order_events_dir,
        hyperliquid_user,
        hyperliquid_source,
        hyperliquid_vault_address,
        hyperliquid_expires_after_ms,
        hyperliquid_asset_ids,
        aster_user,
        aster_signer,
        aster_signer_cmd_env,
    };
    validate_funding_arb_guarded_live_canary_once_options(&options)?;
    Ok(options)
}

pub(crate) fn parse_funding_arb_resident_live_args(
    args: &[String],
) -> RuntimeResult<FundingArbResidentLiveCliOptions> {
    let mut config_path = PathBuf::from("templates/personal_guarded_live.preflight.yaml");
    let mut output_dir = None;
    let mut snapshot_path = None;
    let mut pair_id = None;
    let mut sources = default_funding_arb_venue_sources();
    let mut funding_settlement_ledger_path = None;
    let mut funding_settlement_raw_snapshot_path = None;
    let mut private_execution_snapshot_path = None;
    let mut private_order_events_dir = None;
    let mut poll_interval_secs = 60_u64;
    let mut max_cycles = None;
    let mut notional_usd = BASIS_MONITOR_DEFAULT_NOTIONAL_USD.to_owned();
    let mut taker_fee_bps = BASIS_MONITOR_DEFAULT_PERP_TAKER_FEE_BPS.to_owned();
    let mut slippage_buffer_bps = BASIS_MONITOR_DEFAULT_SLIPPAGE_BUFFER_BPS;
    let mut max_entry_price_divergence_bps = 20;
    let mut min_net_funding_bps = BASIS_MONITOR_DEFAULT_MIN_NET_BPS;
    let mut execute_live = false;
    let mut acknowledge_funding_arb_live_orders = false;
    let mut allow_unknown_recovery = false;
    let mut auto_residual_de_risk = true;
    let mut exit_only = false;
    let mut hyperliquid_user = None;
    let mut hyperliquid_source =
        std::env::var("HYPERLIQUID_SOURCE").unwrap_or_else(|_| "a".to_owned());
    let mut hyperliquid_vault_address = None;
    let mut hyperliquid_expires_after_ms = None;
    let mut hyperliquid_asset_ids = BTreeMap::new();
    let mut aster_user = None;
    let mut aster_signer = None;
    let mut aster_signer_cmd_env = ASTER_EIP712_SIGNER_CMD_ENV_DEFAULT.to_owned();
    let mut index = 0;

    while index < args.len() {
        match args[index].as_str() {
            "--config" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--config requires a config path"));
                };
                config_path = PathBuf::from(value);
            }
            "--out" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--out requires a directory"));
                };
                output_dir = Some(PathBuf::from(value));
            }
            "--snapshot" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--snapshot requires a file path"));
                };
                snapshot_path = Some(PathBuf::from(value));
            }
            "--pair-id" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--pair-id requires a value"));
                };
                pair_id = Some(value.clone());
            }
            "--source" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--source requires venue=url"));
                };
                let (venue, status_url) = parse_funding_arb_source_arg(value)?;
                set_funding_arb_source_url_raw(&mut sources, &venue, status_url);
            }
            "--clear-sources" => sources.clear(),
            "--binance-status-url" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--binance-status-url requires a URL"));
                };
                set_funding_arb_source_url_raw(&mut sources, "binance", value.clone());
            }
            "--bybit-status-url" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--bybit-status-url requires a URL"));
                };
                set_funding_arb_source_url_raw(&mut sources, "bybit", value.clone());
            }
            "--okx-status-url" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--okx-status-url requires a URL"));
                };
                set_funding_arb_source_url_raw(&mut sources, "okx", value.clone());
            }
            "--bitget-status-url" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--bitget-status-url requires a URL"));
                };
                set_funding_arb_source_url_raw(&mut sources, "bitget", value.clone());
            }
            "--aster-status-url" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--aster-status-url requires a URL"));
                };
                set_funding_arb_source_url_raw(&mut sources, "aster", value.clone());
            }
            "--hyperliquid-status-url" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--hyperliquid-status-url requires a URL"));
                };
                set_funding_arb_source_url_raw(&mut sources, "hyperliquid", value.clone());
            }
            "--funding-settlement-ledger" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error(
                        "--funding-settlement-ledger requires a file path",
                    ));
                };
                funding_settlement_ledger_path = Some(PathBuf::from(value));
            }
            "--funding-settlement-raw-snapshot" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error(
                        "--funding-settlement-raw-snapshot requires a file path",
                    ));
                };
                funding_settlement_raw_snapshot_path = Some(PathBuf::from(value));
            }
            "--private-execution-snapshot" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error(
                        "--private-execution-snapshot requires a file path",
                    ));
                };
                private_execution_snapshot_path = Some(PathBuf::from(value));
            }
            "--private-order-events-dir" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error(
                        "--private-order-events-dir requires a directory",
                    ));
                };
                private_order_events_dir = Some(PathBuf::from(value));
            }
            "--interval-secs" | "--poll-interval-secs" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--interval-secs requires an integer"));
                };
                poll_interval_secs = value
                    .parse::<u64>()
                    .map_err(|_| cli_arg_error("--interval-secs must be an integer"))?;
            }
            "--max-cycles" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--max-cycles requires an integer"));
                };
                let parsed = value
                    .parse::<u64>()
                    .map_err(|_| cli_arg_error("--max-cycles must be an integer"))?;
                if parsed == 0 {
                    return Err(cli_arg_error("--max-cycles must be greater than zero"));
                }
                max_cycles = Some(parsed);
            }
            "--notional-usd" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--notional-usd requires a decimal"));
                };
                notional_usd = value.clone();
            }
            "--taker-fee-bps" | "--perp-fee-bps" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--taker-fee-bps requires a value"));
                };
                taker_fee_bps = value.clone();
            }
            "--slippage-bps" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--slippage-bps requires a value"));
                };
                slippage_buffer_bps = value
                    .parse::<i128>()
                    .map_err(|_| cli_arg_error("--slippage-bps must be an integer"))?;
            }
            "--max-entry-price-divergence-bps" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error(
                        "--max-entry-price-divergence-bps requires a value",
                    ));
                };
                max_entry_price_divergence_bps = value.parse::<i128>().map_err(|_| {
                    cli_arg_error("--max-entry-price-divergence-bps must be an integer")
                })?;
            }
            "--min-net-funding-bps" | "--min-net-bps" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--min-net-funding-bps requires a value"));
                };
                min_net_funding_bps = value
                    .parse::<i128>()
                    .map_err(|_| cli_arg_error("--min-net-funding-bps must be an integer"))?;
            }
            "--execute-live" => execute_live = true,
            "--dry-run" => execute_live = false,
            "--i-understand-funding-arb-live-orders" => acknowledge_funding_arb_live_orders = true,
            "--allow-unknown-recovery" => allow_unknown_recovery = true,
            "--auto-residual-de-risk" => auto_residual_de_risk = true,
            "--disable-auto-residual-de-risk" | "--no-auto-residual-de-risk" => {
                auto_residual_de_risk = false
            }
            "--exit-only" => exit_only = true,
            "--hyperliquid-user" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--hyperliquid-user requires an address"));
                };
                hyperliquid_user = Some(value.clone());
            }
            "--hyperliquid-source" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--hyperliquid-source requires a|b"));
                };
                hyperliquid_source = value.clone();
            }
            "--hyperliquid-vault-address" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error(
                        "--hyperliquid-vault-address requires an address",
                    ));
                };
                hyperliquid_vault_address = Some(value.clone());
            }
            "--hyperliquid-expires-after-ms" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error(
                        "--hyperliquid-expires-after-ms requires a millisecond timestamp",
                    ));
                };
                hyperliquid_expires_after_ms =
                    Some(value.parse::<u64>().map_err(|_| {
                        cli_arg_error("--hyperliquid-expires-after-ms must be a u64")
                    })?);
            }
            "--hyperliquid-asset-id" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--hyperliquid-asset-id requires SYMBOL=ID"));
                };
                let (symbol, asset_id) = parse_hyperliquid_asset_id_arg(value)?;
                hyperliquid_asset_ids.insert(symbol, asset_id);
            }
            "--aster-user" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--aster-user requires an address"));
                };
                aster_user = Some(value.clone());
            }
            "--aster-signer" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--aster-signer requires an address"));
                };
                aster_signer = Some(value.clone());
            }
            "--aster-signer-cmd-env" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error(
                        "--aster-signer-cmd-env requires an env var name",
                    ));
                };
                aster_signer_cmd_env = value.clone();
            }
            value if value.starts_with('-') => {
                return Err(cli_arg_error(format!(
                    "unknown funding-arb-resident-live option `{value}`"
                )));
            }
            value => {
                return Err(cli_arg_error(format!(
                    "unexpected funding-arb-resident-live positional argument `{value}`"
                )));
            }
        }
        index += 1;
    }

    let options = FundingArbResidentLiveOptions {
        config_path,
        output_dir,
        snapshot_path,
        pair_id,
        sources,
        funding_settlement_ledger_path,
        funding_settlement_raw_snapshot_path,
        private_execution_snapshot_path,
        private_order_events_dir,
        poll_interval_secs,
        max_cycles,
        notional_usd,
        taker_fee_bps,
        slippage_buffer_bps,
        max_entry_price_divergence_bps,
        min_net_funding_bps,
        execute_live,
        acknowledge_funding_arb_live_orders,
        allow_unknown_recovery,
        auto_residual_de_risk,
        exit_only,
        hyperliquid_user,
        hyperliquid_source,
        hyperliquid_vault_address,
        hyperliquid_expires_after_ms,
        hyperliquid_asset_ids,
        aster_user,
        aster_signer,
        aster_signer_cmd_env,
    };
    validate_funding_arb_resident_live_options(&options)?;
    Ok(options)
}

pub(crate) fn parse_hyperliquid_asset_id_arg(value: &str) -> RuntimeResult<(String, u32)> {
    let Some((symbol, asset_id)) = value.split_once('=') else {
        return Err(cli_arg_error(
            "--hyperliquid-asset-id must be shaped as SYMBOL=ID",
        ));
    };
    let symbol = symbol.trim();
    if symbol.is_empty()
        || symbol
            .bytes()
            .any(|byte| !(byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'/')))
    {
        return Err(cli_arg_error(
            "--hyperliquid-asset-id symbol contains an unsupported byte",
        ));
    }
    let asset_id = asset_id
        .trim()
        .parse::<u32>()
        .map_err(|_| cli_arg_error("--hyperliquid-asset-id ID must be a u32"))?;
    Ok((symbol.to_owned(), asset_id))
}

#[cfg(feature = "live-exec")]
pub(crate) fn parse_opportunity_recorder_args(
    args: &[String],
) -> RuntimeResult<OpportunityRecorderOptions> {
    let mut root = PathBuf::from("target/arb-opportunity-observer");
    let mut opportunity_dir = None::<PathBuf>;
    let mut logs_dir = None::<PathBuf>;
    let mut basis_resident_out_dir = None::<PathBuf>;
    let mut funding_arb_resident_out_dir = None::<PathBuf>;
    let mut spot_sources = Vec::<OpportunityRecorderSource>::new();
    let mut funding_arb_url =
        Some("http://127.0.0.1:8804/api/funding-arb/opportunities".to_owned());
    let mut strategies =
        parse_opportunity_recorder_strategy_list("spot-perp-basis,cross-exchange-funding-arb")?;
    let mut execution_mode = "paper".to_owned();
    let mut spot_perp_basis_mode = "resident".to_owned();
    let mut funding_arb_mode = "resident".to_owned();
    let mut interval_secs = 5_u64;
    let mut timeout_secs = PUBLIC_MARKET_CURL_MAX_TIME_SECS;
    let mut retries = PUBLIC_MARKET_CURL_ATTEMPTS;
    let mut retry_sleep_ms = PUBLIC_MARKET_CURL_RETRY_SLEEP_MS;
    let mut blocking_path_limit = 12_usize;
    let mut health_sample_secs = 300_u64;
    let mut resident_health_tail_lines = 200_usize;
    let mut once = false;
    let mut index = 0_usize;

    while index < args.len() {
        match args[index].as_str() {
            "--root" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--root requires a directory"));
                };
                root = PathBuf::from(value);
            }
            "--opportunity-dir" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--opportunity-dir requires a directory"));
                };
                opportunity_dir = Some(PathBuf::from(value));
            }
            "--logs-dir" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--logs-dir requires a directory"));
                };
                logs_dir = Some(PathBuf::from(value));
            }
            "--basis-resident-out" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--basis-resident-out requires a directory"));
                };
                basis_resident_out_dir = Some(PathBuf::from(value));
            }
            "--funding-arb-resident-out" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error(
                        "--funding-arb-resident-out requires a directory",
                    ));
                };
                funding_arb_resident_out_dir = Some(PathBuf::from(value));
            }
            "--source" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--source requires venue=url"));
                };
                spot_sources.push(parse_opportunity_recorder_source(value)?);
            }
            "--clear-sources" => {
                spot_sources.clear();
            }
            "--funding-arb-url" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--funding-arb-url requires a URL"));
                };
                funding_arb_url = Some(value.clone());
            }
            "--no-funding-arb-url" => {
                funding_arb_url = None;
            }
            "--strategies" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error(
                        "--strategies requires a comma-separated list",
                    ));
                };
                strategies = parse_opportunity_recorder_strategy_list(value)?;
            }
            "--execution-mode" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--execution-mode requires a value"));
                };
                execution_mode = value.clone();
            }
            "--spot-perp-basis-mode" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--spot-perp-basis-mode requires a value"));
                };
                spot_perp_basis_mode = value.clone();
            }
            "--funding-arb-mode" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--funding-arb-mode requires a value"));
                };
                funding_arb_mode = value.clone();
            }
            "--interval-secs" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--interval-secs requires an integer"));
                };
                interval_secs = parse_positive_u64_cli("--interval-secs", value)?;
            }
            "--timeout-secs" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--timeout-secs requires an integer"));
                };
                timeout_secs = parse_positive_u64_cli("--timeout-secs", value)?;
            }
            "--retries" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--retries requires an integer"));
                };
                retries = parse_positive_usize_cli("--retries", value)?;
            }
            "--retry-sleep-secs" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--retry-sleep-secs requires an integer"));
                };
                retry_sleep_ms =
                    parse_nonnegative_u64_cli("--retry-sleep-secs", value)?.saturating_mul(1000);
            }
            "--retry-sleep-ms" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--retry-sleep-ms requires an integer"));
                };
                retry_sleep_ms = parse_nonnegative_u64_cli("--retry-sleep-ms", value)?;
            }
            "--blocking-path-event-limit" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error(
                        "--blocking-path-event-limit requires an integer",
                    ));
                };
                blocking_path_limit =
                    parse_nonnegative_usize_cli("--blocking-path-event-limit", value)?;
            }
            "--health-sample-secs" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--health-sample-secs requires an integer"));
                };
                health_sample_secs = parse_nonnegative_u64_cli("--health-sample-secs", value)?;
            }
            "--resident-health-tail-lines" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error(
                        "--resident-health-tail-lines requires an integer",
                    ));
                };
                resident_health_tail_lines =
                    parse_nonnegative_usize_cli("--resident-health-tail-lines", value)?;
            }
            "--once" => {
                once = true;
            }
            value if value.starts_with('-') => {
                return Err(cli_arg_error(format!(
                    "unknown opportunity-recorder option `{value}`"
                )));
            }
            value => {
                return Err(cli_arg_error(format!(
                    "unexpected opportunity-recorder positional argument `{value}`"
                )));
            }
        }
        index += 1;
    }

    if spot_sources.is_empty() {
        spot_sources = default_opportunity_recorder_sources();
    }
    let opportunity_dir = opportunity_dir.unwrap_or_else(|| root.join("opportunities"));
    let logs_dir = logs_dir.unwrap_or_else(|| root.join("logs"));
    let feedback_log = logs_dir.join("realtime-feedback.log");
    let health_events_jsonl = logs_dir.join("health-events.jsonl");
    let basis_resident_out_dir =
        basis_resident_out_dir.unwrap_or_else(|| root.join("resident-live/spot-perp-basis"));
    let funding_arb_resident_out_dir = funding_arb_resident_out_dir
        .unwrap_or_else(|| root.join("resident-live/cross-exchange-funding-arb"));

    Ok(OpportunityRecorderOptions {
        root,
        opportunity_dir,
        logs_dir,
        feedback_log,
        health_events_jsonl,
        basis_resident_out_dir,
        funding_arb_resident_out_dir,
        spot_sources,
        funding_arb_url,
        strategies,
        execution_mode,
        spot_perp_basis_mode,
        funding_arb_mode,
        interval_secs,
        timeout_secs,
        retries,
        retry_sleep_ms,
        blocking_path_limit,
        health_sample_secs,
        resident_health_tail_lines,
        once,
    })
}

#[cfg(feature = "live-exec")]
pub(crate) fn parse_opportunity_recorder_source(
    value: &str,
) -> RuntimeResult<OpportunityRecorderSource> {
    let Some((venue, url)) = value.split_once('=') else {
        return Err(cli_arg_error("--source must be shaped as venue=url"));
    };
    let venue = venue.trim();
    let url = url.trim();
    if venue.is_empty() || url.is_empty() {
        return Err(cli_arg_error("--source venue and URL cannot be empty"));
    }
    if venue
        .bytes()
        .any(|byte| !(byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_'))
    {
        return Err(cli_arg_error("--source venue contains an unsupported byte"));
    }
    parse_public_http_url(url)
        .map_err(|message| cli_arg_error(format!("invalid --source URL: {message}")))?;
    Ok(OpportunityRecorderSource {
        venue: venue.to_owned(),
        url: url.to_owned(),
    })
}

#[cfg(feature = "live-exec")]
pub(crate) fn default_opportunity_recorder_sources() -> Vec<OpportunityRecorderSource> {
    [
        ("binance", "http://127.0.0.1:8796/api/basis/opportunities"),
        (
            "bybit",
            "http://127.0.0.1:8797/api/bybit-basis/opportunities",
        ),
        ("okx", "http://127.0.0.1:8798/api/okx-basis/opportunities"),
        (
            "bitget",
            "http://127.0.0.1:8803/api/bitget-basis/opportunities",
        ),
        (
            "aster",
            "http://127.0.0.1:8800/api/aster-basis/opportunities",
        ),
        (
            "hyperliquid",
            "http://127.0.0.1:8799/api/hyperliquid-basis/opportunities",
        ),
    ]
    .into_iter()
    .map(|(venue, url)| OpportunityRecorderSource {
        venue: venue.to_owned(),
        url: url.to_owned(),
    })
    .collect()
}

#[cfg(feature = "live-exec")]
pub(crate) fn parse_opportunity_recorder_strategy_list(
    raw: &str,
) -> RuntimeResult<BTreeSet<String>> {
    let mut strategies = BTreeSet::new();
    for item in raw.split(',') {
        let strategy = item.trim();
        if strategy.is_empty() {
            continue;
        }
        match strategy {
            SPOT_PERP_BASIS_OBSERVER_STRATEGY | CROSS_EXCHANGE_FUNDING_ARB_OBSERVER_STRATEGY => {
                strategies.insert(strategy.to_owned());
            }
            _ => {
                return Err(cli_arg_error(format!(
                    "unknown opportunity-recorder strategy `{strategy}`"
                )));
            }
        }
    }
    if strategies.is_empty() {
        return Err(cli_arg_error(
            "opportunity-recorder requires at least one strategy",
        ));
    }
    Ok(strategies)
}

#[cfg(feature = "live-exec")]
pub(crate) fn parse_positive_u64_cli(name: &'static str, value: &str) -> RuntimeResult<u64> {
    let parsed = parse_nonnegative_u64_cli(name, value)?;
    if parsed == 0 {
        return Err(cli_arg_error(format!("{name} must be greater than zero")));
    }
    Ok(parsed)
}

#[cfg(feature = "live-exec")]
pub(crate) fn parse_nonnegative_u64_cli(name: &'static str, value: &str) -> RuntimeResult<u64> {
    value
        .parse::<u64>()
        .map_err(|_| cli_arg_error(format!("{name} must be a non-negative integer")))
}

#[cfg(feature = "live-exec")]
pub(crate) fn parse_positive_usize_cli(name: &'static str, value: &str) -> RuntimeResult<usize> {
    let parsed = parse_nonnegative_usize_cli(name, value)?;
    if parsed == 0 {
        return Err(cli_arg_error(format!("{name} must be greater than zero")));
    }
    Ok(parsed)
}

#[cfg(feature = "live-exec")]
pub(crate) fn parse_nonnegative_usize_cli(name: &'static str, value: &str) -> RuntimeResult<usize> {
    value
        .parse::<usize>()
        .map_err(|_| cli_arg_error(format!("{name} must be a non-negative integer")))
}
