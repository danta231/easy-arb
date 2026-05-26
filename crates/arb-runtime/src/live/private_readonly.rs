#![allow(clippy::wildcard_imports)]

#[cfg(feature = "live-exec")]
use crate::portfolio::dashboard::{truncate_for_json, ERROR_LOG_SOURCE_ERROR_CHAR_LIMIT};
use crate::*;

/// 运行一次 funding arb 私有只读实盘快照采集。
///
/// 中文说明：该入口只读取账户/仓位 USER_DATA 或公开用户状态接口并写出本地
/// raw snapshot；不提交订单、不撤单、不转账，也不启动 mutable execution。
pub(crate) fn run_funding_arb_private_readonly_snapshot_once_impl(
    options: FundingArbPrivateReadonlySnapshotOnceOptions,
) -> RuntimeResult<FundingArbPrivateReadonlySnapshotReport> {
    #[cfg(feature = "live-exec")]
    {
        run_funding_arb_private_readonly_snapshot_once_live(options)
    }
    #[cfg(not(feature = "live-exec"))]
    {
        let _ = options;
        Err(RuntimeError::UnsafeConfig {
            message: "当前 arb-runtime 未使用 live-exec feature 构建，拒绝读取交易所私有账户接口"
                .to_owned(),
        })
    }
}

/// 启动组合看板私有只读全账户采集器。
///
/// 中文说明：该入口只读取交易所私有账户和仓位，不提交订单、不撤单、不转账。
/// 默认循环采集；`once=true` 时只采集一次。
pub(crate) fn run_portfolio_private_readonly_snapshot_impl(
    options: PortfolioPrivateReadonlySnapshotOptions,
) -> RuntimeResult<PortfolioPrivateReadonlySnapshotReport> {
    validate_portfolio_private_readonly_snapshot_options(&options)?;
    if options.once {
        return run_portfolio_private_readonly_snapshot_once(options);
    }
    loop {
        let _ = run_portfolio_private_readonly_snapshot_once(options.clone())?;
        thread::sleep(Duration::from_secs(options.interval_secs));
    }
}

/// 运行一次 wallet signer 本地预检。
///
/// 中文说明：该入口只调用本地 signer 做离线签名，不访问交易所，不启动 mutable
/// execution。报告中的 `Blocked` 表示不能继续私有只读或交易签名前置流程。
pub(crate) fn run_wallet_signer_preflight_impl(
    options: WalletSignerPreflightOptions,
) -> RuntimeResult<WalletSignerPreflightReport> {
    #[cfg(feature = "live-exec")]
    {
        run_wallet_signer_preflight_live(options)
    }
    #[cfg(not(feature = "live-exec"))]
    {
        let _ = options;
        Err(RuntimeError::UnsafeConfig {
            message: "当前 arb-runtime 未使用 live-exec feature 构建，拒绝读取签名环境".to_owned(),
        })
    }
}

#[cfg(feature = "live-exec")]
pub(crate) fn run_funding_arb_private_readonly_snapshot_once_live(
    options: FundingArbPrivateReadonlySnapshotOnceOptions,
) -> RuntimeResult<FundingArbPrivateReadonlySnapshotReport> {
    validate_funding_arb_private_readonly_snapshot_once_options(&options)?;
    let snapshot_json = read_utf8(&options.snapshot_path)?;
    let snapshot = parse_funding_arb_monitor_snapshot_json(&snapshot_json)?;
    let row = snapshot
        .rows
        .iter()
        .find(|row| row.pair_id == options.pair_id)
        .ok_or_else(|| RuntimeError::Module {
            module: "arb-runtime",
            message: format!(
                "funding arb snapshot does not contain pair_id `{}`",
                options.pair_id
            ),
        })?;
    let symbol = funding_display_symbol(&funding_base_asset_from_symbol(&row.symbol));
    let venue_families = [
        normalize_venue_family(&row.venue_a_family),
        normalize_venue_family(&row.venue_b_family),
    ];
    let output_dir = options
        .output_dir
        .clone()
        .unwrap_or_else(|| PathBuf::from(FUNDING_ARB_PRIVATE_READONLY_SNAPSHOT_DEFAULT_OUT));
    fs::create_dir_all(&output_dir).map_err(|error| RuntimeError::Io {
        path: output_dir.clone(),
        message: error.to_string(),
    })?;

    let mut account_statements = Vec::new();
    let mut position_statements = Vec::new();
    let mut settlement_statements = Vec::new();
    let mut notes =
        vec!["只读快照采集未启动 mutable execution，未提交订单、未撤单、未转账。".to_owned()];
    for family in &venue_families {
        let leg = funding_arb_leg_config(family, &symbol)?;
        let fetched = fetch_funding_arb_private_readonly_venue_snapshots(
            family,
            &symbol,
            &leg.account_id,
            &leg.venue_id,
            &leg.instrument_id,
            &options,
        )?;
        account_statements.push(FundingPrivateReadonlyRawStatement {
            venue_family: family.clone(),
            account_id: leg.account_id.clone(),
            payload_json: fetched.account_payload_json,
        });
        position_statements.push(FundingPrivateReadonlyRawStatement {
            venue_family: family.clone(),
            account_id: leg.account_id.clone(),
            payload_json: fetched.position_payload_json,
        });
        settlement_statements.push(FundingPrivateReadonlyRawStatement {
            venue_family: family.clone(),
            account_id: leg.account_id.clone(),
            payload_json: fetched.settlement_payload_json,
        });
        notes.push(fetched.note);
    }

    let updated_at = current_utc_timestamp()?.to_string();
    let account_raw_json =
        funding_private_readonly_raw_snapshot_json(&updated_at, &account_statements);
    let position_raw_json =
        funding_private_readonly_raw_snapshot_json(&updated_at, &position_statements);
    let settlement_raw_json =
        funding_private_readonly_raw_snapshot_json(&updated_at, &settlement_statements);
    parse_funding_private_raw_snapshot_json(
        &account_raw_json,
        "funding private readonly account raw snapshot",
    )?;
    parse_funding_private_raw_snapshot_json(
        &position_raw_json,
        "funding private readonly position raw snapshot",
    )?;
    parse_funding_settlement_raw_snapshot_json(&settlement_raw_json)?;

    let account_raw_snapshot_path =
        output_dir.join("funding_arb_private_account_raw_snapshot.json");
    let position_raw_snapshot_path =
        output_dir.join("funding_arb_private_position_raw_snapshot.json");
    let settlement_raw_snapshot_path =
        output_dir.join("funding_arb_funding_settlement_raw_snapshot.json");
    let summary_path = output_dir.join("funding_arb_private_readonly_summary.json");
    write_utf8(account_raw_snapshot_path.clone(), &account_raw_json)?;
    write_utf8(position_raw_snapshot_path.clone(), &position_raw_json)?;
    write_utf8(settlement_raw_snapshot_path.clone(), &settlement_raw_json)?;
    let settlement_raw_snapshot_mirror_path =
        if let Some(path) = &options.funding_settlement_raw_snapshot_path {
            write_utf8_with_parent(path.clone(), &settlement_raw_json)?;
            Some(path.clone())
        } else {
            None
        };

    let report = FundingArbPrivateReadonlySnapshotReport {
        pair_id: options.pair_id,
        symbol,
        venue_families: venue_families.to_vec(),
        account_statement_count: account_statements.len(),
        position_statement_count: position_statements.len(),
        settlement_statement_count: settlement_statements.len(),
        account_raw_snapshot_path,
        position_raw_snapshot_path,
        settlement_raw_snapshot_path,
        settlement_raw_snapshot_mirror_path,
        summary_path: summary_path.clone(),
        updated_at,
        mutable_execution_started: false,
        notes,
    };
    write_utf8(
        summary_path,
        &funding_private_readonly_snapshot_summary_json(&report),
    )?;
    Ok(report)
}

pub(crate) fn run_portfolio_private_readonly_snapshot_once(
    options: PortfolioPrivateReadonlySnapshotOptions,
) -> RuntimeResult<PortfolioPrivateReadonlySnapshotReport> {
    #[cfg(feature = "live-exec")]
    {
        run_portfolio_private_readonly_snapshot_once_live(&options)
    }
    #[cfg(not(feature = "live-exec"))]
    {
        let _ = options;
        Err(RuntimeError::UnsafeConfig {
            message: "当前 arb-runtime 未使用 live-exec feature 构建，拒绝读取交易所私有账户接口"
                .to_owned(),
        })
    }
}

#[cfg(feature = "live-exec")]
fn run_portfolio_private_readonly_snapshot_once_live(
    options: &PortfolioPrivateReadonlySnapshotOptions,
) -> RuntimeResult<PortfolioPrivateReadonlySnapshotReport> {
    validate_portfolio_private_readonly_snapshot_options(options)?;
    let output_dir = options
        .output_dir
        .clone()
        .unwrap_or_else(|| PathBuf::from(PORTFOLIO_PRIVATE_READONLY_SNAPSHOT_DEFAULT_OUT));
    fs::create_dir_all(&output_dir).map_err(|error| RuntimeError::Io {
        path: output_dir.clone(),
        message: error.to_string(),
    })?;

    let mut account_statements = Vec::new();
    let mut position_statements = Vec::new();
    let mut source_errors = Vec::new();
    let probe_symbol = "BTCUSDT";
    let fetch_options = FundingArbPrivateReadonlySnapshotOnceOptions {
        config_path: options.config_path.clone(),
        snapshot_path: PathBuf::from("portfolio-private-readonly-snapshot"),
        pair_id: "portfolio-private-readonly".to_owned(),
        output_dir: None,
        funding_settlement_raw_snapshot_path: None,
        hyperliquid_user: options.hyperliquid_user.clone(),
        aster_user: options.aster_user.clone(),
        aster_signer: options.aster_signer.clone(),
        aster_signer_cmd_env: options.aster_signer_cmd_env.clone(),
    };

    for venue_family in &options.venue_families {
        let family = normalize_venue_family(venue_family);
        let leg = match funding_arb_leg_config(&family, probe_symbol) {
            Ok(leg) => leg,
            Err(error) => {
                source_errors.push(format!("{family}: {error}"));
                continue;
            }
        };
        match fetch_funding_arb_private_readonly_venue_snapshots(
            &family,
            probe_symbol,
            &leg.account_id,
            &leg.venue_id,
            &leg.instrument_id,
            &fetch_options,
        ) {
            Ok(fetched) => {
                account_statements.push(FundingPrivateReadonlyRawStatement {
                    venue_family: family.clone(),
                    account_id: leg.account_id.clone(),
                    payload_json: fetched.account_payload_json,
                });
                position_statements.push(FundingPrivateReadonlyRawStatement {
                    venue_family: family,
                    account_id: leg.account_id,
                    payload_json: fetched.position_payload_json,
                });
            }
            Err(error) => {
                source_errors.push(format!("{family}: private read-only fetch failed: {error}"));
            }
        }
    }

    for source_error in &mut source_errors {
        *source_error = truncate_for_json(source_error, ERROR_LOG_SOURCE_ERROR_CHAR_LIMIT);
    }
    let status = if source_errors.is_empty() {
        "complete"
    } else if account_statements.is_empty() && position_statements.is_empty() {
        "missing"
    } else {
        "degraded"
    };
    let updated_at = current_utc_timestamp()?.to_string();
    let account_raw_json = funding_private_readonly_raw_snapshot_json_with_status(
        &updated_at,
        status,
        &account_statements,
        &source_errors,
    );
    let position_raw_json = funding_private_readonly_raw_snapshot_json_with_status(
        &updated_at,
        status,
        &position_statements,
        &source_errors,
    );
    parse_funding_private_raw_snapshot_json(
        &account_raw_json,
        "portfolio private readonly account raw snapshot",
    )?;
    parse_funding_private_raw_snapshot_json(
        &position_raw_json,
        "portfolio private readonly position raw snapshot",
    )?;

    let account_raw_snapshot_path =
        output_dir.join("funding_arb_private_account_raw_snapshot.json");
    let position_raw_snapshot_path =
        output_dir.join("funding_arb_private_position_raw_snapshot.json");
    let summary_path = output_dir.join("portfolio_private_readonly_summary.json");
    write_utf8(account_raw_snapshot_path.clone(), &account_raw_json)?;
    write_utf8(position_raw_snapshot_path.clone(), &position_raw_json)?;

    let report = PortfolioPrivateReadonlySnapshotReport {
        venue_families: options
            .venue_families
            .iter()
            .map(|venue| normalize_venue_family(venue))
            .collect(),
        account_statement_count: account_statements.len(),
        position_statement_count: position_statements.len(),
        account_raw_snapshot_path,
        position_raw_snapshot_path,
        summary_path: summary_path.clone(),
        updated_at,
        source_errors,
        mutable_execution_started: false,
    };
    write_utf8(
        summary_path,
        &portfolio_private_readonly_snapshot_summary_json(&report),
    )?;
    Ok(report)
}

#[cfg(feature = "live-exec")]
fn portfolio_private_readonly_snapshot_summary_json(
    report: &PortfolioPrivateReadonlySnapshotReport,
) -> String {
    format!(
        "{{\"account_raw_snapshot_path\":{},\"account_statement_count\":{},\"mutable_execution_started\":{},\"position_raw_snapshot_path\":{},\"position_statement_count\":{},\"source_error_count\":{},\"source_errors\":{},\"summary_path\":{},\"updated_at\":{},\"venue_families\":{}}}",
        json_string(&report.account_raw_snapshot_path.display().to_string()),
        report.account_statement_count,
        report.mutable_execution_started,
        json_string(&report.position_raw_snapshot_path.display().to_string()),
        report.position_statement_count,
        report.source_errors.len(),
        json_string_array(&report.source_errors),
        json_string(&report.summary_path.display().to_string()),
        json_string(&report.updated_at),
        json_string_array(&report.venue_families)
    )
}

#[cfg(feature = "live-exec")]
fn run_wallet_signer_preflight_live(
    options: WalletSignerPreflightOptions,
) -> RuntimeResult<WalletSignerPreflightReport> {
    validate_wallet_signer_preflight_options(&options)?;
    let mut blocking_reasons = Vec::new();

    let aster_status = if options.skip_aster {
        "Skipped".to_owned()
    } else {
        match run_aster_wallet_signer_preflight(&options) {
            Ok(()) => "Passed".to_owned(),
            Err(error) => {
                blocking_reasons.push(format!("Aster signer preflight blocked: {error}"));
                "Blocked".to_owned()
            }
        }
    };

    let should_check_hyperliquid = options.check_hyperliquid
        || options.hyperliquid_agent.is_some()
        || first_non_empty_env(HYPERLIQUID_AGENT_ENV_NAMES).is_some()
        || first_non_empty_env(&[
            "HYPERLIQUID_AGENT_PRIVATE_KEY",
            "HYPERLIQUID_SIGNER_PRIVATE_KEY",
            "HYPERLIQUID_SIGNER_PRIVATE",
            "HYPERLIQUID_PRIVATE_KEY",
        ])
        .is_some();
    let hyperliquid_status = if !should_check_hyperliquid {
        "Skipped".to_owned()
    } else {
        match run_hyperliquid_wallet_signer_preflight(&options) {
            Ok(()) => "Passed".to_owned(),
            Err(error) => {
                blocking_reasons.push(format!("Hyperliquid signer preflight blocked: {error}"));
                "Blocked".to_owned()
            }
        }
    };

    let checked_at = current_utc_timestamp()?.to_string();
    let summary_path = if let Some(output_dir) = &options.output_dir {
        fs::create_dir_all(output_dir).map_err(|error| RuntimeError::Io {
            path: output_dir.clone(),
            message: error.to_string(),
        })?;
        let path = output_dir.join("wallet_signer_preflight_summary.json");
        let report = WalletSignerPreflightReport {
            checked_at: checked_at.clone(),
            aster_status: aster_status.clone(),
            hyperliquid_status: hyperliquid_status.clone(),
            blocking_reasons: blocking_reasons.clone(),
            summary_path: Some(path.clone()),
            mutable_execution_started: false,
        };
        write_utf8(path.clone(), &wallet_signer_preflight_summary_json(&report))?;
        Some(path)
    } else {
        None
    };

    Ok(WalletSignerPreflightReport {
        checked_at,
        aster_status,
        hyperliquid_status,
        blocking_reasons,
        summary_path,
        mutable_execution_started: false,
    })
}
