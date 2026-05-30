#![allow(clippy::wildcard_imports)]

#[cfg(feature = "live-exec")]
use crate::*;

#[cfg(feature = "live-exec")]
pub(crate) fn run_funding_arb_resident_exit_cycle(
    options: &FundingArbResidentLiveOptions,
    position_state_path: &Path,
    output_dir: &Path,
) -> RuntimeResult<FundingArbExitCycleReport> {
    fs::create_dir_all(output_dir).map_err(|error| RuntimeError::Io {
        path: output_dir.to_path_buf(),
        message: error.to_string(),
    })?;
    let mut state = parse_funding_arb_position_state_json(&read_utf8(position_state_path)?)?;
    let snapshot = match load_funding_arb_resident_snapshot(options) {
        Ok(snapshot) => snapshot,
        Err(error) if funding_arb_exit_pre_dispatch_error_can_retry(&error) => {
            return funding_arb_exit_cycle_blocked_before_dispatch(
                &state,
                output_dir,
                "exit_market_snapshot_unavailable",
                format!(
                    "funding arb exit market snapshot unavailable before dispatch; exit will retry: {error}"
                ),
            );
        }
        Err(error) => return Err(error),
    };
    let snapshot_path = output_dir.join("funding_arb_monitor_snapshot.json");
    let selected_row = snapshot
        .rows
        .iter()
        .find(|row| row.pair_id == state.pair_id)
        .cloned();
    let retained_rows = selected_row.iter().cloned().collect::<Vec<_>>();
    write_funding_arb_monitor_snapshot_artifact(
        snapshot_path.clone(),
        &snapshot,
        &retained_rows,
        if selected_row.is_some() {
            "compact_resident_exit_active_pair"
        } else {
            "compact_resident_exit_missing_pair"
        },
    )?;
    let Some(row) = selected_row else {
        return funding_arb_exit_cycle_blocked_before_dispatch(
            &state,
            output_dir,
            "active_pair_missing_from_exit_snapshot",
            format!(
                "funding arb exit snapshot does not contain active pair_id `{}`; position remains open and exit will retry",
                state.pair_id
            ),
        );
    };
    let row = &row;
    let spec = funding_arb_pipeline_spec_from_monitor_row(
        row,
        &funding_arb_resident_dry_run_options(
            options,
            snapshot_path.clone(),
            state.pair_id.clone(),
            None,
        ),
    )?;
    let private = match run_funding_arb_private_readonly_snapshot_once(
        FundingArbPrivateReadonlySnapshotOnceOptions {
            config_path: options.config_path.clone(),
            snapshot_path,
            pair_id: state.pair_id.clone(),
            output_dir: Some(output_dir.join("private-readonly")),
            funding_settlement_raw_snapshot_path: options
                .funding_settlement_raw_snapshot_path
                .clone(),
            hyperliquid_user: options.hyperliquid_user.clone(),
            aster_user: options.aster_user.clone(),
            aster_signer: options.aster_signer.clone(),
            aster_signer_cmd_env: options.aster_signer_cmd_env.clone(),
        },
    ) {
        Ok(private) => private,
        Err(error) if funding_arb_private_readonly_error_is_unavailable(&error) => {
            return funding_arb_exit_cycle_blocked_before_dispatch(
                &state,
                output_dir,
                "private_readonly_snapshot_unavailable",
                format!(
                    "funding arb exit private read-only snapshot unavailable before dispatch; exit will retry: {error}"
                ),
            );
        }
        Err(error) => return Err(error),
    };
    let position_raw_json = read_utf8(&private.position_raw_snapshot_path)?;
    let position_raw = parse_funding_private_raw_snapshot_json(
        &position_raw_json,
        "funding arb exit private position raw snapshot",
    )?;
    let position_snapshot =
        funding_private_position_snapshot_from_raw_snapshot(row, &position_raw)?;
    let position_status = funding_arb_exit_private_position_status(row, &spec, &position_snapshot)?;
    let account_raw_json = read_utf8(&private.account_raw_snapshot_path)?;
    let account_raw = parse_funding_private_raw_snapshot_json(
        &account_raw_json,
        "funding arb exit private account raw snapshot",
    )?;
    let account_snapshot = funding_private_account_snapshot_from_raw_snapshot(&account_raw)?;
    let account_status = reconcile_funding_private_account_snapshot(&spec, &account_snapshot)?;
    let binance_position_mode = if funding_arb_monitor_row_includes_family(row, "binance") {
        resolve_funding_arb_binance_position_mode(
            &row.symbol,
            Some(&private.position_raw_snapshot_path),
        )?
    } else {
        FundingArbBinancePositionMode::OneWay
    };
    let bybit_position_mode = if funding_arb_monitor_row_includes_family(row, "bybit") {
        resolve_funding_arb_bybit_position_mode(
            &row.symbol,
            Some(&private.position_raw_snapshot_path),
        )?
    } else {
        FundingArbBybitPositionMode::OneWay
    };
    let okx_position_mode = if funding_arb_monitor_row_includes_family(row, "okx") {
        resolve_funding_arb_okx_position_mode(
            &row.symbol,
            Some(&private.position_raw_snapshot_path),
        )?
    } else {
        FundingArbOkxPositionMode::Net
    };
    let bitget_position_mode = if funding_arb_monitor_row_includes_family(row, "bitget") {
        resolve_funding_arb_bitget_position_mode(
            &row.symbol,
            Some(&private.position_raw_snapshot_path),
        )?
    } else {
        FundingArbBitgetPositionMode::OneWay
    };
    let aster_position_mode = if funding_arb_monitor_row_includes_family(row, "aster") {
        resolve_funding_arb_aster_position_mode(
            &row.symbol,
            Some(&private.position_raw_snapshot_path),
        )?
    } else {
        FundingArbAsterPositionMode::OneWay
    };
    let position_modes = FundingArbResolvedPositionModes {
        binance: binance_position_mode,
        bybit: bybit_position_mode,
        okx: okx_position_mode,
        bitget: bitget_position_mode,
        aster: aster_position_mode,
    };
    if funding_arb_unknown_recovery_enabled(options) && state.plan_hash.is_none() {
        let mut aligned_state = state.clone();
        if funding_arb_align_unknown_recovery_state_to_private_positions(
            &mut aligned_state,
            row,
            &position_snapshot,
        )? {
            write_funding_arb_position_state_path(position_state_path, &aligned_state)?;
        }
        state = aligned_state;
    }
    let settlement = funding_arb_resident_settlement_reconciliation(
        options,
        row,
        &spec,
        &state,
        Some(&private.settlement_raw_snapshot_path),
    )?;
    let mut leg_a = funding_arb_exit_leg_snapshot(
        row,
        &state.leg_a,
        &position_snapshot,
        options.slippage_buffer_bps,
    )?;
    let mut leg_b = funding_arb_exit_leg_snapshot(
        row,
        &state.leg_b,
        &position_snapshot,
        options.slippage_buffer_bps,
    )?;
    let runtime_risk = funding_arb_exit_runtime_risk_summary(
        row,
        &position_snapshot,
        &leg_a,
        &leg_b,
        &account_status,
    )?;
    let mut reason_codes = Vec::new();
    let mut blocking_reasons = Vec::new();
    let mut receipts = Vec::new();
    let mut confirmations = Vec::new();
    let mut residual_risk = None;
    let mut dispatch_attempted = false;
    let mut partial_close = false;
    let mut requested_close_quantity = None;

    let close_required_count = [leg_a.close_required, leg_b.close_required]
        .into_iter()
        .filter(|required| *required)
        .count();
    let mut position_state_changed = false;
    let financial_risk = if position_status.is_matched() && close_required_count == 2 {
        position_state_changed = true;
        funding_arb_exit_financial_risk_summary(row, &mut state, options, &snapshot.updated_at)?
    } else {
        FundingArbExitFinancialRiskSummary::not_checked()
    };
    let rollover_allowed = settlement.is_observed_complete()
        && funding_arb_exit_rollover_allowed(row, &state, options.min_net_funding_bps);
    let decision = funding_arb_exit_cycle_decision(
        FundingArbExitCycleDecisionInput {
            private_position_matched: position_status.is_matched(),
            private_position_reason: position_status.reason.as_deref(),
            settlement_matched: settlement.is_matched(),
            settlement_observed_complete: settlement.is_observed_complete(),
            settlement_observed_mismatch: settlement.is_observed_mismatch(),
            rollover_allowed,
            close_required_count,
            runtime_risk: Some(&runtime_risk),
            financial_risk: Some(&financial_risk),
            unknown_recovery_exit: funding_arb_unknown_recovery_enabled(options)
                && state.plan_hash.is_none(),
        },
        &mut reason_codes,
        &mut blocking_reasons,
    );
    if decision == "rollover" {
        funding_arb_rollover_position_state_target(&mut state, row)?;
        position_state_changed = true;
    }
    if position_state_changed {
        write_funding_arb_position_state_path(position_state_path, &state)?;
    }

    if matches!(decision.as_str(), "close" | "emergency_de_risk") {
        let mut close_legs = [leg_a.clone(), leg_b.clone()];
        let mut close_blocked = false;
        let close_liquidity_blocking_reasons = if decision == "close" {
            funding_arb_exit_close_liquidity_blocking_reasons(
                row,
                [&close_legs[0], &close_legs[1]],
            )?
        } else {
            Vec::new()
        };
        if !close_liquidity_blocking_reasons.is_empty() {
            if financial_risk.is_triggered() {
                if let Some((partial_legs, quantity)) =
                    funding_arb_exit_partial_close_legs(row, [&close_legs[0], &close_legs[1]])?
                {
                    funding_arb_push_unique_reason_code(
                        &mut reason_codes,
                        "exit_close_liquidity_partial",
                    );
                    partial_close = true;
                    requested_close_quantity = Some(quantity);
                    close_legs = partial_legs;
                } else {
                    funding_arb_push_unique_reason_code(
                        &mut reason_codes,
                        "exit_close_liquidity_insufficient",
                    );
                    blocking_reasons.extend(close_liquidity_blocking_reasons);
                    close_blocked = true;
                }
            } else {
                funding_arb_push_unique_reason_code(
                    &mut reason_codes,
                    "exit_close_liquidity_insufficient",
                );
                blocking_reasons.extend(close_liquidity_blocking_reasons);
                close_blocked = true;
            }
        }
        if !close_blocked {
            if !options.acknowledge_funding_arb_live_orders {
                blocking_reasons.push(
                    "缺少 --i-understand-funding-arb-live-orders，拒绝提交 funding arb 退出订单"
                        .to_owned(),
                );
            } else {
                if decision == "emergency_de_risk" {
                    let de_risk_slippage_buffer_bps =
                        funding_arb_exit_emergency_de_risk_slippage_bps(
                            options.slippage_buffer_bps,
                        );
                    leg_a = funding_arb_exit_leg_snapshot(
                        row,
                        &state.leg_a,
                        &position_snapshot,
                        de_risk_slippage_buffer_bps,
                    )?;
                    leg_b = funding_arb_exit_leg_snapshot(
                        row,
                        &state.leg_b,
                        &position_snapshot,
                        de_risk_slippage_buffer_bps,
                    )?;
                    close_legs = [leg_a.clone(), leg_b.clone()];
                }
                let config = arb_config::ArbConfig::from_path(&options.config_path)?;
                let signing_policy = signing_policy_from_config(&config)?;
                let private_order_events = state
                    .private_order_events_dir
                    .as_ref()
                    .map(|path| PrivateOrderEventStore::from_dir(Path::new(path)))
                    .transpose()?;
                dispatch_attempted = true;
                let outcome = execute_funding_arb_exit_close_orders(
                    options,
                    &signing_policy,
                    &close_legs,
                    private_order_events.as_ref(),
                    position_modes,
                )?;
                receipts = outcome.receipts;
                confirmations = outcome.confirmations;
                residual_risk = outcome.protection.residual_risk;
                blocking_reasons.extend(outcome.blocking_reasons);
            }
        }
    }

    let report = FundingArbExitCycleReport {
        pair_id: state.pair_id,
        symbol: state.symbol,
        decision,
        reason_codes,
        runtime_risk,
        financial_risk,
        partial_close,
        requested_close_quantity,
        funding_settlement_status: settlement.status,
        private_position_status: position_status.status,
        dispatch_attempted,
        submitted_receipt_count: receipts.len(),
        private_confirmation_count: confirmations.len(),
        residual_risk,
        blocking_reasons,
        output_dir: Some(output_dir.to_path_buf()),
    };
    write_funding_arb_exit_cycle_artifacts(output_dir, &report, &receipts, &confirmations)?;
    Ok(report)
}
