#![allow(clippy::wildcard_imports)]

use crate::*;

/// 运行 Binance BTCUSDT GuardedLive 受控实盘分发。
///
/// 中文说明：默认构建始终拒绝。`live-exec` 构建会先检查人工审批、熔断、真实签名、
/// 私有账户余额和分发白名单；真实 REST 下单后仍必须通过私有查单确认生成执行报告。
pub(crate) fn run_binance_guarded_live_dispatch_impl(
    options: BinanceGuardedLiveDispatchOptions,
) -> RuntimeResult<BinanceGuardedLiveDispatchReport> {
    #[cfg(feature = "live-exec")]
    {
        run_binance_guarded_live_dispatch_live(options)
    }
    #[cfg(not(feature = "live-exec"))]
    {
        let _ = options;
        Err(RuntimeError::UnsafeConfig {
            message: "当前 arb-runtime 未使用 live-exec feature 构建，拒绝真实 Binance 下单"
                .to_owned(),
        })
    }
}

#[cfg(feature = "live-exec")]
pub(crate) fn run_binance_guarded_live_dispatch_live(
    options: BinanceGuardedLiveDispatchOptions,
) -> RuntimeResult<BinanceGuardedLiveDispatchReport> {
    if !options.acknowledge_live_orders {
        return Err(RuntimeError::UnsafeConfig {
            message: "缺少 --i-understand-live-orders，拒绝读取凭证或提交真实订单".to_owned(),
        });
    }

    let (pending, approved_record) = load_binance_manual_gate_release_inputs(&options.preview_dir)?;
    let release = release_manual_approval_gate(&pending, &approved_record)?;
    let plan = &pending.plan_preview;
    let plan_hash = execution_plan_hash(plan);
    let config = arb_config::ArbConfig::from_path(&options.config_path)?;
    let service = start_runtime_with_config(&config)?;
    let health = service.health();
    let generated_at = current_utc_timestamp()?;
    let generated_at_text = generated_at.to_string();
    let output_dir = Some(
        options
            .output_dir
            .clone()
            .unwrap_or_else(|| options.preview_dir.clone()),
    );

    let mut blocking_reasons =
        live_dispatch_blocking_reasons(&config, plan, &release, &plan_hash, &health);
    let mut receipts = Vec::new();
    let mut confirmations = Vec::new();
    let mut execution_report = None;
    let mut pre_account_event = None;
    let mut post_account_event = None;

    if blocking_reasons.is_empty() {
        let signing_policy = signing_policy_from_config(&config)?;
        let pre_account = fetch_binance_spot_private_account_snapshot(
            &signing_policy,
            &generated_at,
            "pre-dispatch",
        )?;
        ensure_private_balance_covers_plan(plan, &pre_account.balances)?;
        pre_account_event = Some(pre_account.balance_event_json);

        let mut adapter = build_binance_spot_live_adapter(&signing_policy)?;
        let dispatch_plan = build_execution_dispatch_plan(
            plan,
            &live_dispatch_policy_from_config(&config, BASIS_SYMBOL)?,
            generated_at,
        )?;
        if dispatch_plan.requests.len() != 1 {
            blocking_reasons.push(format!(
                "当前 GuardedLive 实盘入口只允许单腿 BTCUSDT spot 计划，收到 {} 腿",
                dispatch_plan.requests.len()
            ));
        } else {
            let planned = &dispatch_plan.requests[0];
            if planned.request.venue_id.as_str() != BINANCE_BASIS_SPOT_VENUE_ID {
                blocking_reasons.push(format!(
                    "当前 GuardedLive 实盘入口只允许 venue `{}`，收到 `{}`",
                    BINANCE_BASIS_SPOT_VENUE_ID, planned.request.venue_id
                ));
            } else {
                let receipt = adapter.submit_order(planned.request.clone())?;
                receipts.push(receipt.clone());
                let update =
                    confirm_live_receipt_with_order_query(&mut adapter, planned, &receipt)?;
                confirmations.push(private_confirmation_from_update(
                    &planned.plan_leg_id,
                    &update,
                ));
                execution_report =
                    Some(arb_execution::execution_report_from_private_confirmations(
                        arb_execution::PrivateExecutionReportInput::new(
                            plan,
                            &generated_at_text,
                            &confirmations,
                        ),
                    )?);
            }
        }

        let post_account = fetch_binance_spot_private_account_snapshot(
            &signing_policy,
            &generated_at,
            "post-dispatch",
        )?;
        post_account_event = Some(post_account.balance_event_json);
    }

    let report = BinanceGuardedLiveDispatchReport {
        plan_id: plan.plan_id.as_str().to_owned(),
        plan_hash,
        approval_event_id: release.approval_event_id,
        dispatch_allowed: blocking_reasons.is_empty(),
        submitted_receipt_count: receipts.len(),
        private_confirmation_count: confirmations.len(),
        pre_account_balance_event_id: pre_account_event
            .as_ref()
            .and_then(|event| json_string_field(event, "event_id").ok()),
        post_account_balance_event_id: post_account_event
            .as_ref()
            .and_then(|event| json_string_field(event, "event_id").ok()),
        execution_report_status: execution_report
            .as_ref()
            .map(|report| report.status.as_str().to_owned()),
        blocking_reasons,
        output_dir,
    };

    if let Some(dir) = &report.output_dir {
        write_binance_guarded_live_dispatch_artifacts(
            dir,
            &report,
            &receipts,
            &confirmations,
            execution_report.as_ref(),
            pre_account_event.as_deref(),
            post_account_event.as_deref(),
        )?;
    }
    Ok(report)
}
