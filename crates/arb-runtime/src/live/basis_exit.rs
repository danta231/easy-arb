#![allow(clippy::wildcard_imports)]

use crate::*;

/// 运行 spot-perp basis 平仓监督器。
pub(crate) fn run_basis_exit_supervisor_impl(
    options: BasisExitSupervisorOptions,
) -> RuntimeResult<BasisExitSupervisorReport> {
    #[cfg(feature = "live-exec")]
    {
        run_basis_exit_supervisor_live(options)
    }
    #[cfg(not(feature = "live-exec"))]
    {
        let _ = options;
        Err(RuntimeError::UnsafeConfig {
            message: "当前 arb-runtime 未使用 live-exec feature 构建，拒绝 basis 平仓监督器"
                .to_owned(),
        })
    }
}

#[cfg(feature = "live-exec")]
pub(crate) fn run_basis_exit_supervisor_live(
    options: BasisExitSupervisorOptions,
) -> RuntimeResult<BasisExitSupervisorReport> {
    if options.poll_interval_secs == 0 {
        return Err(RuntimeError::UnsafeConfig {
            message: "basis exit supervisor poll interval must be greater than zero".to_owned(),
        });
    }
    let mut latest_report = run_basis_exit_supervisor_once(&options)?;
    while !options.once && latest_report.decision == SpotPerpBasisExitDecision::Hold.as_str() {
        thread::sleep(Duration::from_secs(options.poll_interval_secs));
        latest_report = run_basis_exit_supervisor_once(&options)?;
    }
    Ok(latest_report)
}

#[cfg(feature = "live-exec")]
fn run_basis_exit_supervisor_once(
    options: &BasisExitSupervisorOptions,
) -> RuntimeResult<BasisExitSupervisorReport> {
    let state_json = read_utf8(&options.position_state_path)?;
    let state = parse_basis_exit_supervisor_state_json(&state_json)?;
    let output_root = options.output_dir.clone().unwrap_or_else(|| {
        options
            .position_state_path
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("target/basis-exit-supervisor"))
    });
    let market = fetch_basis_exit_market_snapshot(&state, options, &output_root)?;
    let adl_state = latest_basis_adl_state(options.adl_events_dir.as_deref(), &state)?;
    let config = arb_config::ArbConfig::from_path(&options.config_path)?;
    let signing_policy = if options.execute_live {
        Some(signing_policy_from_config(&config)?)
    } else {
        None
    };
    let reconciled = reconcile_basis_exit_position(
        &state,
        &market,
        signing_policy.as_ref(),
        current_utc_timestamp()?,
    )?;
    let signal = evaluate_spot_perp_basis_exit_signal(&SpotPerpBasisExitSignalInput {
        symbol: state.symbol.clone(),
        spot_best_bid: market.spot_bid.clone(),
        perp_best_ask: market.perp_ask.clone(),
        notional_usd: state.notional_usd.clone(),
        entry_gross_basis_bps: state.entry_gross_basis_bps,
        entry_total_cost_bps: state.entry_total_cost_bps,
        accumulated_funding_bps: state.accumulated_funding_bps,
        expected_next_funding_bps: market.expected_next_funding_bps,
        exit_spot_taker_fee_bps: monitor_bps_ceil_i128(
            "exit_spot_taker_fee_bps",
            BASIS_MONITOR_DEFAULT_SPOT_TAKER_FEE_BPS,
        )?,
        exit_perp_taker_fee_bps: monitor_bps_ceil_i128(
            "exit_perp_taker_fee_bps",
            BASIS_MONITOR_DEFAULT_PERP_TAKER_FEE_BPS,
        )?,
        exit_slippage_buffer_bps: BASIS_MONITOR_DEFAULT_SLIPPAGE_BUFFER_BPS,
        target_profit_bps: BASIS_EXIT_DEFAULT_TARGET_PROFIT_BPS,
        convergence_buffer_bps: BASIS_EXIT_DEFAULT_CONVERGENCE_BUFFER_BPS,
        min_next_funding_bps: BASIS_EXIT_DEFAULT_MIN_NEXT_FUNDING_BPS,
        max_basis_widen_bps: BASIS_EXIT_DEFAULT_MAX_BASIS_WIDEN_BPS,
        max_loss_bps: BASIS_EXIT_DEFAULT_MAX_LOSS_BPS,
        liquidation_buffer_bps: reconciled.liquidation_buffer_bps,
        min_liquidation_buffer_bps: BASIS_EXIT_DEFAULT_MIN_LIQUIDATION_BUFFER_BPS,
        position_imbalance_bps: reconciled.position_imbalance_bps,
        max_position_imbalance_bps: BASIS_EXIT_DEFAULT_MAX_POSITION_IMBALANCE_BPS,
        data_is_stale: false,
        external_state_unknown: reconciled.external_state_unknown,
        adl_state,
    })
    .map_err(|message| RuntimeError::Module {
        module: "arb-strategies",
        message,
    })?;

    let mut blocking_reasons = Vec::new();
    let mut receipts = Vec::new();
    let mut confirmations = Vec::new();
    let mut residual_risk = reconciled.residual_risk.clone();
    let mut dispatch_attempted = false;

    if signal.decision != SpotPerpBasisExitDecision::Hold {
        if options.execute_live {
            if !options.acknowledge_basis_live_orders {
                blocking_reasons.push(
                    "缺少 --i-understand-basis-live-orders，拒绝提交 basis 平仓订单".to_owned(),
                );
            } else if reconciled.external_state_unknown {
                blocking_reasons.push(
                    "私有仓位状态未知，自动平仓被抑制；必须先恢复交易所私有状态源".to_owned(),
                );
            } else {
                let signing_policy = signing_policy.ok_or_else(|| RuntimeError::UnsafeConfig {
                    message: "basis exit supervisor live signing policy missing".to_owned(),
                })?;
                dispatch_attempted = true;
                let outcome = execute_basis_exit_close_orders(
                    &state,
                    &market,
                    &reconciled,
                    &signing_policy,
                    &signal,
                )?;
                receipts = outcome.receipts;
                confirmations = outcome.confirmations;
                blocking_reasons.extend(outcome.blocking_reasons);
                residual_risk = outcome.protection.residual_risk.or(residual_risk);
            }
        } else {
            blocking_reasons.push(
                "basis exit supervisor ran in dry-run mode; pass --execute-live and --i-understand-basis-live-orders to submit close legs"
                    .to_owned(),
            );
        }
    }

    let report = BasisExitSupervisorReport {
        symbol: state.symbol.clone(),
        strategy_id: state.strategy_id.clone(),
        decision: signal.decision.as_str().to_owned(),
        reason_codes: signal
            .reason_codes
            .iter()
            .map(|reason| reason.as_str().to_owned())
            .collect(),
        signal: basis_exit_signal_summary(&signal),
        adl_state: adl_state.as_str().to_owned(),
        liquidation_buffer_bps: reconciled.liquidation_buffer_bps,
        position_imbalance_bps: reconciled.position_imbalance_bps,
        dispatch_attempted,
        submitted_receipt_count: receipts.len(),
        private_confirmation_count: confirmations.len(),
        residual_risk,
        blocking_reasons,
        output_dir: Some(output_root.clone()),
    };
    write_basis_exit_supervisor_artifacts(&output_root, &report, &receipts, &confirmations)?;
    Ok(report)
}
