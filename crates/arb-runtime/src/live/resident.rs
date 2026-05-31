#![allow(clippy::wildcard_imports)]

#[cfg(feature = "live-exec")]
use crate::live::funding_exit::run_funding_arb_resident_exit_cycle;
use crate::*;

/// 运行 Binance basis 常驻实盘状态机。
pub(crate) fn run_binance_basis_resident_live_impl(
    options: BinanceBasisResidentLiveOptions,
) -> RuntimeResult<BinanceBasisResidentLiveReport> {
    #[cfg(feature = "live-exec")]
    {
        run_binance_basis_resident_live_inner(options)
    }
    #[cfg(not(feature = "live-exec"))]
    {
        let _ = options;
        Err(RuntimeError::UnsafeConfig {
            message: "当前 arb-runtime 未使用 live-exec feature 构建，拒绝 Binance basis 常驻实盘"
                .to_owned(),
        })
    }
}

/// 运行 Binance basis 实盘 stack supervisor。
pub(crate) fn run_binance_basis_live_stack_impl(
    options: BinanceBasisLiveStackOptions,
) -> RuntimeResult<BinanceBasisLiveStackReport> {
    #[cfg(feature = "live-exec")]
    {
        run_binance_basis_live_stack_inner(options)
    }
    #[cfg(not(feature = "live-exec"))]
    {
        let _ = options;
        Err(RuntimeError::UnsafeConfig {
            message:
                "当前 arb-runtime 未使用 live-exec feature 构建，拒绝启动 Binance basis 实盘 stack"
                    .to_owned(),
        })
    }
}

/// 运行多交易所 basis 常驻实盘状态机。
pub(crate) fn run_multi_venue_basis_resident_live_impl(
    options: MultiVenueBasisResidentLiveOptions,
) -> RuntimeResult<MultiVenueBasisResidentLiveReport> {
    #[cfg(feature = "live-exec")]
    {
        run_multi_venue_basis_resident_live_inner(options)
    }
    #[cfg(not(feature = "live-exec"))]
    {
        let _ = options;
        Err(RuntimeError::UnsafeConfig {
            message: "当前 arb-runtime 未使用 live-exec feature 构建，拒绝多交易所 basis 常驻实盘"
                .to_owned(),
        })
    }
}

/// 运行多交易所 basis live stack supervisor。
pub(crate) fn run_multi_venue_basis_live_stack_impl(
    options: MultiVenueBasisLiveStackOptions,
) -> RuntimeResult<MultiVenueBasisLiveStackReport> {
    #[cfg(feature = "live-exec")]
    {
        run_multi_venue_basis_live_stack_inner(options)
    }
    #[cfg(not(feature = "live-exec"))]
    {
        let _ = options;
        Err(RuntimeError::UnsafeConfig {
            message:
                "当前 arb-runtime 未使用 live-exec feature 构建，拒绝启动多交易所 basis live stack"
                    .to_owned(),
        })
    }
}

/// 运行 funding arb 常驻 guarded live。
pub(crate) fn run_funding_arb_resident_live_impl(
    options: FundingArbResidentLiveOptions,
) -> RuntimeResult<FundingArbResidentLiveReport> {
    #[cfg(feature = "live-exec")]
    {
        run_funding_arb_resident_live_inner(options)
    }
    #[cfg(not(feature = "live-exec"))]
    {
        let _ = options;
        Err(RuntimeError::UnsafeConfig {
            message: "funding-arb resident live requires the live-exec feature".to_owned(),
        })
    }
}

#[cfg(feature = "live-exec")]
pub(crate) fn run_binance_basis_resident_live_inner(
    options: BinanceBasisResidentLiveOptions,
) -> RuntimeResult<BinanceBasisResidentLiveReport> {
    validate_binance_basis_resident_live_options(&options)?;
    let output_root = options
        .output_dir
        .clone()
        .unwrap_or_else(|| PathBuf::from(BINANCE_BASIS_RESIDENT_LIVE_DEFAULT_OUT));
    fs::create_dir_all(&output_root).map_err(|error| RuntimeError::Io {
        path: output_root.clone(),
        message: error.to_string(),
    })?;
    let _lock = acquire_binance_basis_resident_live_lock(&output_root)?;
    write_binance_basis_resident_live_config(&output_root, &options)?;

    let mut state = load_binance_basis_resident_live_state(&output_root)?
        .unwrap_or_else(|| initial_binance_basis_resident_live_state(&options));
    if let Some(position_state_path) = &options.position_state_path {
        state.phase = "running".to_owned();
        state.position_state_path = Some(position_state_path.clone());
        state.updated_at = current_utc_timestamp()?.to_string();
        ensure_initial_resident_position_registered(&output_root, position_state_path)?;
    }
    write_binance_basis_resident_live_state(&output_root, &state)?;

    let mut cycles = 0_u64;
    let mut last_net_bps = None;
    let mut entry_dispatch_attempted = false;
    let mut exit_dispatch_attempted = false;
    let mut halt_reason = None;
    loop {
        if output_root.join("STOP").exists() {
            halt_reason =
                Some("STOP file observed; resident live halted before next cycle".to_owned());
            state.phase = "halted".to_owned();
            state.updated_at = current_utc_timestamp()?.to_string();
            write_binance_basis_resident_live_state(&output_root, &state)?;
            break;
        }
        if !binance_basis_resident_cycle_allowed(cycles, options.max_cycles) {
            halt_reason = Some("max cycles reached".to_owned());
            break;
        }
        cycles += 1;

        if state.phase == "halted" {
            halt_reason.get_or_insert_with(|| "resident state is halted".to_owned());
            break;
        }

        let latest_registry = load_binance_basis_resident_position_registry(&output_root)?;
        for position in latest_registry.active_positions() {
            let cycle_dir = output_root
                .join("exit")
                .join(&position.position_id)
                .join(resident_cycle_dir_name(cycles)?);
            match run_binance_basis_resident_exit_cycle(
                &options,
                &position.position_state_path,
                &cycle_dir,
            ) {
                Ok(report) => {
                    append_binance_basis_resident_exit_event(
                        &output_root,
                        cycles,
                        &cycle_dir,
                        &report,
                    )?;
                    if options.execute_live && report.dispatch_attempted {
                        exit_dispatch_attempted = true;
                        if binance_basis_resident_exit_cleanly_closed(&report) {
                            append_binance_basis_resident_position_closed(
                                &output_root,
                                &position.position_id,
                                cycles,
                                &cycle_dir,
                                &report,
                            )?;
                        } else {
                            append_binance_basis_resident_position_unknown(
                                &output_root,
                                &position.position_id,
                                cycles,
                                &cycle_dir,
                                "exit dispatch attempted but close was not cleanly confirmed",
                            )?;
                            halt_reason = Some(
                                "exit dispatch produced unknown or residual state; resident live stopped"
                                    .to_owned(),
                            );
                            state.phase = "halted".to_owned();
                            state.updated_at = current_utc_timestamp()?.to_string();
                            write_binance_basis_resident_live_state(&output_root, &state)?;
                            break;
                        }
                    } else if options.execute_live
                        && report.decision != SpotPerpBasisExitDecision::Hold.as_str()
                        && !report.blocking_reasons.is_empty()
                    {
                        halt_reason = Some(
                            "exit signal was non-hold but live close was blocked; resident live stopped"
                                .to_owned(),
                        );
                        state.phase = "halted".to_owned();
                        state.updated_at = current_utc_timestamp()?.to_string();
                        write_binance_basis_resident_live_state(&output_root, &state)?;
                        break;
                    }
                }
                Err(error) => {
                    append_binance_basis_resident_error_event(
                        &output_root,
                        cycles,
                        "exit",
                        &cycle_dir,
                        &error.to_string(),
                    )?;
                    if options.execute_live {
                        halt_reason = Some(format!(
                            "exit cycle failed for {}; resident live stopped: {error}",
                            position.position_id
                        ));
                        state.phase = "halted".to_owned();
                        state.updated_at = current_utc_timestamp()?.to_string();
                        write_binance_basis_resident_live_state(&output_root, &state)?;
                        break;
                    }
                }
            }
        }
        if state.phase == "halted" {
            break;
        }

        let latest_registry = load_binance_basis_resident_position_registry(&output_root)?;
        match binance_basis_resident_entry_capacity(&options, &latest_registry)? {
            ResidentEntryCapacity::Allowed => {
                let cycle_dir = output_root
                    .join("entry")
                    .join(resident_cycle_dir_name(cycles)?);
                match run_binance_basis_resident_entry_cycle(&options, &cycle_dir) {
                    Ok(report) => {
                        last_net_bps = report.net_bps;
                        append_binance_basis_resident_entry_event(
                            &output_root,
                            cycles,
                            &cycle_dir,
                            &report,
                        )?;
                        if options.execute_live && report.dispatch_attempted {
                            entry_dispatch_attempted = true;
                            let position_state =
                                cycle_dir.join("live-dispatch/basis_exit_supervisor_state.json");
                            if binance_basis_resident_entry_cleanly_opened(&report, &position_state)
                            {
                                let position = append_binance_basis_resident_position_opened(
                                    &output_root,
                                    cycles,
                                    &cycle_dir,
                                    &position_state,
                                    &report,
                                )?;
                                state.phase = "running".to_owned();
                                state.position_state_path = Some(position.position_state_path);
                                state.updated_at = current_utc_timestamp()?.to_string();
                                write_binance_basis_resident_live_state(&output_root, &state)?;
                            } else {
                                halt_reason = Some(
                                    "entry dispatch was attempted but no clean open-position state was confirmed; resident live stopped"
                                        .to_owned(),
                                );
                                state.phase = "halted".to_owned();
                                state.updated_at = current_utc_timestamp()?.to_string();
                                write_binance_basis_resident_live_state(&output_root, &state)?;
                                break;
                            }
                        }
                    }
                    Err(error) => {
                        append_binance_basis_resident_error_event(
                            &output_root,
                            cycles,
                            "entry",
                            &cycle_dir,
                            &error.to_string(),
                        )?;
                        if options.execute_live {
                            halt_reason = Some(format!(
                                "entry cycle failed; resident live stopped: {error}"
                            ));
                            state.phase = "halted".to_owned();
                            state.updated_at = current_utc_timestamp()?.to_string();
                            write_binance_basis_resident_live_state(&output_root, &state)?;
                            break;
                        }
                    }
                }
            }
            ResidentEntryCapacity::Blocked(reason) => {
                append_binance_basis_resident_capacity_event(&output_root, cycles, &reason)?;
                if latest_registry.active_positions().is_empty() {
                    halt_reason = Some(reason);
                    break;
                }
            }
        }

        if !binance_basis_resident_cycle_allowed(cycles, options.max_cycles) {
            halt_reason = Some("max cycles reached".to_owned());
            break;
        }
        thread::sleep(Duration::from_secs(options.poll_interval_secs));
    }

    let latest_registry = load_binance_basis_resident_position_registry(&output_root)?;
    Ok(BinanceBasisResidentLiveReport {
        phase: state.phase,
        cycles,
        last_net_bps,
        entry_dispatch_attempted,
        exit_dispatch_attempted,
        position_state_path: state.position_state_path,
        open_position_count: latest_registry.active_positions().len(),
        total_open_notional_usdt: latest_registry.total_active_notional()?.to_string(),
        live_entry_count: latest_registry.live_entry_count(),
        halt_reason,
        output_dir: Some(output_root),
    })
}

#[cfg(feature = "live-exec")]
pub(crate) fn run_multi_venue_basis_resident_live_inner(
    options: MultiVenueBasisResidentLiveOptions,
) -> RuntimeResult<MultiVenueBasisResidentLiveReport> {
    validate_multi_venue_basis_resident_live_options(&options)?;
    let output_root = options
        .output_dir
        .clone()
        .unwrap_or_else(|| PathBuf::from(MULTI_VENUE_BASIS_RESIDENT_LIVE_DEFAULT_OUT));
    fs::create_dir_all(&output_root).map_err(|error| RuntimeError::Io {
        path: output_root.clone(),
        message: error.to_string(),
    })?;
    let _lock = acquire_multi_venue_basis_resident_live_lock(&output_root)?;
    write_multi_venue_basis_resident_live_config(&output_root, &options)?;
    write_multi_venue_basis_resident_live_state(&output_root, "running", 0, None)?;

    let mut cycles = 0_u64;
    let mut entry_dispatch_attempted = false;
    let mut exit_dispatch_attempted = false;
    let mut halt_reason = None;
    let mut retryable_entry_error_streaks: BTreeMap<BasisLiveVenue, u32> = BTreeMap::new();
    loop {
        if output_root.join("STOP").exists() {
            halt_reason = Some("STOP file observed; multi-venue resident live halted".to_owned());
            write_multi_venue_basis_resident_live_state(
                &output_root,
                "halted",
                cycles,
                halt_reason.as_deref(),
            )?;
            break;
        }
        if !binance_basis_resident_cycle_allowed(cycles, options.max_cycles) {
            halt_reason = Some("max cycles reached".to_owned());
            break;
        }
        cycles += 1;
        append_multi_venue_basis_resident_event(
            &output_root,
            "cycle_started",
            &format!("\"cycle\":{}", cycles),
        )?;

        for venue in &options.venues {
            let venue_dir = multi_venue_basis_resident_venue_dir(&output_root, *venue);
            fs::create_dir_all(&venue_dir).map_err(|error| RuntimeError::Io {
                path: venue_dir.clone(),
                message: error.to_string(),
            })?;
            if venue_dir.join("STOP").exists() {
                append_multi_venue_basis_resident_event(
                    &output_root,
                    "venue_stop_observed",
                    &format!(
                        "\"cycle\":{},\"venue\":{}",
                        cycles,
                        json_string(venue.as_str())
                    ),
                )?;
                continue;
            }

            let registry = load_binance_basis_resident_position_registry(&venue_dir)?;
            for position in registry.active_positions() {
                let cycle_dir = venue_dir
                    .join("exit")
                    .join(&position.position_id)
                    .join(resident_cycle_dir_name(cycles)?);
                match run_multi_venue_basis_resident_exit_cycle(
                    &options,
                    *venue,
                    &position.position_state_path,
                    &cycle_dir,
                ) {
                    Ok(report) => {
                        append_binance_basis_resident_exit_event(
                            &venue_dir, cycles, &cycle_dir, &report,
                        )?;
                        append_multi_venue_basis_resident_event(
                            &output_root,
                            "exit_cycle",
                            &format!(
                                "\"blocking_reasons\":{},\"cycle\":{},\"decision\":{},\"dispatch_attempted\":{},\"private_confirmation_count\":{},\"residual_risk\":{},\"venue\":{}",
                                report.blocking_reasons.len(),
                                cycles,
                                json_string(&report.decision),
                                report.dispatch_attempted,
                                report.private_confirmation_count,
                                optional_json_string(report.residual_risk.as_deref()),
                                json_string(venue.as_str())
                            ),
                        )?;
                        if options.execute_live && report.dispatch_attempted {
                            exit_dispatch_attempted = true;
                            if binance_basis_resident_exit_cleanly_closed(&report) {
                                append_binance_basis_resident_position_closed(
                                    &venue_dir,
                                    &position.position_id,
                                    cycles,
                                    &cycle_dir,
                                    &report,
                                )?;
                            } else {
                                append_binance_basis_resident_position_unknown(
                                    &venue_dir,
                                    &position.position_id,
                                    cycles,
                                    &cycle_dir,
                                    "exit dispatch attempted but close was not cleanly confirmed",
                                )?;
                                halt_reason = Some(format!(
                                    "{} exit dispatch produced unknown or residual state; multi-venue resident live stopped",
                                    venue.label()
                                ));
                                write_multi_venue_basis_resident_live_state(
                                    &output_root,
                                    "halted",
                                    cycles,
                                    halt_reason.as_deref(),
                                )?;
                                break;
                            }
                        } else if options.execute_live
                            && report.decision != SpotPerpBasisExitDecision::Hold.as_str()
                            && !report.blocking_reasons.is_empty()
                        {
                            halt_reason = Some(format!(
                                "{} exit signal was non-hold but live close was blocked; multi-venue resident live stopped",
                                venue.label()
                            ));
                            write_multi_venue_basis_resident_live_state(
                                &output_root,
                                "halted",
                                cycles,
                                halt_reason.as_deref(),
                            )?;
                            break;
                        }
                    }
                    Err(error) => {
                        append_binance_basis_resident_error_event(
                            &venue_dir,
                            cycles,
                            "exit",
                            &cycle_dir,
                            &error.to_string(),
                        )?;
                        append_multi_venue_basis_resident_error_event(
                            &output_root,
                            cycles,
                            *venue,
                            "exit",
                            &cycle_dir,
                            &error.to_string(),
                        )?;
                        if options.execute_live {
                            halt_reason = Some(format!(
                                "{} exit cycle failed; multi-venue resident live stopped: {error}",
                                venue.label()
                            ));
                            write_multi_venue_basis_resident_live_state(
                                &output_root,
                                "halted",
                                cycles,
                                halt_reason.as_deref(),
                            )?;
                            break;
                        }
                    }
                }
            }
            if halt_reason.is_some() {
                break;
            }
        }
        if halt_reason.is_some() {
            break;
        }

        for venue in &options.venues {
            let venue_dir = multi_venue_basis_resident_venue_dir(&output_root, *venue);
            if venue_dir.join("STOP").exists() {
                continue;
            }
            match multi_venue_basis_resident_entry_capacity(&options, &output_root)? {
                ResidentEntryCapacity::Allowed => {
                    let cycle_dir = venue_dir
                        .join("entry")
                        .join(resident_cycle_dir_name(cycles)?);
                    match run_multi_venue_basis_resident_entry_cycle(&options, *venue, &cycle_dir) {
                        Ok(report) => {
                            retryable_entry_error_streaks.remove(venue);
                            append_binance_basis_resident_entry_event(
                                &venue_dir, cycles, &cycle_dir, &report,
                            )?;
                            append_multi_venue_basis_resident_entry_event(
                                &output_root,
                                cycles,
                                *venue,
                                &cycle_dir,
                                &report,
                            )?;
                            if options.execute_live && report.dispatch_attempted {
                                entry_dispatch_attempted = true;
                                let position_state = cycle_dir
                                    .join("live-dispatch/basis_exit_supervisor_state.json");
                                if binance_basis_resident_entry_cleanly_opened(
                                    &report,
                                    &position_state,
                                ) {
                                    append_binance_basis_resident_position_opened(
                                        &venue_dir,
                                        cycles,
                                        &cycle_dir,
                                        &position_state,
                                        &report,
                                    )?;
                                } else {
                                    halt_reason = Some(format!(
                                        "{} entry dispatch was attempted but no clean open-position state was confirmed; multi-venue resident live stopped",
                                        venue.label()
                                    ));
                                    write_multi_venue_basis_resident_live_state(
                                        &output_root,
                                        "halted",
                                        cycles,
                                        halt_reason.as_deref(),
                                    )?;
                                    break;
                                }
                            }
                        }
                        Err(error) => {
                            append_binance_basis_resident_error_event(
                                &venue_dir,
                                cycles,
                                "entry",
                                &cycle_dir,
                                &error.to_string(),
                            )?;
                            append_multi_venue_basis_resident_error_event(
                                &output_root,
                                cycles,
                                *venue,
                                "entry",
                                &cycle_dir,
                                &error.to_string(),
                            )?;
                            let retryable = resident_entry_cycle_error_can_retry(&error);
                            if retryable {
                                let streak = {
                                    let entry =
                                        retryable_entry_error_streaks.entry(*venue).or_insert(0);
                                    *entry = entry.saturating_add(1);
                                    *entry
                                };
                                let backoff = resident_retryable_error_backoff(streak);
                                append_multi_venue_basis_resident_event(
                                    &output_root,
                                    "entry_retry_backoff",
                                    &format!(
                                        "\"cycle\":{},\"reason_class\":{},\"retryable_error_streak\":{},\"sleep_ms\":{},\"venue\":{}",
                                        cycles,
                                        json_string(resident_retryable_error_class(&error)),
                                        streak,
                                        backoff.as_millis(),
                                        json_string(venue.as_str())
                                    ),
                                )?;
                                thread::sleep(backoff);
                            } else {
                                retryable_entry_error_streaks.remove(venue);
                            }
                            if options.execute_live && !retryable {
                                if resident_entry_cycle_error_can_isolate_venue(&error) {
                                    let reason = format!(
                                        "{} entry cycle isolated venue after non-retryable configuration error: {error}",
                                        venue.label()
                                    );
                                    write_utf8(venue_dir.join("STOP"), &format!("{reason}\n"))?;
                                    append_binance_basis_resident_jsonl(
                                        &venue_dir,
                                        &format!(
                                            "{{\"cycle\":{},\"cycle_dir\":{},\"error\":{},\"event_type\":\"venue_isolated\",\"phase\":\"entry\",\"reason\":{},\"status\":\"stopped\"}}",
                                            cycles,
                                            json_string(&cycle_dir.display().to_string()),
                                            json_string(&error.to_string()),
                                            json_string(&reason)
                                        ),
                                    )?;
                                    append_multi_venue_basis_resident_event(
                                        &output_root,
                                        "venue_isolated",
                                        &format!(
                                            "\"cycle\":{},\"error\":{},\"reason\":{},\"venue\":{}",
                                            cycles,
                                            json_string(&error.to_string()),
                                            json_string(&reason),
                                            json_string(venue.as_str())
                                        ),
                                    )?;
                                } else {
                                    halt_reason = Some(format!(
                                        "{} entry cycle failed; multi-venue resident live stopped: {error}",
                                        venue.label()
                                    ));
                                    write_multi_venue_basis_resident_live_state(
                                        &output_root,
                                        "halted",
                                        cycles,
                                        halt_reason.as_deref(),
                                    )?;
                                    break;
                                }
                            }
                        }
                    }
                }
                ResidentEntryCapacity::Blocked(reason) => {
                    retryable_entry_error_streaks.remove(venue);
                    append_multi_venue_basis_resident_event(
                        &output_root,
                        "entry_capacity_blocked",
                        &format!(
                            "\"cycle\":{},\"reason\":{},\"venue\":{}",
                            cycles,
                            json_string(&reason),
                            json_string(venue.as_str())
                        ),
                    )?;
                    let summary =
                        multi_venue_basis_resident_registry_summary(&options, &output_root)?;
                    if summary.active_position_count == 0 {
                        halt_reason = Some(reason);
                        break;
                    }
                }
            }
            if halt_reason.is_some() {
                break;
            }
        }

        if halt_reason.is_some() {
            break;
        }
        write_multi_venue_basis_resident_live_state(&output_root, "running", cycles, None)?;
        if !binance_basis_resident_cycle_allowed(cycles, options.max_cycles) {
            halt_reason = Some("max cycles reached".to_owned());
            break;
        }
        thread::sleep(Duration::from_secs(options.poll_interval_secs));
    }

    let summary = multi_venue_basis_resident_registry_summary(&options, &output_root)?;
    let phase = if halt_reason.is_some() {
        "halted"
    } else {
        "completed"
    };
    write_multi_venue_basis_resident_live_state(
        &output_root,
        phase,
        cycles,
        halt_reason.as_deref(),
    )?;
    let report = MultiVenueBasisResidentLiveReport {
        phase: phase.to_owned(),
        cycles,
        venue_count: options.venues.len(),
        entry_dispatch_attempted,
        exit_dispatch_attempted,
        open_position_count: summary.active_position_count,
        total_open_notional_usdt: summary.total_active_notional_usdt.to_string(),
        live_entry_count: summary.live_entry_count,
        halt_reason,
        output_dir: Some(output_root.clone()),
    };
    write_multi_venue_basis_resident_live_summary(&output_root, &report)?;
    Ok(report)
}

#[cfg(feature = "live-exec")]
pub(crate) fn run_binance_basis_live_stack_inner(
    options: BinanceBasisLiveStackOptions,
) -> RuntimeResult<BinanceBasisLiveStackReport> {
    validate_binance_basis_live_stack_options(&options)?;
    let output_root = options
        .output_dir
        .clone()
        .unwrap_or_else(|| PathBuf::from(BINANCE_BASIS_LIVE_STACK_DEFAULT_OUT));
    fs::create_dir_all(&output_root).map_err(|error| RuntimeError::Io {
        path: output_root.clone(),
        message: error.to_string(),
    })?;
    let logs_dir = output_root.join("logs");
    fs::create_dir_all(&logs_dir).map_err(|error| RuntimeError::Io {
        path: logs_dir.clone(),
        message: error.to_string(),
    })?;
    let resident_dir = output_root.join("resident");
    let private_order_events_dir = options
        .private_order_events_dir
        .clone()
        .unwrap_or_else(|| output_root.join("private-order-events"));
    let adl_events_dir = options
        .adl_events_dir
        .clone()
        .unwrap_or_else(|| output_root.join("adl-events"));
    fs::create_dir_all(&private_order_events_dir).map_err(|error| RuntimeError::Io {
        path: private_order_events_dir.clone(),
        message: error.to_string(),
    })?;
    fs::create_dir_all(&adl_events_dir).map_err(|error| RuntimeError::Io {
        path: adl_events_dir.clone(),
        message: error.to_string(),
    })?;
    write_binance_basis_live_stack_config(
        &output_root,
        &options,
        &resident_dir,
        &private_order_events_dir,
        &adl_events_dir,
    )?;

    let mut children = BinanceBasisLiveStackChildren::default();
    if !options.use_existing_monitors {
        let monitor_symbol = binance_basis_live_stack_monitor_symbol(&options);
        let spot_args = binance_basis_live_stack_monitor_args(
            &options.spot_wss_bind_addr,
            &monitor_symbol,
            "spot",
            options.monitor_reconnect_delay_secs,
        );
        let spot_child =
            spawn_binance_basis_live_stack_child("spot_wss_monitor", &spot_args, &logs_dir)?;
        append_binance_basis_live_stack_event(
            &output_root,
            "child_started",
            &format!(
                "\"role\":\"spot_wss_monitor\",\"pid\":{},\"stdout\":{},\"stderr\":{}",
                spot_child.pid(),
                json_string(&spot_child.stdout_log.display().to_string()),
                json_string(&spot_child.stderr_log.display().to_string())
            ),
        )?;
        children.push(spot_child);

        let perp_args = binance_basis_live_stack_monitor_args(
            &options.perp_wss_bind_addr,
            &monitor_symbol,
            "usdm-perp",
            options.monitor_reconnect_delay_secs,
        );
        let perp_child =
            spawn_binance_basis_live_stack_child("perp_wss_monitor", &perp_args, &logs_dir)?;
        append_binance_basis_live_stack_event(
            &output_root,
            "child_started",
            &format!(
                "\"role\":\"perp_wss_monitor\",\"pid\":{},\"stdout\":{},\"stderr\":{}",
                perp_child.pid(),
                json_string(&perp_child.stdout_log.display().to_string()),
                json_string(&perp_child.stderr_log.display().to_string())
            ),
        )?;
        children.push(perp_child);
    } else {
        append_binance_basis_live_stack_event(
            &output_root,
            "using_existing_monitors",
            "\"detail\":\"monitor child processes were not spawned\"",
        )?;
    }

    wait_for_binance_basis_live_stack_monitors_ready(&options, &output_root, &mut children)?;
    let resident_args = binance_basis_live_stack_resident_args(
        &options,
        &resident_dir,
        &private_order_events_dir,
        &adl_events_dir,
    );
    let resident_child =
        spawn_binance_basis_live_stack_child("resident_runner", &resident_args, &logs_dir)?;
    append_binance_basis_live_stack_event(
        &output_root,
        "child_started",
        &format!(
            "\"role\":\"resident_runner\",\"pid\":{},\"stdout\":{},\"stderr\":{}",
            resident_child.pid(),
            json_string(&resident_child.stdout_log.display().to_string()),
            json_string(&resident_child.stderr_log.display().to_string())
        ),
    )?;
    children.push(resident_child);

    let mut halt_reason = None;
    loop {
        if let Some(status) = children.try_wait_role("resident_runner")? {
            append_binance_basis_live_stack_event(
                &output_root,
                "resident_exited",
                &format!(
                    "\"exit_status\":{}",
                    json_string(&binance_basis_live_stack_status_string(&status))
                ),
            )?;
            if !status.success() {
                halt_reason = Some(format!(
                    "resident runner exited unsuccessfully: {}",
                    binance_basis_live_stack_status_string(&status)
                ));
            }
            break;
        }
        if let Some((role, status)) = children.first_exited_role_except("resident_runner")? {
            let status = binance_basis_live_stack_status_string(&status);
            let reason = format!("{role} exited while resident runner was active: {status}");
            append_binance_basis_live_stack_event(
                &output_root,
                "monitor_exited",
                &format!(
                    "\"role\":{},\"exit_status\":{},\"resident_stop_file\":{}",
                    json_string(role),
                    json_string(&status),
                    json_string(&resident_dir.join("STOP").display().to_string())
                ),
            )?;
            let _ = write_utf8(
                resident_dir.join("STOP"),
                &format!("created_by=binance-basis-live-stack\nreason={reason}\n"),
            );
            thread::sleep(Duration::from_secs(options.shutdown_grace_secs));
            halt_reason = Some(reason);
            break;
        }
        thread::sleep(Duration::from_secs(1));
    }

    children.shutdown_all()?;
    let phase = if halt_reason.is_some() {
        "halted"
    } else {
        "completed"
    };
    let report = BinanceBasisLiveStackReport {
        phase: phase.to_owned(),
        output_dir: output_root.clone(),
        resident_exit_status: children.status_for("resident_runner"),
        spot_monitor_exit_status: children.status_for("spot_wss_monitor"),
        perp_monitor_exit_status: children.status_for("perp_wss_monitor"),
        readiness_ok: true,
        halt_reason,
    };
    write_binance_basis_live_stack_summary(&output_root, &report)?;
    Ok(report)
}

#[cfg(feature = "live-exec")]
pub(crate) fn run_multi_venue_basis_live_stack_inner(
    options: MultiVenueBasisLiveStackOptions,
) -> RuntimeResult<MultiVenueBasisLiveStackReport> {
    validate_multi_venue_basis_live_stack_options(&options)?;
    let output_root = options
        .output_dir
        .clone()
        .unwrap_or_else(|| PathBuf::from(MULTI_VENUE_BASIS_LIVE_STACK_DEFAULT_OUT));
    fs::create_dir_all(&output_root).map_err(|error| RuntimeError::Io {
        path: output_root.clone(),
        message: error.to_string(),
    })?;
    let logs_dir = output_root.join("logs");
    fs::create_dir_all(&logs_dir).map_err(|error| RuntimeError::Io {
        path: logs_dir.clone(),
        message: error.to_string(),
    })?;
    let resident_dir = output_root.join("resident");
    let private_order_events_dir = options
        .private_order_events_dir
        .clone()
        .unwrap_or_else(|| output_root.join("private-order-events"));
    let adl_events_dir = options
        .adl_events_dir
        .clone()
        .unwrap_or_else(|| output_root.join("adl-events"));
    fs::create_dir_all(&private_order_events_dir).map_err(|error| RuntimeError::Io {
        path: private_order_events_dir.clone(),
        message: error.to_string(),
    })?;
    fs::create_dir_all(&adl_events_dir).map_err(|error| RuntimeError::Io {
        path: adl_events_dir.clone(),
        message: error.to_string(),
    })?;
    write_multi_venue_basis_live_stack_config(
        &output_root,
        &options,
        &resident_dir,
        &private_order_events_dir,
        &adl_events_dir,
    )?;

    let mut children = BinanceBasisLiveStackChildren::default();
    if !options.use_existing_monitors {
        for venue in &options.venues {
            if !multi_venue_basis_live_stack_managed_wss_supported(*venue) {
                append_multi_venue_basis_live_stack_event(
                    &output_root,
                    "managed_monitor_skipped",
                    &format!(
                        "\"reason\":\"managed WSS monitor is not attached for this venue\",\"venue\":{}",
                        json_string(venue.as_str())
                    ),
                )?;
                continue;
            }
            let monitor_symbol = multi_venue_basis_live_stack_monitor_symbol(*venue, &options);
            let spot_args = multi_venue_basis_live_stack_monitor_args(
                *venue,
                multi_venue_basis_live_stack_spot_bind(&options, *venue),
                &monitor_symbol,
                "spot",
                options.monitor_reconnect_delay_secs,
            )?;
            let spot_role = multi_venue_basis_live_stack_child_role(*venue, "spot_wss_monitor");
            let spot_child =
                spawn_binance_basis_live_stack_child(spot_role, &spot_args, &logs_dir)?;
            append_multi_venue_basis_live_stack_child_started(
                &output_root,
                spot_role,
                &spot_child,
            )?;
            children.push(spot_child);

            let perp_args = multi_venue_basis_live_stack_monitor_args(
                *venue,
                multi_venue_basis_live_stack_perp_bind(&options, *venue),
                &monitor_symbol,
                "perp",
                options.monitor_reconnect_delay_secs,
            )?;
            let perp_role = multi_venue_basis_live_stack_child_role(*venue, "perp_wss_monitor");
            let perp_child =
                spawn_binance_basis_live_stack_child(perp_role, &perp_args, &logs_dir)?;
            append_multi_venue_basis_live_stack_child_started(
                &output_root,
                perp_role,
                &perp_child,
            )?;
            children.push(perp_child);
        }
    } else {
        append_multi_venue_basis_live_stack_event(
            &output_root,
            "using_existing_monitors",
            "\"detail\":\"monitor child processes were not spawned\"",
        )?;
    }

    if let Err(error) =
        wait_for_multi_venue_basis_live_stack_monitors_ready(&options, &output_root, &mut children)
    {
        let _ = children.shutdown_all();
        return Err(error);
    }
    let resident_args = multi_venue_basis_live_stack_resident_args(
        &options,
        &resident_dir,
        &private_order_events_dir,
        &adl_events_dir,
    );
    let resident_child = spawn_binance_basis_live_stack_child(
        "multi_venue_resident_runner",
        &resident_args,
        &logs_dir,
    )?;
    append_multi_venue_basis_live_stack_child_started(
        &output_root,
        "multi_venue_resident_runner",
        &resident_child,
    )?;
    children.push(resident_child);

    let mut halt_reason = None;
    loop {
        if let Some(status) = children.try_wait_role("multi_venue_resident_runner")? {
            append_multi_venue_basis_live_stack_event(
                &output_root,
                "resident_exited",
                &format!(
                    "\"exit_status\":{}",
                    json_string(&binance_basis_live_stack_status_string(&status))
                ),
            )?;
            if !status.success() {
                halt_reason = Some(format!(
                    "multi-venue resident runner exited unsuccessfully: {}",
                    binance_basis_live_stack_status_string(&status)
                ));
            }
            break;
        }
        if let Some((role, status)) =
            children.first_exited_role_except("multi_venue_resident_runner")?
        {
            let status = binance_basis_live_stack_status_string(&status);
            let reason =
                format!("{role} exited while multi-venue resident runner was active: {status}");
            append_multi_venue_basis_live_stack_event(
                &output_root,
                "monitor_exited",
                &format!(
                    "\"exit_status\":{},\"resident_stop_file\":{},\"role\":{}",
                    json_string(&status),
                    json_string(&resident_dir.join("STOP").display().to_string()),
                    json_string(role)
                ),
            )?;
            let _ = write_utf8(
                resident_dir.join("STOP"),
                &format!("created_by=multi-venue-basis-live-stack\nreason={reason}\n"),
            );
            thread::sleep(Duration::from_secs(options.shutdown_grace_secs));
            halt_reason = Some(reason);
            break;
        }
        thread::sleep(Duration::from_secs(1));
    }

    children.shutdown_all()?;
    let phase = if halt_reason.is_some() {
        "halted"
    } else {
        "completed"
    };
    let report = MultiVenueBasisLiveStackReport {
        phase: phase.to_owned(),
        output_dir: output_root.clone(),
        resident_exit_status: children.status_for("multi_venue_resident_runner"),
        monitor_exit_statuses: multi_venue_basis_live_stack_monitor_statuses(&children),
        readiness_ok: true,
        halt_reason,
    };
    write_multi_venue_basis_live_stack_summary(&output_root, &report)?;
    Ok(report)
}

#[cfg(feature = "live-exec")]
pub(crate) fn run_funding_arb_resident_live_inner(
    options: FundingArbResidentLiveOptions,
) -> RuntimeResult<FundingArbResidentLiveReport> {
    validate_funding_arb_resident_live_options(&options)?;
    let output_root = options
        .output_dir
        .clone()
        .unwrap_or_else(|| PathBuf::from(FUNDING_ARB_RESIDENT_LIVE_DEFAULT_OUT));
    fs::create_dir_all(&output_root).map_err(|error| RuntimeError::Io {
        path: output_root.clone(),
        message: error.to_string(),
    })?;
    let _lock = acquire_funding_arb_resident_live_lock(&output_root)?;
    write_funding_arb_resident_live_config(&output_root, &options)?;
    write_funding_arb_resident_live_state(&output_root, "running", 0, None)?;
    append_funding_arb_resident_started_event(&output_root)?;

    let mut cycles = 0_u64;
    let mut last_pair_id = None;
    let mut last_symbol = None;
    let mut last_net_funding_bps = None;
    let mut dispatch_attempted = false;
    let mut halt_reason = None;
    let mut force_residual_de_risk_cycle = false;
    let mut position_recovery_drain_mode = false;
    write_funding_arb_resident_live_progress_summary(
        &output_root,
        FundingArbResidentLiveProgressInput {
            phase: "running",
            cycles,
            last_pair_id: &last_pair_id,
            last_symbol: &last_symbol,
            last_net_funding_bps,
            dispatch_attempted,
            halt_reason: None,
        },
    )?;
    loop {
        if output_root.join("STOP").exists() {
            halt_reason = Some(
                "STOP file observed; funding arb resident live halted before next cycle".to_owned(),
            );
            break;
        }
        if !force_residual_de_risk_cycle
            && !binance_basis_resident_cycle_allowed(cycles, options.max_cycles)
        {
            halt_reason = Some("max cycles reached".to_owned());
            break;
        }

        cycles += 1;
        force_residual_de_risk_cycle = false;
        write_funding_arb_resident_live_state(&output_root, "running", cycles, None)?;
        let cycle_dir = output_root
            .join("cycles")
            .join(resident_cycle_dir_name(cycles)?);

        if options.execute_live {
            let unknown_recovery_enabled = funding_arb_unknown_recovery_enabled(&options);
            let recovered_unknown_positions = if unknown_recovery_enabled {
                recover_funding_arb_unknown_positions_for_exit(
                    &options,
                    &output_root,
                    cycles,
                    &cycle_dir,
                )?
            } else {
                let unknowns = load_funding_arb_unknown_position_recovery_records(&output_root)?;
                if !unknowns.is_empty() {
                    halt_reason = Some(format!(
                        "{} unresolved funding arb unknown position(s) exist; startup recovery requires --allow-unknown-recovery or auto residual de-risk",
                        unknowns.len()
                    ));
                    break;
                }
                0
            };
            let recovered_flat_cancelled_orphan_positions = if unknown_recovery_enabled {
                recover_funding_arb_flat_cancelled_orphan_positions_for_exit(
                    &options,
                    &output_root,
                    cycles,
                    &cycle_dir,
                )?
            } else {
                let orphans =
                    load_funding_arb_flat_cancelled_orphan_recovery_records(&output_root)?;
                if !orphans.is_empty() {
                    halt_reason = Some(format!(
                        "{} unresolved flat-cancelled funding arb position candidate(s) exist; orphan recovery requires --allow-unknown-recovery or auto residual de-risk",
                        orphans.len()
                    ));
                    break;
                }
                0
            };
            let recovered_positions =
                recovered_unknown_positions + recovered_flat_cancelled_orphan_positions;
            if recovered_positions > 0 {
                position_recovery_drain_mode = true;
            }
            let registry = load_funding_arb_resident_position_registry(&output_root)?;
            let active_positions = registry.active_positions();
            let mut residual_de_risk_pending = false;
            for position in active_positions {
                let exit_dir = output_root
                    .join("exit")
                    .join(&position.position_id)
                    .join(resident_cycle_dir_name(cycles)?);
                match run_funding_arb_resident_exit_cycle(
                    &options,
                    &position.position_state_path,
                    &exit_dir,
                ) {
                    Ok(report) => {
                        append_funding_arb_resident_exit_event(
                            &output_root,
                            cycles,
                            &position.position_id,
                            &exit_dir,
                            &report,
                        )?;
                        if report.dispatch_attempted {
                            dispatch_attempted = true;
                        }
                        if funding_arb_exit_cycle_cleanly_closed(&report) {
                            append_funding_arb_resident_position_closed(
                                &output_root,
                                &position,
                                cycles,
                                &exit_dir,
                                &report,
                            )?;
                        } else if report.dispatch_attempted
                            && (report.residual_risk.is_some()
                                || !report.blocking_reasons.is_empty())
                        {
                            append_funding_arb_resident_position_unknown(
                                &output_root,
                                &position,
                                cycles,
                                &exit_dir,
                                &report,
                                "funding arb exit/de-risk dispatch left unknown or residual state",
                            )?;
                            if options.auto_residual_de_risk {
                                append_funding_arb_resident_residual_retry_event(
                                    &output_root,
                                    cycles,
                                    &exit_dir,
                                    "funding arb residual state detected; auto residual de-risk will retry from private position snapshot before new entries",
                                )?;
                                residual_de_risk_pending = true;
                            } else {
                                halt_reason = Some(
                                    "funding arb exit/de-risk dispatch left unknown or residual state; resident live stopped"
                                        .to_owned(),
                                );
                            }
                            break;
                        }
                    }
                    Err(error) => {
                        append_funding_arb_resident_error_event(
                            &output_root,
                            cycles,
                            &exit_dir,
                            &format!("exit cycle failed for {}: {error}", position.position_id),
                        )?;
                        halt_reason = Some(format!(
                            "funding arb exit cycle failed for {}; resident live stopped: {error}",
                            position.position_id
                        ));
                        break;
                    }
                }
            }
            if halt_reason.is_some() {
                break;
            }
            if residual_de_risk_pending {
                force_residual_de_risk_cycle = true;
                write_funding_arb_resident_live_progress_summary(
                    &output_root,
                    FundingArbResidentLiveProgressInput {
                        phase: "running",
                        cycles,
                        last_pair_id: &last_pair_id,
                        last_symbol: &last_symbol,
                        last_net_funding_bps,
                        dispatch_attempted,
                        halt_reason: None,
                    },
                )?;
                thread::sleep(Duration::from_secs(options.poll_interval_secs));
                continue;
            }
            if position_recovery_drain_mode {
                let registry = load_funding_arb_resident_position_registry(&output_root)?;
                if funding_arb_position_recovery_drain_is_complete(&registry) {
                    halt_reason = Some(
                        "funding arb position recovery cycle completed; resident live stopped before new entries"
                            .to_owned(),
                    );
                    break;
                }
                write_funding_arb_resident_live_progress_summary(
                    &output_root,
                    FundingArbResidentLiveProgressInput {
                        phase: "running",
                        cycles,
                        last_pair_id: &last_pair_id,
                        last_symbol: &last_symbol,
                        last_net_funding_bps,
                        dispatch_attempted,
                        halt_reason: None,
                    },
                )?;
                thread::sleep(Duration::from_secs(options.poll_interval_secs));
                continue;
            }
            if options.exit_only {
                halt_reason = Some(
                    "funding arb exit-only cycle completed; resident live stopped before new entries"
                        .to_owned(),
                );
                break;
            }
        }

        match funding_arb_resident_entry_capacity(&options, &output_root)? {
            ResidentEntryCapacity::Allowed => {
                match run_funding_arb_resident_cycle(&options, &cycle_dir) {
                    Ok(FundingArbResidentCycleResult::Candidate(outcome)) => {
                        last_pair_id = Some(outcome.pair_id.clone());
                        last_symbol = Some(outcome.symbol.clone());
                        last_net_funding_bps = outcome.net_funding_bps;
                        append_funding_arb_resident_candidate_event(
                            &output_root,
                            cycles,
                            &cycle_dir,
                            &outcome,
                        )?;
                        if outcome.canary.dispatch_attempted {
                            dispatch_attempted = true;
                            if options.execute_live {
                                if let Some(position_state_path) = &outcome.position_state_path {
                                    append_funding_arb_resident_position_opened(
                                        &output_root,
                                        cycles,
                                        &cycle_dir,
                                        position_state_path,
                                        &outcome,
                                    )?;
                                } else if outcome.canary.flat_after_cancel {
                                    append_funding_arb_resident_entry_flat_cancelled(
                                        &output_root,
                                        cycles,
                                        &cycle_dir,
                                        &outcome,
                                    )?;
                                } else if funding_arb_entry_dispatch_missing_position_state_requires_halt(
                                    outcome.canary.submitted_receipt_count,
                                    outcome.canary.private_confirmation_count,
                                    outcome.canary.residual_risk.as_deref(),
                                    !outcome.canary.blocking_reasons.is_empty(),
                                ) {
                                    append_funding_arb_resident_entry_position_unknown(
                                        &output_root,
                                        cycles,
                                        &cycle_dir,
                                        &outcome,
                                        "funding arb entry dispatch left unknown or residual state before a durable position state was written",
                                    )?;
                                    halt_reason = Some(
                                        "funding arb entry dispatch attempted but no durable position state was written; resident live stopped"
                                            .to_owned(),
                                    );
                                    break;
                                } else if funding_arb_entry_dispatch_rejection_requires_halt(
                                    outcome.canary.mutable_execution_started,
                                    !outcome.canary.blocking_reasons.is_empty(),
                                    outcome.canary.submitted_receipt_count,
                                    outcome.canary.private_confirmation_count,
                                    outcome.canary.residual_risk.as_deref(),
                                ) {
                                    halt_reason = Some(
                                        "funding arb entry dispatch was rejected after mutable execution started; resident live stopped"
                                            .to_owned(),
                                    );
                                    break;
                                }
                            }
                        }
                    }
                    Ok(FundingArbResidentCycleResult::NoCandidate {
                        pair_id,
                        symbol,
                        reason,
                        blocking_path,
                        snapshot_path,
                    }) => {
                        append_funding_arb_resident_no_candidate_event(
                            &output_root,
                            FundingArbResidentNoCandidateEventInput {
                                cycle: cycles,
                                cycle_dir: &cycle_dir,
                                snapshot_path: &snapshot_path,
                                pair_id: pair_id.as_deref(),
                                symbol: symbol.as_deref(),
                                reason: &reason,
                                blocking_path: &blocking_path,
                            },
                        )?;
                    }
                    Err(error) => {
                        append_funding_arb_resident_error_event(
                            &output_root,
                            cycles,
                            &cycle_dir,
                            &error.to_string(),
                        )?;
                        if options.execute_live && !resident_entry_cycle_error_can_retry(&error) {
                            halt_reason = Some(format!(
                                "funding arb resident cycle failed; resident live stopped: {error}"
                            ));
                            break;
                        }
                    }
                }
            }
            ResidentEntryCapacity::Blocked(reason) => {
                append_funding_arb_resident_capacity_event(&output_root, cycles, &reason)?;
                let registry = load_funding_arb_resident_position_registry(&output_root)?;
                if registry.active_positions().is_empty() {
                    halt_reason = Some(reason);
                    break;
                }
            }
        }

        if !binance_basis_resident_cycle_allowed(cycles, options.max_cycles) {
            halt_reason = Some("max cycles reached".to_owned());
            break;
        }
        write_funding_arb_resident_live_progress_summary(
            &output_root,
            FundingArbResidentLiveProgressInput {
                phase: "running",
                cycles,
                last_pair_id: &last_pair_id,
                last_symbol: &last_symbol,
                last_net_funding_bps,
                dispatch_attempted,
                halt_reason: None,
            },
        )?;
        thread::sleep(Duration::from_secs(options.poll_interval_secs));
    }

    let registry = load_funding_arb_resident_position_registry(&output_root)?;
    let phase = if halt_reason.is_some() {
        "halted".to_owned()
    } else {
        "completed".to_owned()
    };
    write_funding_arb_resident_live_state(&output_root, &phase, cycles, halt_reason.as_deref())?;
    let report = FundingArbResidentLiveReport {
        phase,
        cycles,
        last_pair_id,
        last_symbol,
        last_net_funding_bps,
        dispatch_attempted,
        live_entry_count: registry.live_entry_count() as u64,
        open_position_count: registry.active_positions().len(),
        closed_position_count: registry.closed_position_count(),
        unknown_position_count: registry.unknown_position_count(),
        halt_reason,
        output_dir: Some(output_root.clone()),
        mutable_execution_started: dispatch_attempted,
    };
    write_funding_arb_resident_live_summary(&output_root, &report)?;
    log_funding_arb_resident_live_heartbeat(&report);
    Ok(report)
}

#[cfg(feature = "live-exec")]
fn funding_arb_position_recovery_drain_is_complete(
    registry: &FundingArbResidentPositionRegistry,
) -> bool {
    registry.active_positions().is_empty() && registry.unknown_position_count() == 0
}

#[cfg(all(test, feature = "live-exec"))]
mod tests {
    use super::*;

    #[test]
    fn funding_arb_position_recovery_drain_requires_no_open_or_unknown_positions() {
        let mut registry = FundingArbResidentPositionRegistry::default();
        assert!(funding_arb_position_recovery_drain_is_complete(&registry));

        registry.positions.insert(
            "pos:open".to_owned(),
            FundingArbResidentPosition {
                position_id: "pos:open".to_owned(),
                position_state_path: PathBuf::from("position.json"),
                pair_id: "binance:bybit:LABUSDT:LABUSDT".to_owned(),
                symbol: "LABUSDT".to_owned(),
                notional_usdt: "50.00".to_owned(),
                status: "open".to_owned(),
                opened_at: None,
                closed_at: None,
            },
        );
        assert!(!funding_arb_position_recovery_drain_is_complete(&registry));

        registry
            .positions
            .get_mut("pos:open")
            .expect("open position")
            .status = "closed".to_owned();
        assert!(funding_arb_position_recovery_drain_is_complete(&registry));

        registry.positions.insert(
            "pos:unknown".to_owned(),
            FundingArbResidentPosition {
                position_id: "pos:unknown".to_owned(),
                position_state_path: PathBuf::new(),
                pair_id: "bybit:bitget:PRLUSDT:PRLUSDT".to_owned(),
                symbol: "PRLUSDT".to_owned(),
                notional_usdt: "unknown".to_owned(),
                status: "unknown".to_owned(),
                opened_at: None,
                closed_at: None,
            },
        );
        assert!(!funding_arb_position_recovery_drain_is_complete(&registry));
    }
}
