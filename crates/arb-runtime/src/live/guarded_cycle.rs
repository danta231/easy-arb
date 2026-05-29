#![allow(clippy::wildcard_imports)]

use crate::*;

/// 生成 Binance BTCUSDT GuardedLivePersonal 计划预览。
///
/// 中文说明：该函数只读取已保存的公开行情 artifact，并生成不可调度的人工审批
/// 计划预览；它不访问私有账户、不下单、不撤单、不转账、不签名。
pub(crate) fn run_binance_guarded_live_preview_impl(
    market_artifacts_dir: impl AsRef<Path>,
    output_dir: Option<PathBuf>,
    decision_request: Option<BinanceManualApprovalDecisionRequest>,
) -> RuntimeResult<BinanceGuardedLivePreviewReport> {
    let market_artifacts_dir = market_artifacts_dir.as_ref();
    let quote = load_binance_guarded_live_spot_quote(market_artifacts_dir)?;
    let generated_at = current_utc_timestamp()?.to_string();
    let plan_created_at = quote.observed_at.clone();
    let candidate = binance_guarded_live_candidate(&quote, &plan_created_at)?;
    let risk_decision = binance_guarded_live_manual_risk_decision(&quote, &generated_at)?;
    let outcome = build_execution_plan_preview(ExecutionPlanBuildInput::new(
        &risk_decision,
        &candidate,
        ContractExecutionMode::GuardedLive,
        &plan_created_at,
    ))?;
    let pending = match outcome {
        PlanBuildOutcome::PendingManualApproval(pending) => pending,
        PlanBuildOutcome::Schedulable(_) => {
            return Err(RuntimeError::UnsafeConfig {
                message: "Binance GuardedLive preview unexpectedly produced a dispatchable plan"
                    .to_owned(),
            });
        }
    };
    let plan_hash = execution_plan_hash(&pending.plan_preview);
    if plan_hash != pending.approval_material.plan_hash {
        return Err(RuntimeError::Module {
            module: "arb-runtime",
            message: "manual approval material plan_hash does not match canonical plan hash"
                .to_owned(),
        });
    }

    let mut approval_records = Vec::new();
    if let Some(request) = decision_request {
        if request.expected_plan_hash != plan_hash {
            return Err(RuntimeError::UnsafeConfig {
                message: format!(
                    "expected plan hash `{}` does not match current plan hash `{plan_hash}`",
                    request.expected_plan_hash
                ),
            });
        }
        approval_records.push(review_manual_approval(
            ManualApprovalReviewInput::new(
                &pending,
                &request.approval_event_id,
                &request.reviewer_id,
                &request.decided_at,
                &request.expires_at,
                request.decision,
            )
            .with_reason(&request.reason),
        )?);
    }
    let approval_audit_records = approval_records
        .iter()
        .map(manual_approval_audit_record_from_execution)
        .collect::<Vec<_>>();
    let manual_material = generate_manual_approval_material(
        &risk_decision,
        &pending.plan_preview,
        &plan_hash,
        &approval_audit_records,
        &generated_at,
    )?;
    let manual_material_md = manual_material.render_markdown()?;
    let approval_records_jsonl = manual_approval_records_jsonl(&approval_records);
    let confirmation_template_md = binance_manual_confirmation_template(
        &pending.plan_preview,
        &plan_hash,
        &generated_at,
        &approval_records,
    );

    let output_dir =
        Some(output_dir.unwrap_or_else(|| PathBuf::from(BINANCE_GUARDED_LIVE_PREVIEW_DEFAULT_OUT)));
    if let Some(dir) = &output_dir {
        write_binance_guarded_live_preview_artifacts(
            dir,
            BinanceGuardedLivePreviewArtifacts {
                candidate: &candidate,
                risk_decision: &risk_decision,
                plan_preview: &pending.plan_preview,
                plan_hash: &plan_hash,
                manual_material_md: &manual_material_md,
                approval_records_jsonl: &approval_records_jsonl,
                confirmation_template_md: &confirmation_template_md,
            },
        )?;
    }

    Ok(BinanceGuardedLivePreviewReport {
        symbol: BASIS_SYMBOL.to_owned(),
        source_event_id: quote.event_id,
        generated_at,
        plan_id: pending.plan_preview.plan_id.as_str().to_owned(),
        plan_hash,
        dispatchable_before_approval: pending.is_dispatchable(),
        approval_record_count: approval_records.len(),
        approval_status: approval_records
            .first()
            .map(|record| record.status.as_str().to_owned()),
        output_dir,
    })
}

/// 预览释放 Binance BTCUSDT 人工审批门禁。
///
/// 中文说明：该函数只消费本地 Approved 审批记录并生成可审计释放事实预览；
/// 不分发订单、不下单、不撤单、不转账、不签名。
pub(crate) fn run_binance_manual_gate_release_preview_impl(
    preview_dir: impl AsRef<Path>,
    output_dir: Option<PathBuf>,
) -> RuntimeResult<BinanceManualGateReleasePreviewReport> {
    let preview_dir = preview_dir.as_ref();
    let (pending, approved_record) = load_binance_manual_gate_release_inputs(preview_dir)?;
    let release = release_manual_approval_gate(&pending, &approved_record)?;
    let dispatchable_after_release = false;
    let generated_at = current_utc_timestamp()?.to_string();
    let output_dir = Some(output_dir.unwrap_or_else(|| preview_dir.to_path_buf()));
    if let Some(dir) = &output_dir {
        write_binance_manual_gate_release_artifacts(
            dir,
            &release,
            dispatchable_after_release,
            &generated_at,
        )?;
    }

    Ok(BinanceManualGateReleasePreviewReport {
        plan_id: release.plan_id,
        plan_hash: release.plan_hash,
        approval_event_id: release.approval_event_id,
        released_manual_gate: release.gate_transition.to_state.as_str() == "Ready",
        dependent_transition_count: release.dependent_transitions.len(),
        dispatchable_after_release,
        output_dir,
    })
}

/// 运行 Binance BTCUSDT 分发前 dry run。
///
/// 中文说明：dry run 只检查释放人工门禁后的分发阻断项。当前命令必须保持
/// fail-closed：不会调用真实交易 API，不会写账本，不会签名，不会下单。
pub(crate) fn run_binance_pre_dispatch_dry_run_impl(
    preview_dir: impl AsRef<Path>,
    config_path: impl AsRef<Path>,
    output_dir: Option<PathBuf>,
) -> RuntimeResult<BinancePreDispatchDryRunReport> {
    let preview_dir = preview_dir.as_ref();
    let config_path = config_path.as_ref();
    let release_report = run_binance_manual_gate_release_preview(preview_dir, output_dir.clone())?;
    let service = start_runtime_from_config_path(config_path)?;
    let health = service.health();
    let mut blocking_reasons = Vec::new();
    if !release_report.released_manual_gate {
        blocking_reasons.push("manual gate release preview is not Ready".to_owned());
    }
    if health.kill_switch_triggered {
        blocking_reasons.push("kill switch is triggered for the preflight config".to_owned());
    }
    if !health.mutable_execution_started {
        blocking_reasons.push("mutable execution task is not started".to_owned());
    }
    blocking_reasons.push(
        "pre-dispatch dry run does not enable live-exec, real signing, private balance reads, or order dispatch"
            .to_owned(),
    );
    blocking_reasons.push(
        "approval releases only the manual gate; execution mode, capital, ledger, reconciliation, permission, and signer gates remain mandatory"
            .to_owned(),
    );
    let dispatch_allowed = false;
    let generated_at = current_utc_timestamp()?.to_string();
    let output_dir = Some(output_dir.unwrap_or_else(|| preview_dir.to_path_buf()));
    if let Some(dir) = &output_dir {
        write_binance_pre_dispatch_dry_run_artifacts(
            dir,
            &release_report,
            &health,
            &blocking_reasons,
            dispatch_allowed,
            &generated_at,
        )?;
    }

    Ok(BinancePreDispatchDryRunReport {
        plan_id: release_report.plan_id,
        plan_hash: release_report.plan_hash,
        approval_event_id: release_report.approval_event_id,
        manual_gate_released: release_report.released_manual_gate,
        dispatch_allowed,
        blocking_reasons,
        output_dir,
    })
}

/// 运行一次 Binance BTCUSDT 自动链路。
///
/// 中文说明：该函数会读取一次最新公开 spot bookTicker，生成新的策略候选和计划，
/// 自动生成同一 plan_hash 的受控审批事实，然后进入 dry-run 或显式实盘分发。
pub(crate) fn run_binance_guarded_live_cycle_impl(
    options: BinanceGuardedLiveCycleOptions,
) -> RuntimeResult<BinanceGuardedLiveCycleReport> {
    let source_url = binance_spot_book_ticker_url(BASIS_SYMBOL);
    let raw_spot_book = fetch_public_json_with_curl(&source_url)?;
    let ingested_at = current_utc_timestamp()?;
    run_binance_guarded_live_cycle_from_spot_json(&raw_spot_book, &source_url, ingested_at, options)
}

/// 运行一次 Binance spot-perp basis 双腿自动套利链路。
pub(crate) fn run_binance_basis_guarded_live_cycle_impl(
    options: BasisGuardedLiveCycleOptions,
) -> RuntimeResult<BasisGuardedLiveCycleReport> {
    #[cfg(feature = "live-exec")]
    {
        run_binance_basis_guarded_live_cycle_live(options)
    }
    #[cfg(not(feature = "live-exec"))]
    {
        let _ = options;
        Err(RuntimeError::UnsafeConfig {
            message:
                "当前 arb-runtime 未使用 live-exec feature 构建，拒绝 Binance basis 实盘自动链路"
                    .to_owned(),
        })
    }
}

/// 运行一次 Bybit spot-linear basis 双腿自动套利链路。
pub(crate) fn run_bybit_basis_guarded_live_cycle_impl(
    options: BybitBasisGuardedLiveCycleOptions,
) -> RuntimeResult<BybitBasisGuardedLiveCycleReport> {
    #[cfg(feature = "live-exec")]
    {
        run_bybit_basis_guarded_live_cycle_live(options)
    }
    #[cfg(not(feature = "live-exec"))]
    {
        let _ = options;
        Err(RuntimeError::UnsafeConfig {
            message:
                "当前 arb-runtime 未使用 live-exec feature 构建，拒绝 Bybit basis 实盘自动链路"
                    .to_owned(),
        })
    }
}

/// 运行一次 OKX spot-swap basis 双腿自动套利链路。
pub(crate) fn run_okx_basis_guarded_live_cycle_impl(
    options: OkxBasisGuardedLiveCycleOptions,
) -> RuntimeResult<OkxBasisGuardedLiveCycleReport> {
    #[cfg(feature = "live-exec")]
    {
        run_okx_basis_guarded_live_cycle_live(options)
    }
    #[cfg(not(feature = "live-exec"))]
    {
        let _ = options;
        Err(RuntimeError::UnsafeConfig {
            message: "当前 arb-runtime 未使用 live-exec feature 构建，拒绝 OKX basis 实盘自动链路"
                .to_owned(),
        })
    }
}

/// 运行一次 Bitget spot-USDT futures basis 双腿自动套利链路。
pub(crate) fn run_bitget_basis_guarded_live_cycle_impl(
    options: BitgetBasisGuardedLiveCycleOptions,
) -> RuntimeResult<BitgetBasisGuardedLiveCycleReport> {
    #[cfg(feature = "live-exec")]
    {
        run_bitget_basis_guarded_live_cycle_live(options)
    }
    #[cfg(not(feature = "live-exec"))]
    {
        let _ = options;
        Err(RuntimeError::UnsafeConfig {
            message:
                "当前 arb-runtime 未使用 live-exec feature 构建，拒绝 Bitget basis 实盘自动链路"
                    .to_owned(),
        })
    }
}

/// 从 funding arb observer 快照中选择一个候选并生成 guarded dry-run 报告。
pub(crate) fn run_funding_arb_guarded_dry_run_once_impl(
    options: FundingArbGuardedDryRunOnceOptions,
) -> RuntimeResult<FundingArbGuardedDryRunReport> {
    validate_funding_arb_guarded_dry_run_once_options(&options)?;
    let snapshot_json = read_utf8(&options.snapshot_path)?;
    let snapshot = parse_funding_arb_monitor_snapshot_json(&snapshot_json)?;
    let row = snapshot
        .rows
        .iter()
        .find(|row| row.pair_id == options.pair_id)
        .ok_or_else(|| RuntimeError::Module {
            module: "arb-runtime",
            message: format!(
                "funding arb snapshot does not contain candidate pair_id `{}`",
                options.pair_id
            ),
        })?;
    if !row.is_candidate {
        return Err(RuntimeError::Module {
            module: "arb-runtime",
            message: format!(
                "funding arb snapshot row `{}` is not a candidate",
                options.pair_id
            ),
        });
    }
    let observed_at =
        UtcTimestamp::from_str(&snapshot.updated_at).map_err(|error| RuntimeError::Module {
            module: "arb-runtime",
            message: format!(
                "funding arb snapshot updated_at is not a strict UTC timestamp: {error}"
            ),
        })?;
    let spec = funding_arb_pipeline_spec_from_monitor_row(row, &options)?;
    let events = funding_arb_monitor_row_to_normalized_events(&spec, row, observed_at)?;
    let mut funding_settlement_source = "not_provided";
    let funding_settlement = match &options.funding_settlement_ledger_path {
        Some(path) => {
            funding_settlement_source = "ledger_snapshot";
            let ledger_json = read_utf8(path)?;
            reconcile_funding_settlement_ledger_json(row, &spec, &ledger_json)?
        }
        None => match &options.funding_settlement_raw_snapshot_path {
            Some(path) => {
                funding_settlement_source = "raw_snapshot";
                let raw_json = read_utf8(path)?;
                reconcile_funding_settlement_raw_snapshot_json(row, &spec, &raw_json)?
            }
            None => FundingSettlementReconciliationSummary::not_provided(),
        },
    };
    let funding_settlement_ingestion = funding_settlement_ingestion_summary(
        &spec,
        funding_settlement_source,
        funding_settlement.snapshot_updated_at.clone(),
    );
    let private_accounts = match &options.private_account_snapshot_path {
        Some(path) => {
            let account_json = read_utf8(path)?;
            reconcile_funding_private_account_snapshot_json(&spec, &account_json)?
        }
        None => match &options.private_account_raw_snapshot_path {
            Some(path) => {
                let account_json = read_utf8(path)?;
                reconcile_funding_private_account_raw_snapshot_json(&spec, &account_json)?
            }
            None => FundingPrivateAccountReconciliationSummary::not_provided(),
        },
    };
    let private_positions = match &options.private_position_snapshot_path {
        Some(path) => {
            let snapshot_json = read_utf8(path)?;
            reconcile_funding_private_position_snapshot_json(row, &spec, &snapshot_json)?
        }
        None => match &options.private_position_raw_snapshot_path {
            Some(path) => {
                let snapshot_json = read_utf8(path)?;
                reconcile_funding_private_position_raw_snapshot_json(row, &spec, &snapshot_json)?
            }
            None => FundingPrivatePositionReconciliationSummary::not_provided(),
        },
    };
    let private_execution = match &options.private_execution_snapshot_path {
        Some(path) => {
            let execution_json = read_utf8(path)?;
            reconcile_funding_private_execution_snapshot_json(row, &spec, &execution_json)?
        }
        None => FundingPrivateExecutionReconciliationSummary::not_provided(),
    };
    let config = arb_config::ArbConfig::from_path(&options.config_path)?;
    let report = run_funding_arb_guarded_dry_run_from_normalized_events_with_reconciliations(
        &config,
        &spec,
        events,
        observed_at,
        FundingArbGuardedDryRunReconciliations {
            funding_settlement,
            funding_settlement_ingestion,
            private_accounts,
            private_positions,
            private_execution,
        },
    )?;
    if let Some(output_dir) = &options.output_dir {
        write_funding_arb_guarded_dry_run_artifacts(
            output_dir,
            &snapshot,
            &snapshot_json,
            row,
            &spec,
            &report,
        )?;
    }
    Ok(report)
}

/// 运行一次 funding arb guarded live canary。
pub(crate) fn run_funding_arb_guarded_live_canary_once_impl(
    options: FundingArbGuardedLiveCanaryOnceOptions,
) -> RuntimeResult<FundingArbGuardedLiveCanaryOnceReport> {
    #[cfg(feature = "live-exec")]
    {
        run_funding_arb_guarded_live_canary_once_live(options)
    }
    #[cfg(not(feature = "live-exec"))]
    {
        let _ = options;
        Err(RuntimeError::UnsafeConfig {
            message: "funding-arb guarded live canary requires the live-exec feature".to_owned(),
        })
    }
}

pub(crate) fn run_binance_guarded_live_cycle_from_spot_json(
    raw_spot_book: &str,
    raw_response_ref: &str,
    ingested_at: UtcTimestamp,
    options: BinanceGuardedLiveCycleOptions,
) -> RuntimeResult<BinanceGuardedLiveCycleReport> {
    let output_root = options
        .output_dir
        .clone()
        .unwrap_or_else(|| PathBuf::from(BINANCE_GUARDED_LIVE_CYCLE_DEFAULT_OUT));
    let output_dir = Some(output_root.clone());
    let market_dir = output_root.join("market");
    let preview_dir = output_root.join("preview");
    let dry_run_dir = output_root.join("dry-run");
    let live_dispatch_dir = output_root.join("live-dispatch");

    let quote = write_binance_guarded_live_auto_market_artifacts(
        raw_spot_book,
        raw_response_ref,
        ingested_at,
        &market_dir,
    )?;
    let mut blocking_reasons = Vec::new();
    if let Some(max_ask) = &options.max_ask {
        if Decimal::from_str(&quote.best_ask)? > Decimal::from_str(max_ask)? {
            blocking_reasons.push(format!(
                "strategy signal blocked: best_ask={} is above max_ask={max_ask}",
                quote.best_ask
            ));
        }
    } else if options.execute_live {
        blocking_reasons.push(
            "live automatic execution requires --max-ask so the strategy has an explicit price guard"
                .to_owned(),
        );
    }

    if !blocking_reasons.is_empty() {
        let report = BinanceGuardedLiveCycleReport {
            symbol: BASIS_SYMBOL.to_owned(),
            strategy_id: BINANCE_GUARDED_LIVE_STRATEGY_ID.to_owned(),
            source_event_id: Some(quote.event_id),
            best_ask: Some(quote.best_ask),
            max_ask: options.max_ask,
            signal_allowed: false,
            plan_hash: None,
            approval_event_id: None,
            manual_gate_released: false,
            dispatch_attempted: false,
            dispatch_allowed: false,
            submitted_receipt_count: 0,
            private_confirmation_count: 0,
            execution_report_status: None,
            blocking_reasons,
            output_dir,
        };
        write_binance_guarded_live_cycle_artifacts(&output_root, &report)?;
        return Ok(report);
    }

    let preview = run_binance_guarded_live_preview(&market_dir, Some(preview_dir.clone()), None)?;
    let approval_event_id = format!(
        "event:approval:auto-live:binance-btcusdt:{}",
        ingested_at.unix_seconds()
    );
    let decided_at = ingested_at.to_string();
    let expires_at =
        UtcTimestamp::from_unix_parts(ingested_at.unix_seconds() + 300, 0)?.to_string();
    let approved_preview = run_binance_guarded_live_preview(
        &market_dir,
        Some(preview_dir.clone()),
        Some(BinanceManualApprovalDecisionRequest {
            decision: ManualApprovalDecision::Approve,
            expected_plan_hash: preview.plan_hash.clone(),
            approval_event_id: approval_event_id.clone(),
            reviewer_id: "system:guarded-live-auto-strategy".to_owned(),
            decided_at,
            expires_at,
            reason: "Automatic guarded-live strategy signal approved for the same fresh plan hash."
                .to_owned(),
        }),
    )?;
    let release = run_binance_manual_gate_release_preview(&preview_dir, Some(preview_dir.clone()))?;

    let (
        dispatch_attempted,
        dispatch_allowed,
        submitted_receipt_count,
        private_confirmation_count,
        execution_report_status,
        mut dispatch_reasons,
    ) = if options.execute_live {
        if !options.acknowledge_auto_live_orders {
            return Err(RuntimeError::UnsafeConfig {
                message: "缺少 --i-understand-auto-live-orders，拒绝进入自动真实下单链路"
                    .to_owned(),
            });
        }
        let dispatch = run_binance_guarded_live_dispatch(BinanceGuardedLiveDispatchOptions {
            preview_dir: preview_dir.clone(),
            config_path: options.config_path.clone(),
            output_dir: Some(live_dispatch_dir),
            acknowledge_live_orders: true,
        })?;
        (
            true,
            dispatch.dispatch_allowed,
            dispatch.submitted_receipt_count,
            dispatch.private_confirmation_count,
            dispatch.execution_report_status,
            dispatch.blocking_reasons,
        )
    } else {
        let dry_run = run_binance_pre_dispatch_dry_run(
            &preview_dir,
            &options.config_path,
            Some(dry_run_dir),
        )?;
        let mut reasons = dry_run.blocking_reasons;
        reasons.push(
                "automatic chain ran in dry-run mode; pass --execute-live and --i-understand-auto-live-orders to submit a real order"
                    .to_owned(),
            );
        (false, false, 0, 0, None, reasons)
    };

    let mut report_reasons = Vec::new();
    report_reasons.append(&mut dispatch_reasons);
    let report = BinanceGuardedLiveCycleReport {
        symbol: BASIS_SYMBOL.to_owned(),
        strategy_id: BINANCE_GUARDED_LIVE_STRATEGY_ID.to_owned(),
        source_event_id: Some(quote.event_id),
        best_ask: Some(quote.best_ask),
        max_ask: options.max_ask,
        signal_allowed: true,
        plan_hash: Some(approved_preview.plan_hash),
        approval_event_id: Some(release.approval_event_id),
        manual_gate_released: release.released_manual_gate,
        dispatch_attempted,
        dispatch_allowed,
        submitted_receipt_count,
        private_confirmation_count,
        execution_report_status,
        blocking_reasons: report_reasons,
        output_dir,
    };
    write_binance_guarded_live_cycle_artifacts(&output_root, &report)?;
    Ok(report)
}

#[cfg(feature = "live-exec")]
pub(crate) fn run_binance_basis_guarded_live_cycle_live(
    options: BasisGuardedLiveCycleOptions,
) -> RuntimeResult<BasisGuardedLiveCycleReport> {
    let symbol = normalize_cex_usdt_basis_symbol(&options.symbol, "Binance")?;
    let spot_instrument_id = binance_basis_spot_instrument_id(&symbol)?;
    let perp_instrument_id = binance_basis_perp_instrument_id(&symbol)?;
    let spot_url = binance_spot_book_ticker_url(&symbol);
    let perp_url = binance_usdm_book_ticker_url(&symbol);
    let premium_url = binance_usdm_premium_index_url(&symbol);
    let wss_monitor_urls = match (
        options.spot_wss_monitor_url.as_deref(),
        options.perp_wss_monitor_url.as_deref(),
    ) {
        (Some(spot_monitor), Some(perp_monitor)) => Some((spot_monitor, perp_monitor)),
        (None, None) if !options.execute_live => None,
        (None, None) => {
            return Err(RuntimeError::UnsafeConfig {
                message:
                    "真实 basis 自动下单必须提供 --spot-wss-monitor-url 和 --perp-wss-monitor-url"
                        .to_owned(),
            });
        }
        _ => {
            return Err(RuntimeError::UnsafeConfig {
                message:
                    "basis 自动链路必须同时提供 --spot-wss-monitor-url 和 --perp-wss-monitor-url"
                        .to_owned(),
            });
        }
    };
    let (raw_spot, spot_ref, raw_perp, perp_ref) =
        if let Some((spot_monitor, perp_monitor)) = wss_monitor_urls {
            let spot_snapshot = fetch_binance_basis_wss_monitor_book_ticker_json(
                spot_monitor,
                &symbol,
                BinancePublicMarket::Spot,
                BINANCE_BASIS_SPOT_VENUE_ID,
                &spot_instrument_id,
            )?;
            let perp_snapshot = fetch_binance_basis_wss_monitor_book_ticker_json(
                perp_monitor,
                &symbol,
                BinancePublicMarket::UsdmPerpetual,
                BINANCE_BASIS_PERP_VENUE_ID,
                &perp_instrument_id,
            )?;
            (
                spot_snapshot.raw_ticker_json,
                spot_snapshot.source_ref,
                perp_snapshot.raw_ticker_json,
                perp_snapshot.source_ref,
            )
        } else {
            (
                fetch_public_json_with_curl(&spot_url)?,
                spot_url.clone(),
                fetch_public_json_with_curl(&perp_url)?,
                perp_url.clone(),
            )
        };
    let raw_premium = fetch_public_json_with_curl(&premium_url)?;
    let ingested_at = current_utc_timestamp()?;
    run_binance_basis_guarded_live_cycle_from_json(
        &symbol,
        &raw_spot,
        &spot_ref,
        &raw_perp,
        &perp_ref,
        &raw_premium,
        &premium_url,
        ingested_at,
        options,
    )
}

#[cfg(feature = "live-exec")]
pub(crate) fn run_bybit_basis_guarded_live_cycle_live(
    options: BybitBasisGuardedLiveCycleOptions,
) -> RuntimeResult<BybitBasisGuardedLiveCycleReport> {
    let symbol = normalize_cex_usdt_basis_symbol(&options.symbol, "Bybit")?;
    let spot_instrument_id = bybit_basis_spot_instrument_id(&symbol)?;
    let perp_instrument_id = bybit_basis_perp_instrument_id(&symbol)?;
    let spot_url = bybit_spot_tickers_url();
    let linear_url = bybit_linear_tickers_url();
    let wss_monitor_urls = match (
        options.spot_wss_monitor_url.as_deref(),
        options.perp_wss_monitor_url.as_deref(),
    ) {
        (Some(spot_monitor), Some(perp_monitor)) => Some((spot_monitor, perp_monitor)),
        (None, None) if !options.execute_live => None,
        (None, None) => {
            return Err(RuntimeError::UnsafeConfig {
                message:
                    "真实 Bybit basis 自动下单必须提供 --spot-wss-monitor-url 和 --perp-wss-monitor-url"
                        .to_owned(),
            });
        }
        _ => {
            return Err(RuntimeError::UnsafeConfig {
                message:
                    "Bybit basis 自动链路必须同时提供 --spot-wss-monitor-url 和 --perp-wss-monitor-url"
                        .to_owned(),
            });
        }
    };
    let (raw_spot, spot_ref, raw_linear, linear_ref) =
        if let Some((spot_monitor, linear_monitor)) = wss_monitor_urls {
            let raw_linear_rest = fetch_public_json_with_curl(&linear_url)?;
            let spot_snapshot = fetch_bybit_basis_wss_monitor_ticker_json(
                spot_monitor,
                &symbol,
                BybitPublicMarket::Spot,
                BYBIT_BASIS_SPOT_VENUE_ID,
                &spot_instrument_id,
                None,
            )?;
            let linear_snapshot = fetch_bybit_basis_wss_monitor_ticker_json(
                linear_monitor,
                &symbol,
                BybitPublicMarket::LinearPerpetual,
                BYBIT_BASIS_PERP_VENUE_ID,
                &perp_instrument_id,
                Some(&raw_linear_rest),
            )?;
            (
                spot_snapshot.raw_ticker_json,
                spot_snapshot.source_ref,
                linear_snapshot.raw_ticker_json,
                linear_snapshot.source_ref,
            )
        } else {
            (
                fetch_public_json_with_curl(&spot_url)?,
                spot_url.clone(),
                fetch_public_json_with_curl(&linear_url)?,
                linear_url.clone(),
            )
        };
    let ingested_at = current_utc_timestamp()?;
    run_bybit_basis_guarded_live_cycle_from_json(
        &symbol,
        &raw_spot,
        &spot_ref,
        &raw_linear,
        &linear_ref,
        ingested_at,
        options,
    )
}

#[cfg(feature = "live-exec")]
pub(crate) fn run_okx_basis_guarded_live_cycle_live(
    options: OkxBasisGuardedLiveCycleOptions,
) -> RuntimeResult<OkxBasisGuardedLiveCycleReport> {
    let symbol = normalize_okx_usdt_basis_symbol(&options.symbol)?;
    let spot_instrument_id = okx_basis_spot_instrument_id(&symbol)?;
    let perp_instrument_id = okx_basis_perp_instrument_id(&symbol)?;
    let spot_url = okx_tickers_url("SPOT");
    let swap_url = okx_tickers_url("SWAP");
    let mark_url = okx_mark_price_url();
    let index_url = okx_index_tickers_url();
    let funding_url = okx_funding_rate_url(&format!("{symbol}-SWAP"));
    let wss_monitor_urls = match (
        options.spot_wss_monitor_url.as_deref(),
        options.perp_wss_monitor_url.as_deref(),
    ) {
        (Some(spot_monitor), Some(swap_monitor)) => Some((spot_monitor, swap_monitor)),
        (None, None) if !options.execute_live => None,
        (None, None) => {
            return Err(RuntimeError::UnsafeConfig {
                message:
                    "真实 OKX basis 自动下单必须提供 --spot-wss-monitor-url 和 --perp-wss-monitor-url"
                        .to_owned(),
            });
        }
        _ => {
            return Err(RuntimeError::UnsafeConfig {
                message:
                    "OKX basis 自动链路必须同时提供 --spot-wss-monitor-url 和 --perp-wss-monitor-url"
                        .to_owned(),
            });
        }
    };
    let (raw_spot, spot_ref, raw_swap, swap_ref) =
        if let Some((spot_monitor, swap_monitor)) = wss_monitor_urls {
            let spot_snapshot = fetch_okx_basis_wss_monitor_ticker_json(
                spot_monitor,
                &symbol,
                "SPOT",
                OKX_BASIS_SPOT_VENUE_ID,
                &spot_instrument_id,
            )?;
            let swap_snapshot = fetch_okx_basis_wss_monitor_ticker_json(
                swap_monitor,
                &symbol,
                "SWAP",
                OKX_BASIS_PERP_VENUE_ID,
                &perp_instrument_id,
            )?;
            (
                spot_snapshot.raw_ticker_json,
                spot_snapshot.source_ref,
                swap_snapshot.raw_ticker_json,
                swap_snapshot.source_ref,
            )
        } else {
            (
                fetch_public_json_with_curl(&spot_url)?,
                spot_url.clone(),
                fetch_public_json_with_curl(&swap_url)?,
                swap_url.clone(),
            )
        };
    let raw_mark = fetch_public_json_with_curl(&mark_url)?;
    let raw_index = fetch_public_json_with_curl(&index_url)?;
    let raw_funding = fetch_public_json_with_curl(&funding_url)?;
    let ingested_at = current_utc_timestamp()?;
    run_okx_basis_guarded_live_cycle_from_json(
        OkxBasisRawInputs {
            symbol: &symbol,
            raw_spot_ticker: &raw_spot,
            spot_ticker_ref: &spot_ref,
            raw_swap_ticker: &raw_swap,
            swap_ticker_ref: &swap_ref,
            raw_mark_price: &raw_mark,
            mark_price_ref: &mark_url,
            raw_index_ticker: &raw_index,
            index_ticker_ref: &index_url,
            raw_funding_rate: &raw_funding,
            funding_rate_ref: &funding_url,
        },
        ingested_at,
        options,
    )
}

#[cfg(feature = "live-exec")]
pub(crate) fn run_bitget_basis_guarded_live_cycle_live(
    options: BitgetBasisGuardedLiveCycleOptions,
) -> RuntimeResult<BitgetBasisGuardedLiveCycleReport> {
    let symbol = normalize_cex_usdt_basis_symbol(&options.symbol, "Bitget")?;
    let spot_url = bitget_spot_tickers_url();
    let usdt_futures_url = bitget_usdt_futures_tickers_url();
    let usdt_futures_funding_rate_url = bitget_usdt_futures_funding_rate_url();
    let spot_instrument_id = bitget_basis_spot_instrument_id(&symbol)?;
    let perp_instrument_id = bitget_basis_perp_instrument_id(&symbol)?;
    let wss_monitor_urls = match (
        options.spot_wss_monitor_url.as_deref(),
        options.perp_wss_monitor_url.as_deref(),
    ) {
        (Some(spot_monitor), Some(perp_monitor)) => Some((spot_monitor, perp_monitor)),
        (None, None) if !options.execute_live => None,
        (None, None) => {
            return Err(RuntimeError::UnsafeConfig {
                message:
                    "真实 Bitget basis 自动下单必须提供 --spot-wss-monitor-url 和 --perp-wss-monitor-url"
                        .to_owned(),
            });
        }
        _ => {
            return Err(RuntimeError::UnsafeConfig {
                message:
                    "Bitget basis 自动链路必须同时提供 --spot-wss-monitor-url 和 --perp-wss-monitor-url"
                        .to_owned(),
            });
        }
    };
    let (raw_spot, spot_ref, raw_usdt_futures, usdt_futures_ref, raw_funding, funding_ref) =
        if let Some((spot_monitor, usdt_futures_monitor)) = wss_monitor_urls {
            let raw_usdt_futures_rest = fetch_public_json_with_curl(&usdt_futures_url)?;
            let raw_usdt_futures_funding_rate =
                fetch_public_json_with_curl(&usdt_futures_funding_rate_url)?;
            let spot_snapshot = fetch_bitget_basis_wss_monitor_ticker_json(
                spot_monitor,
                &symbol,
                "spot",
                BITGET_BASIS_SPOT_VENUE_ID,
                &spot_instrument_id,
                None,
                None,
            )?;
            let futures_snapshot = fetch_bitget_basis_wss_monitor_ticker_json(
                usdt_futures_monitor,
                &symbol,
                "usdt-futures",
                BITGET_BASIS_PERP_VENUE_ID,
                &perp_instrument_id,
                Some(&raw_usdt_futures_rest),
                Some(&raw_usdt_futures_funding_rate),
            )?;
            (
                spot_snapshot.raw_ticker_json,
                spot_snapshot.source_ref,
                futures_snapshot.raw_ticker_json,
                futures_snapshot.source_ref,
                raw_usdt_futures_funding_rate,
                usdt_futures_funding_rate_url.clone(),
            )
        } else {
            (
                fetch_public_json_with_curl(&spot_url)?,
                spot_url.clone(),
                fetch_public_json_with_curl(&usdt_futures_url)?,
                usdt_futures_url.clone(),
                fetch_public_json_with_curl(&usdt_futures_funding_rate_url)?,
                usdt_futures_funding_rate_url.clone(),
            )
        };
    let ingested_at = current_utc_timestamp()?;
    run_bitget_basis_guarded_live_cycle_from_json(
        BitgetBasisRawInputs {
            symbol: &symbol,
            raw_spot_ticker: &raw_spot,
            spot_ticker_ref: &spot_ref,
            raw_usdt_futures_ticker: &raw_usdt_futures,
            usdt_futures_ticker_ref: &usdt_futures_ref,
            raw_usdt_futures_funding_rate: &raw_funding,
            usdt_futures_funding_rate_ref: &funding_ref,
        },
        ingested_at,
        options,
    )
}

#[cfg(feature = "live-exec")]
pub(crate) fn run_funding_arb_guarded_live_canary_once_live(
    options: FundingArbGuardedLiveCanaryOnceOptions,
) -> RuntimeResult<FundingArbGuardedLiveCanaryOnceReport> {
    validate_funding_arb_guarded_live_canary_once_options(&options)?;
    if options.execute_live && !options.acknowledge_funding_arb_live_orders {
        return Err(RuntimeError::UnsafeConfig {
            message:
                "缺少 --i-understand-funding-arb-live-orders，拒绝进入 funding arb 真实双腿下单链路"
                    .to_owned(),
        });
    }

    let dry_run = run_funding_arb_guarded_dry_run_once(options.dry_run.clone())?;
    let snapshot_json = read_utf8(&options.dry_run.snapshot_path)?;
    let snapshot = parse_funding_arb_monitor_snapshot_json(&snapshot_json)?;
    let row = snapshot
        .rows
        .iter()
        .find(|row| row.pair_id == options.dry_run.pair_id)
        .ok_or_else(|| RuntimeError::Module {
            module: "arb-runtime",
            message: format!(
                "funding arb snapshot does not contain candidate pair_id `{}`",
                options.dry_run.pair_id
            ),
        })?;
    let observed_at =
        UtcTimestamp::from_str(&snapshot.updated_at).map_err(|error| RuntimeError::Module {
            module: "arb-runtime",
            message: format!(
                "funding arb snapshot updated_at is not a strict UTC timestamp: {error}"
            ),
        })?;
    let spec = funding_arb_pipeline_spec_from_monitor_row(row, &options.dry_run)?;
    let events = funding_arb_monitor_row_to_normalized_events(&spec, row, observed_at)?;
    let config = arb_config::ArbConfig::from_path(&options.dry_run.config_path)?;
    let generated_at = observed_at.to_string();
    let output_dir = options
        .dry_run
        .output_dir
        .as_ref()
        .map(|dir| dir.join("live-canary"));

    let mut blocking_reasons = funding_arb_live_canary_blocking_reasons(&dry_run);
    let mut plan_hash = None;
    let mut approval_event_id = None;
    let mut manual_gate_released = false;
    let mut dispatch_plan_built = false;
    let mut dispatch_request_count = 0usize;
    let mut receipts = Vec::new();
    let mut confirmations = Vec::new();
    let mut execution_report = None;
    let mut dispatch_attempted = false;
    let mut submitted_receipt_count = 0usize;
    let mut protection = BasisLiveProtection::default();
    let mut flat_after_cancel = false;
    let mut post_fill_entry_economics_failed = false;

    let maybe_pending =
        build_funding_arb_live_canary_pending_plan(&config, &spec, events, observed_at, &dry_run)?;

    let mut maybe_dispatch_plan = None;
    if let Some(pending) = maybe_pending.as_ref() {
        let approved_record = review_manual_approval(
            ManualApprovalReviewInput::new(
                pending,
                &format!(
                    "event:approval:funding-arb-live-canary:{}",
                    observed_at.unix_seconds()
                ),
                "system:funding-arb-guarded-live-canary",
                &generated_at,
                &UtcTimestamp::from_unix_parts(observed_at.unix_seconds() + 300, 0)?.to_string(),
                ManualApprovalDecision::Approve,
            )
            .with_reason(
                "Funding arb guarded live canary approved both perp legs for the same fresh plan hash.",
            ),
        )?;
        let release = release_manual_approval_gate(pending, &approved_record)?;
        plan_hash = Some(execution_plan_hash(&pending.plan_preview));
        approval_event_id = Some(release.approval_event_id.clone());
        manual_gate_released = true;

        let service = start_runtime_with_config(&config)?;
        let health = service.health();
        blocking_reasons.extend(live_dispatch_blocking_reasons(
            &config,
            &pending.plan_preview,
            &release,
            plan_hash.as_deref().expect("plan hash"),
            &health,
        ));
        let dispatch_policy = live_dispatch_policy_from_config(&config, &row.symbol)?;
        let dispatch_plan =
            build_execution_dispatch_plan(&pending.plan_preview, &dispatch_policy, observed_at)?;
        dispatch_request_count = dispatch_plan.requests.len();
        dispatch_plan_built = true;
        if dispatch_request_count != 2 {
            blocking_reasons.push(format!(
                "funding arb canary requires exactly two perp order legs, got {dispatch_request_count}"
            ));
        }
        maybe_dispatch_plan = Some(dispatch_plan);
    } else {
        blocking_reasons.push(
            "funding arb canary could not build a manual approval plan from the current candidate"
                .to_owned(),
        );
    }

    if !options.execute_live {
        blocking_reasons.push(
            "funding arb canary ran in dry-run mode; pass --execute-live and --i-understand-funding-arb-live-orders to submit both real perp legs"
                .to_owned(),
        );
    }

    if options.execute_live && blocking_reasons.is_empty() {
        let dispatch_plan =
            maybe_dispatch_plan
                .as_mut()
                .ok_or_else(|| RuntimeError::UnsafeConfig {
                    message: "funding arb canary dispatch plan is missing".to_owned(),
                })?;
        push_funding_arb_live_pre_dispatch_blocking_reason(
            &mut blocking_reasons,
            "funding arb live quantity step guard",
            align_funding_arb_dispatch_plan_quantities(dispatch_plan),
        )?;
        if blocking_reasons.is_empty() {
            apply_funding_arb_binance_position_side(
                dispatch_plan,
                &row.symbol,
                options
                    .dry_run
                    .private_position_raw_snapshot_path
                    .as_deref(),
            )?;
            apply_funding_arb_bybit_position_side(
                dispatch_plan,
                &row.symbol,
                options
                    .dry_run
                    .private_position_raw_snapshot_path
                    .as_deref(),
            )?;
            apply_funding_arb_okx_position_side(
                dispatch_plan,
                &row.symbol,
                options
                    .dry_run
                    .private_position_raw_snapshot_path
                    .as_deref(),
            )?;
            apply_funding_arb_bitget_position_side(
                dispatch_plan,
                &row.symbol,
                options
                    .dry_run
                    .private_position_raw_snapshot_path
                    .as_deref(),
            )?;
            apply_funding_arb_aster_position_side(
                dispatch_plan,
                &row.symbol,
                options
                    .dry_run
                    .private_position_raw_snapshot_path
                    .as_deref(),
            )?;
        }
        if blocking_reasons.is_empty() {
            push_funding_arb_live_pre_dispatch_blocking_reason(
                &mut blocking_reasons,
                "funding arb live IOC price guard",
                apply_funding_arb_entry_time_in_force_ioc(
                    dispatch_plan,
                    options.dry_run.slippage_buffer_bps,
                ),
            )?;
        }
        if blocking_reasons.is_empty() {
            if let Some(reason) = funding_arb_live_dispatch_affordability_blocking_reason(
                &dry_run.private_accounts,
                dispatch_plan,
                monitor_bps_ceil_i128("taker_fee_bps", &options.dry_run.taker_fee_bps)?,
                options.dry_run.slippage_buffer_bps,
            )? {
                blocking_reasons.push(reason);
            }
        }
        if blocking_reasons.is_empty() {
            let signing_policy = signing_policy_from_config(&config)?;
            let private_order_events = options
                .private_order_events_dir
                .as_ref()
                .map(|path| PrivateOrderEventStore::from_dir(path))
                .transpose()?;
            dispatch_attempted = true;
            let outcome = execute_funding_arb_live_canary(
                dispatch_plan,
                &options,
                &signing_policy,
                FundingLiveCanaryContext {
                    plan: maybe_pending.as_ref().map(|pending| &pending.plan_preview),
                    generated_at: &generated_at,
                    protection_suffix: &observed_at.unix_seconds().to_string(),
                    private_order_events: private_order_events.as_ref(),
                    order_query_event_prefix: "event:funding-arb-live-canary-order-query",
                    slippage_buffer_bps: options.dry_run.slippage_buffer_bps,
                    price_tick_resolver: funding_arb_price_tick_for_venue,
                },
            )?;
            flat_after_cancel = outcome.flat_after_cancel;
            receipts = outcome.receipts;
            confirmations = outcome.confirmations;
            execution_report = outcome.execution_report;
            submitted_receipt_count = outcome.primary_submit_receipt_count;
            protection = outcome.protection;
            blocking_reasons.extend(outcome.blocking_reasons);
        }
    }

    if execution_report.is_none() && blocking_reasons.is_empty() && options.execute_live {
        if let Some(pending) = maybe_pending.as_ref() {
            execution_report = Some(arb_execution::execution_report_from_private_confirmations(
                arb_execution::PrivateExecutionReportInput::new(
                    &pending.plan_preview,
                    &generated_at,
                    &confirmations,
                ),
            )?);
        }
    }

    if blocking_reasons.is_empty() && options.execute_live {
        if let (Some(dispatch_plan), Some(report)) =
            (maybe_dispatch_plan.as_ref(), execution_report.as_ref())
        {
            if let Some(reason) = funding_arb_post_fill_entry_economics_blocking_reason(
                row,
                dispatch_plan,
                report,
                options.dry_run.min_net_funding_bps,
            )? {
                post_fill_entry_economics_failed = true;
                protection.record_residual_risk(
                    "funding arb entry actual fill economics fell below the configured minimum after both legs filled; position state is still written for resident supervision",
                );
                blocking_reasons.push(reason);
            }
        }
    }

    if options.execute_live
        && dispatch_attempted
        && (blocking_reasons.is_empty() || post_fill_entry_economics_failed)
        && submitted_receipt_count == 2
    {
        if let (Some(output_dir), Some(dispatch_plan)) = (&output_dir, maybe_dispatch_plan.as_ref())
        {
            write_funding_arb_position_state_from_dispatch(
                output_dir,
                row,
                dispatch_plan,
                plan_hash.clone(),
                options.private_order_events_dir.as_ref(),
                observed_at,
            )?;
        }
    }

    let mutable_execution_started = funding_arb_live_canary_mutable_execution_started(
        submitted_receipt_count,
        confirmations.len(),
        protection.attempted,
        protection.residual_risk.as_deref(),
    );

    let report = FundingArbGuardedLiveCanaryOnceReport {
        pair_id: row.pair_id.clone(),
        symbol: row.symbol.clone(),
        venue_a_family: row.venue_a_family.clone(),
        venue_b_family: row.venue_b_family.clone(),
        source_status: row.source_status.clone(),
        net_funding_bps: row.net_funding_bps.clone(),
        expected_funding_usd: row.expected_funding_usd.clone(),
        dry_run,
        plan_hash,
        approval_event_id,
        manual_gate_released,
        execute_live: options.execute_live,
        dispatch_allowed: blocking_reasons.is_empty(),
        dispatch_plan_built,
        dispatch_request_count,
        dispatch_attempted,
        submitted_receipt_count,
        private_confirmation_count: confirmations.len(),
        protection_attempted: protection.attempted,
        protection_actions: protection.actions,
        protection_receipt_count: protection.receipt_count,
        residual_risk: protection.residual_risk,
        flat_after_cancel,
        execution_report_status: execution_report
            .as_ref()
            .map(|report| report.status.as_str().to_owned()),
        blocking_reasons,
        output_dir,
        mutable_execution_started,
    };
    if let Some(output_dir) = &report.output_dir {
        write_funding_arb_guarded_live_canary_artifacts(
            output_dir,
            &report,
            &receipts,
            &confirmations,
            execution_report.as_ref(),
        )?;
    }
    Ok(report)
}
