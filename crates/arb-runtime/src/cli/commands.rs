use std::path::PathBuf;
use std::process::Command;

use crate::*;

use super::args::*;
use super::help::help_text;

/// CLI 入口。
pub(crate) fn main_cli_impl() -> i32 {
    match run_cli(std::env::args().skip(1).collect()) {
        Ok(message) => {
            println!("{message}");
            0
        }
        Err(error) => {
            eprintln!("arb-runtime failed: {error}");
            1
        }
    }
}

pub(crate) fn run_unified_runtime(
    options: UnifiedRuntimeCliOptions,
) -> RuntimeResult<UnifiedRuntimeReport> {
    let repo_root = workspace_root();
    let script_path = repo_root
        .join("scripts")
        .join("start-basis-opportunity-observer.sh");
    let strategies = std::env::var("BASIS_OBSERVER_STRATEGIES")
        .unwrap_or_else(|_| UNIFIED_RUNTIME_DEFAULT_STRATEGIES.to_owned());
    let monitors = std::env::var("BASIS_OBSERVER_MONITORS")
        .unwrap_or_else(|_| UNIFIED_RUNTIME_DEFAULT_MONITORS.to_owned());
    let status = Command::new("bash")
        .arg(&script_path)
        .current_dir(&repo_root)
        .env("BASIS_OBSERVER_ROOT", &options.output_dir)
        .env("BASIS_OBSERVER_CONFIG", &options.config_path)
        .env(
            "BASIS_OBSERVER_INTERVAL_SECS",
            options.interval_secs.to_string(),
        )
        .env(
            "BASIS_OBSERVER_MIN_NET_BPS",
            options.min_net_bps.to_string(),
        )
        .env("BASIS_OBSERVER_STRATEGIES", strategies)
        .env("BASIS_OBSERVER_MONITORS", monitors)
        .env("BASIS_OBSERVER_FOREGROUND", "1")
        .env(
            "BASIS_OBSERVER_EXECUTE_LIVE",
            if options.mode.execute_live() {
                "1"
            } else {
                "0"
            },
        )
        .env(
            "BASIS_OBSERVER_LIVE_ACK",
            if options.mode.execute_live() {
                "1"
            } else {
                "0"
            },
        )
        .env(ARB_RUNTIME_LEGACY_COMMANDS_ENV, "1")
        .status()
        .map_err(|error| RuntimeError::Io {
            path: script_path.clone(),
            message: error.to_string(),
        })?;
    let exit_status = process_exit_status_label(&status);
    if !status.success() {
        return Err(RuntimeError::Module {
            module: "arb-runtime",
            message: format!(
                "{} mode exited with {exit_status}; artifacts may be under {}",
                options.mode.execution_mode_name(),
                options.output_dir.display()
            ),
        });
    }

    Ok(UnifiedRuntimeReport {
        mode: options.mode,
        output_dir: options.output_dir,
        exit_status,
    })
}

pub(crate) fn process_exit_status_label(status: &std::process::ExitStatus) -> String {
    status
        .code()
        .map(|code| format!("exit_code={code}"))
        .unwrap_or_else(|| "terminated_by_signal".to_owned())
}

pub(crate) fn legacy_cli_commands_enabled() -> bool {
    std::env::var(ARB_RUNTIME_LEGACY_COMMANDS_ENV)
        .ok()
        .map(|value| {
            value == "1" || value.eq_ignore_ascii_case("true") || value.eq_ignore_ascii_case("yes")
        })
        .unwrap_or(false)
}

pub(crate) fn unknown_public_command_error(command: &str) -> RuntimeError {
    cli_arg_error(format!(
        "unknown command `{command}`; supported commands: live, paper, portfolio-dashboard"
    ))
}

pub(crate) fn run_cli(args: Vec<String>) -> RuntimeResult<String> {
    if args.is_empty() || args.iter().any(|arg| arg == "-h" || arg == "--help") {
        return Ok(help_text());
    }
    if let Some(mode) = UnifiedRuntimeMode::from_command(&args[0]) {
        let options = parse_unified_runtime_args(mode, &args[1..])?;
        let report = run_unified_runtime(options)?;
        return Ok(format!(
            "ok: arb-runtime {} exited; status={}; wrote artifacts to {}",
            report.mode.label(),
            report.exit_status,
            report.output_dir.display()
        ));
    }
    if args[0] == "portfolio-dashboard" {
        return run_portfolio_dashboard_command(&args[1..]);
    }
    if !legacy_cli_commands_enabled() {
        return Err(unknown_public_command_error(&args[0]));
    }
    if args[0] == "live-market-sim" {
        let options = parse_live_market_sim_args(&args[1..])?;
        let report = run_live_market_simulation(
            &options.fixture_root,
            &options.symbol,
            options.output_dir.clone(),
        )?;
        let output_note = report
            .output_dir
            .as_ref()
            .map(|path| format!("; wrote artifacts to {}", path.display()))
            .unwrap_or_else(|| {
                "; no artifacts written, pass --out <dir> to persist them".to_owned()
            });
        return Ok(format!(
            "ok: fetched live public market data for {}; execution_mode=Simulated; mutable_execution_started=false; execution_reports={}; incidents={}{}",
            report.symbol,
            count_jsonl_records(&report.artifacts.execution_reports_jsonl),
            count_jsonl_records(&report.artifacts.incidents_jsonl),
            output_note
        ));
    }
    if args[0] == "binance-basis-scan" {
        let options = parse_binance_basis_scan_args(&args[1..])?;
        let report = run_binance_basis_scan(&options.symbol, options.output_dir.clone())?;
        let output_note = report
            .output_dir
            .as_ref()
            .map(|path| format!("; wrote artifacts to {}", path.display()))
            .unwrap_or_else(|| {
                "; no artifacts written, pass --out <dir> to persist them".to_owned()
            });
        let candidate_count = count_jsonl_records(&report.candidate_transitions_jsonl);
        let outcome = report
            .rejection_reason
            .as_ref()
            .map(|reason| {
                format!(
                    "rejected; reason={reason}; detail={}",
                    report.rejection_detail.as_deref().unwrap_or("")
                )
            })
            .unwrap_or_else(|| "candidate=true".to_owned());
        return Ok(format!(
            "ok: fetched Binance public spot/perp basis data for {}; {}; candidate_transitions={}; mutable_execution_started=false{}",
            report.symbol, outcome, candidate_count, output_note
        ));
    }
    if args[0] == "binance-basis-pipeline" {
        let options = parse_binance_basis_pipeline_args(&args[1..])?;
        let report = run_binance_basis_pipeline(&options.symbol, options.output_dir.clone())?;
        let output_note = report
            .output_dir
            .as_ref()
            .map(|path| format!("; wrote artifacts to {}", path.display()))
            .unwrap_or_else(|| {
                "; no artifacts written, pass --out <dir> to persist them".to_owned()
            });
        return Ok(format!(
            "ok: ran Binance public spot/perp basis through simulated pipeline for {}; candidate_transitions={}; risk_decisions={}; execution_reports={}; incidents={}; mutable_execution_started=false{}",
            report.symbol,
            count_jsonl_records(&report.artifacts.candidate_transitions_jsonl),
            count_jsonl_records(&report.artifacts.risk_decisions_jsonl),
            count_jsonl_records(&report.artifacts.execution_reports_jsonl),
            count_jsonl_records(&report.artifacts.incidents_jsonl),
            output_note
        ));
    }
    if args[0] == "bybit-basis-scan" {
        let options = parse_bybit_basis_scan_args(&args[1..])?;
        let report = run_bybit_basis_scan(&options.symbol, options.output_dir.clone())?;
        let output_note = report
            .output_dir
            .as_ref()
            .map(|path| format!("; wrote artifacts to {}", path.display()))
            .unwrap_or_else(|| {
                "; no artifacts written, pass --out <dir> to persist them".to_owned()
            });
        let candidate_count = count_jsonl_records(&report.candidate_transitions_jsonl);
        let outcome = report
            .rejection_reason
            .as_ref()
            .map(|reason| {
                format!(
                    "rejected; reason={reason}; detail={}",
                    report.rejection_detail.as_deref().unwrap_or("")
                )
            })
            .unwrap_or_else(|| "candidate=true".to_owned());
        return Ok(format!(
            "ok: fetched Bybit public spot/linear-perp basis data for {}; {}; candidate_transitions={}; mutable_execution_started=false{}",
            report.symbol, outcome, candidate_count, output_note
        ));
    }
    if args[0] == "bybit-basis-pipeline" {
        let options = parse_bybit_basis_pipeline_args(&args[1..])?;
        let report = run_bybit_basis_pipeline(&options.symbol, options.output_dir.clone())?;
        let output_note = report
            .output_dir
            .as_ref()
            .map(|path| format!("; wrote artifacts to {}", path.display()))
            .unwrap_or_else(|| {
                "; no artifacts written, pass --out <dir> to persist them".to_owned()
            });
        return Ok(format!(
            "ok: ran Bybit public spot/linear-perp basis through simulated pipeline for {}; candidate_transitions={}; risk_decisions={}; execution_reports={}; incidents={}; mutable_execution_started=false{}",
            report.symbol,
            count_jsonl_records(&report.artifacts.candidate_transitions_jsonl),
            count_jsonl_records(&report.artifacts.risk_decisions_jsonl),
            count_jsonl_records(&report.artifacts.execution_reports_jsonl),
            count_jsonl_records(&report.artifacts.incidents_jsonl),
            output_note
        ));
    }
    if args[0] == "binance-guarded-live-preview" {
        let options = parse_binance_guarded_live_preview_args(&args[1..])?;
        let report = run_binance_guarded_live_preview(
            &options.market_artifacts_dir,
            options.output_dir.clone(),
            options.decision_request,
        )?;
        let output_note = report
            .output_dir
            .as_ref()
            .map(|path| format!("; wrote artifacts to {}", path.display()))
            .unwrap_or_else(|| {
                "; no artifacts written, pass --out <dir> to persist them".to_owned()
            });
        let approval_status = report
            .approval_status
            .as_deref()
            .unwrap_or("PendingManualApproval");
        return Ok(format!(
            "ok: generated Binance BTCUSDT GuardedLivePersonal plan preview; plan_hash={}; dispatchable_before_approval={}; approval_records={}; approval_status={}; mutable_execution_started=false{}",
            report.plan_hash,
            report.dispatchable_before_approval,
            report.approval_record_count,
            approval_status,
            output_note
        ));
    }
    if args[0] == "binance-guarded-live-gate-release-preview" {
        let options = parse_binance_manual_gate_release_preview_args(&args[1..])?;
        let report = run_binance_manual_gate_release_preview(
            &options.preview_dir,
            options.output_dir.clone(),
        )?;
        let output_note = report
            .output_dir
            .as_ref()
            .map(|path| format!("; wrote artifacts to {}", path.display()))
            .unwrap_or_else(|| {
                "; no artifacts written, pass --out <dir> to persist them".to_owned()
            });
        return Ok(format!(
            "ok: generated Binance BTCUSDT manual gate release preview; plan_hash={}; approval_event_id={}; released_manual_gate={}; dependent_transitions={}; dispatchable_after_release={}; mutable_execution_started=false{}",
            report.plan_hash,
            report.approval_event_id,
            report.released_manual_gate,
            report.dependent_transition_count,
            report.dispatchable_after_release,
            output_note
        ));
    }
    if args[0] == "binance-guarded-live-pre-dispatch-dry-run" {
        let options = parse_binance_pre_dispatch_dry_run_args(&args[1..])?;
        let report = run_binance_pre_dispatch_dry_run(
            &options.preview_dir,
            &options.config_path,
            options.output_dir.clone(),
        )?;
        let output_note = report
            .output_dir
            .as_ref()
            .map(|path| format!("; wrote artifacts to {}", path.display()))
            .unwrap_or_else(|| {
                "; no artifacts written, pass --out <dir> to persist them".to_owned()
            });
        return Ok(format!(
            "ok: ran Binance BTCUSDT pre-dispatch dry run; plan_hash={}; approval_event_id={}; manual_gate_released={}; dispatch_allowed={}; blocking_reasons={}; mutable_execution_started=false{}",
            report.plan_hash,
            report.approval_event_id,
            report.manual_gate_released,
            report.dispatch_allowed,
            report.blocking_reasons.len(),
            output_note
        ));
    }
    if args[0] == "binance-guarded-live-dispatch" {
        let options = parse_binance_guarded_live_dispatch_args(&args[1..])?;
        let report = run_binance_guarded_live_dispatch(options)?;
        let output_note = report
            .output_dir
            .as_ref()
            .map(|path| format!("; wrote artifacts to {}", path.display()))
            .unwrap_or_else(|| {
                "; no artifacts written, pass --out <dir> to persist them".to_owned()
            });
        return Ok(format!(
            "ok: Binance BTCUSDT GuardedLive dispatch completed; plan_hash={}; approval_event_id={}; dispatch_allowed={}; submitted_receipts={}; private_confirmations={}; execution_report_status={}; blocking_reasons={}{}",
            report.plan_hash,
            report.approval_event_id,
            report.dispatch_allowed,
            report.submitted_receipt_count,
            report.private_confirmation_count,
            report
                .execution_report_status
                .as_deref()
                .unwrap_or("none"),
            report.blocking_reasons.len(),
            output_note
        ));
    }
    if args[0] == "binance-basis-live-stack" {
        let options = parse_binance_basis_live_stack_args(&args[1..])?;
        let report = run_binance_basis_live_stack(options)?;
        return Ok(format!(
            "ok: Binance basis live stack completed; phase={}; readiness_ok={}; resident_exit_status={}; spot_monitor_exit_status={}; perp_monitor_exit_status={}; halt_reason={}; wrote artifacts to {}",
            report.phase,
            report.readiness_ok,
            report.resident_exit_status.as_deref().unwrap_or("none"),
            report.spot_monitor_exit_status.as_deref().unwrap_or("none"),
            report.perp_monitor_exit_status.as_deref().unwrap_or("none"),
            report.halt_reason.as_deref().unwrap_or("none"),
            report.output_dir.display()
        ));
    }
    if args[0] == "binance-basis-resident-live" {
        let options = parse_binance_basis_resident_live_args(&args[1..])?;
        let report = run_binance_basis_resident_live(options)?;
        let output_note = report
            .output_dir
            .as_ref()
            .map(|path| format!("; wrote artifacts to {}", path.display()))
            .unwrap_or_else(|| "; no artifacts written".to_owned());
        return Ok(format!(
            "ok: Binance basis resident live completed; phase={}; cycles={}; last_net_bps={}; entry_dispatch_attempted={}; exit_dispatch_attempted={}; open_positions={}; live_entries={}; total_open_notional_usdt={}; position_state={}; halt_reason={}{}",
            report.phase,
            report.cycles,
            report
                .last_net_bps
                .map(|value| value.to_string())
                .unwrap_or_else(|| "none".to_owned()),
            report.entry_dispatch_attempted,
            report.exit_dispatch_attempted,
            report.open_position_count,
            report.live_entry_count,
            report.total_open_notional_usdt,
            report
                .position_state_path
                .as_ref()
                .map(|path| path.display().to_string())
                .unwrap_or_else(|| "none".to_owned()),
            report.halt_reason.as_deref().unwrap_or("none"),
            output_note
        ));
    }
    if args[0] == "multi-venue-basis-resident-live" {
        let options = parse_multi_venue_basis_resident_live_args(&args[1..])?;
        let report = run_multi_venue_basis_resident_live(options)?;
        let output_note = report
            .output_dir
            .as_ref()
            .map(|path| format!("; wrote artifacts to {}", path.display()))
            .unwrap_or_else(|| "; no artifacts written".to_owned());
        return Ok(format!(
            "ok: multi-venue basis resident live completed; phase={}; cycles={}; venues={}; entry_dispatch_attempted={}; exit_dispatch_attempted={}; open_positions={}; live_entries={}; total_open_notional_usdt={}; halt_reason={}{}",
            report.phase,
            report.cycles,
            report.venue_count,
            report.entry_dispatch_attempted,
            report.exit_dispatch_attempted,
            report.open_position_count,
            report.live_entry_count,
            report.total_open_notional_usdt,
            report.halt_reason.as_deref().unwrap_or("none"),
            output_note
        ));
    }
    if args[0] == "multi-venue-basis-live-stack" {
        let options = parse_multi_venue_basis_live_stack_args(&args[1..])?;
        let report = run_multi_venue_basis_live_stack(options)?;
        return Ok(format!(
            "ok: multi-venue basis live stack completed; phase={}; readiness_ok={}; resident_exit_status={}; monitor_exit_statuses={}; halt_reason={}; wrote artifacts to {}",
            report.phase,
            report.readiness_ok,
            report.resident_exit_status.as_deref().unwrap_or("none"),
            report.monitor_exit_statuses.len(),
            report.halt_reason.as_deref().unwrap_or("none"),
            report.output_dir.display()
        ));
    }
    if args[0] == "basis-exit-supervisor" {
        let options = parse_basis_exit_supervisor_args(&args[1..])?;
        let report = run_basis_exit_supervisor(options)?;
        let output_note = report
            .output_dir
            .as_ref()
            .map(|path| format!("; wrote artifacts to {}", path.display()))
            .unwrap_or_else(|| "; no artifacts written".to_owned());
        return Ok(format!(
            "ok: basis exit supervisor completed; symbol={}; decision={}; reasons={}; dispatch_attempted={}; submitted_receipts={}; private_confirmations={}; residual_risk={}{}",
            report.symbol,
            report.decision,
            report.reason_codes.len(),
            report.dispatch_attempted,
            report.submitted_receipt_count,
            report.private_confirmation_count,
            report.residual_risk.as_deref().unwrap_or("none"),
            output_note
        ));
    }
    if args[0] == "binance-wss-book-ticker" {
        let options = parse_binance_wss_book_ticker_args(&args[1..])?;
        let once = options.once;
        let bind_addr = options.bind_addr.clone();
        if once && !is_binance_wss_all_symbols_scope(&options.symbol) {
            let report = run_binance_wss_book_ticker_probe(options)?;
            return Ok(format!(
                "ok: Binance public WSS bookTicker market={} symbol={} updates={} fail_closed={} status={} bid={} ask={} stream={}; mutable_execution_started=false",
                report.market.as_str(),
                report.symbol,
                report.update_count,
                report.fail_closed_count,
                report.coordinator_status,
                report.latest_best_bid.as_deref().unwrap_or("null"),
                report.latest_best_ask.as_deref().unwrap_or("null"),
                report.stream_url
            ));
        }
        run_binance_wss_book_ticker_monitor(options)?;
        if once {
            return Ok(format!(
                "ok: ran one Binance public WSS bookTicker all-market monitor cycle; api_bind={bind_addr}; mutable_execution_started=false"
            ));
        }
        return Ok(format!(
            "ok: Binance public WSS bookTicker monitor stopped; api_bind={bind_addr}; mutable_execution_started=false"
        ));
    }
    if args[0] == "bybit-wss-book-ticker" {
        let options = parse_bybit_wss_book_ticker_args(&args[1..])?;
        let once = options.once;
        let bind_addr = options.bind_addr.clone();
        if once && !is_bybit_wss_all_symbols_scope(&options.symbol) {
            let report = run_bybit_wss_book_ticker_probe(options)?;
            return Ok(format!(
                "ok: Bybit public WSS orderbook.1 market={} symbol={} updates={} fail_closed={} status={} bid={} ask={} stream={}; mutable_execution_started=false",
                report.market.as_str(),
                report.symbol,
                report.update_count,
                report.fail_closed_count,
                report.coordinator_status,
                report.latest_best_bid.as_deref().unwrap_or("null"),
                report.latest_best_ask.as_deref().unwrap_or("null"),
                report.stream_url
            ));
        }
        run_bybit_wss_book_ticker_monitor(options)?;
        if once {
            return Ok(format!(
                "ok: ran one Bybit public WSS orderbook.1 monitor cycle; api_bind={bind_addr}; mutable_execution_started=false"
            ));
        }
        return Ok(format!(
            "ok: Bybit public WSS orderbook.1 monitor stopped; api_bind={bind_addr}; mutable_execution_started=false"
        ));
    }
    if args[0] == "okx-wss-book-ticker" {
        let options = parse_okx_wss_book_ticker_args(&args[1..])?;
        let once = options.once;
        let bind_addr = options.bind_addr.clone();
        let market = options.market.as_str();
        let symbol = normalize_okx_wss_symbol_scope(&options.symbol)?;
        run_okx_wss_book_ticker_monitor(options)?;
        if once {
            return Ok(format!(
                "ok: ran one OKX public WSS tickers monitor cycle; market={market}; symbol={symbol}; api_bind={bind_addr}; mutable_execution_started=false"
            ));
        }
        return Ok(format!(
            "ok: OKX public WSS tickers monitor stopped; api_bind={bind_addr}; mutable_execution_started=false"
        ));
    }
    if args[0] == "bitget-wss-book-ticker" {
        let options = parse_bitget_wss_book_ticker_args(&args[1..])?;
        let once = options.once;
        let bind_addr = options.bind_addr.clone();
        let market = options.market.as_str();
        let symbol = normalize_bitget_wss_symbol_scope(&options.symbol)?;
        run_bitget_wss_book_ticker_monitor(options)?;
        if once {
            return Ok(format!(
                "ok: ran one Bitget public WSS ticker monitor cycle; market={market}; symbol={symbol}; api_bind={bind_addr}; mutable_execution_started=false"
            ));
        }
        return Ok(format!(
            "ok: Bitget public WSS ticker monitor stopped; api_bind={bind_addr}; mutable_execution_started=false"
        ));
    }
    if args[0] == "aster-wss-book-ticker" {
        let options = parse_aster_wss_book_ticker_args(&args[1..])?;
        let once = options.once;
        let bind_addr = options.bind_addr.clone();
        let market = options.market.as_str();
        let symbol = normalize_aster_wss_symbol_scope(&options.symbol)?;
        run_aster_wss_book_ticker_monitor(options)?;
        if once {
            return Ok(format!(
                "ok: ran one Aster public WSS bookTicker monitor cycle; market={market}; symbol={symbol}; api_bind={bind_addr}; mutable_execution_started=false"
            ));
        }
        return Ok(format!(
            "ok: Aster public WSS bookTicker monitor stopped; api_bind={bind_addr}; mutable_execution_started=false"
        ));
    }
    if args[0] == "hyperliquid-wss-book-ticker" {
        let options = parse_hyperliquid_wss_book_ticker_args(&args[1..])?;
        let once = options.once;
        let bind_addr = options.bind_addr.clone();
        let market = options.market.as_str();
        let symbol = normalize_hyperliquid_wss_symbol_scope(&options.symbol)?;
        run_hyperliquid_wss_book_ticker_monitor(options)?;
        if once {
            return Ok(format!(
                "ok: ran one Hyperliquid public WSS bbo monitor cycle; market={market}; symbol={symbol}; api_bind={bind_addr}; mutable_execution_started=false"
            ));
        }
        return Ok(format!(
            "ok: Hyperliquid public WSS bbo monitor stopped; api_bind={bind_addr}; mutable_execution_started=false"
        ));
    }
    if args[0] == "binance-basis-monitor" {
        let options = parse_binance_basis_monitor_args(&args[1..])?;
        let once = options.once;
        let bind_addr = options.bind_addr.clone();
        let output_dir = options.output_dir.clone();
        run_binance_basis_monitor(options)?;
        if once {
            let output_note = output_dir
                .as_ref()
                .map(|path| format!("; wrote snapshot to {}", path.display()))
                .unwrap_or_else(|| "; no snapshot written, pass --out <dir>".to_owned());
            return Ok(format!(
                "ok: ran one Binance basis monitor refresh; api_bind={bind_addr}; mutable_execution_started=false{output_note}"
            ));
        }
        return Ok(format!(
            "ok: Binance basis monitor stopped; api_bind={bind_addr}; mutable_execution_started=false"
        ));
    }
    if args[0] == "bybit-basis-monitor" {
        let options = parse_bybit_basis_monitor_args(&args[1..])?;
        let once = options.once;
        let bind_addr = options.bind_addr.clone();
        let output_dir = options.output_dir.clone();
        run_bybit_basis_monitor(options)?;
        if once {
            let output_note = output_dir
                .as_ref()
                .map(|path| format!("; wrote snapshot to {}", path.display()))
                .unwrap_or_else(|| "; no snapshot written, pass --out <dir>".to_owned());
            return Ok(format!(
                "ok: ran one Bybit basis monitor refresh; api_bind={bind_addr}; mutable_execution_started=false{output_note}"
            ));
        }
        return Ok(format!(
            "ok: Bybit basis monitor stopped; api_bind={bind_addr}; mutable_execution_started=false"
        ));
    }
    if args[0] == "okx-basis-monitor" {
        let options = parse_okx_basis_monitor_args(&args[1..])?;
        let once = options.once;
        let bind_addr = options.bind_addr.clone();
        let output_dir = options.output_dir.clone();
        run_okx_basis_monitor(options)?;
        if once {
            let output_note = output_dir
                .as_ref()
                .map(|path| format!("; wrote snapshot to {}", path.display()))
                .unwrap_or_else(|| "; no snapshot written, pass --out <dir>".to_owned());
            return Ok(format!(
                "ok: ran one OKX basis monitor refresh; api_bind={bind_addr}; mutable_execution_started=false{output_note}"
            ));
        }
        return Ok(format!(
            "ok: OKX basis monitor stopped; api_bind={bind_addr}; mutable_execution_started=false"
        ));
    }
    if args[0] == "bitget-basis-monitor" {
        let options = parse_bitget_basis_monitor_args(&args[1..])?;
        let once = options.once;
        let bind_addr = options.bind_addr.clone();
        let output_dir = options.output_dir.clone();
        run_bitget_basis_monitor(options)?;
        if once {
            let output_note = output_dir
                .as_ref()
                .map(|path| format!("; wrote snapshot to {}", path.display()))
                .unwrap_or_else(|| "; no snapshot written, pass --out <dir>".to_owned());
            return Ok(format!(
                "ok: ran one Bitget basis monitor refresh; api_bind={bind_addr}; mutable_execution_started=false{output_note}"
            ));
        }
        return Ok(format!(
            "ok: Bitget basis monitor stopped; api_bind={bind_addr}; mutable_execution_started=false"
        ));
    }
    if args[0] == "hyperliquid-basis-monitor" {
        let options = parse_hyperliquid_basis_monitor_args(&args[1..])?;
        let once = options.once;
        let bind_addr = options.bind_addr.clone();
        let output_dir = options.output_dir.clone();
        run_hyperliquid_basis_monitor(options)?;
        if once {
            let output_note = output_dir
                .as_ref()
                .map(|path| format!("; wrote snapshot to {}", path.display()))
                .unwrap_or_else(|| "; no snapshot written, pass --out <dir>".to_owned());
            return Ok(format!(
                "ok: ran one Hyperliquid basis monitor refresh; api_bind={bind_addr}; mutable_execution_started=false{output_note}"
            ));
        }
        return Ok(format!(
            "ok: Hyperliquid basis monitor stopped; api_bind={bind_addr}; mutable_execution_started=false"
        ));
    }
    if args[0] == "aster-basis-monitor" {
        let options = parse_aster_basis_monitor_args(&args[1..])?;
        let once = options.once;
        let bind_addr = options.bind_addr.clone();
        let output_dir = options.output_dir.clone();
        run_aster_basis_monitor(options)?;
        if once {
            let output_note = output_dir
                .as_ref()
                .map(|path| format!("; wrote snapshot to {}", path.display()))
                .unwrap_or_else(|| "; no snapshot written, pass --out <dir>".to_owned());
            return Ok(format!(
                "ok: ran one Aster basis monitor refresh; api_bind={bind_addr}; mutable_execution_started=false{output_note}"
            ));
        }
        return Ok(format!(
            "ok: Aster basis monitor stopped; api_bind={bind_addr}; mutable_execution_started=false"
        ));
    }
    if args[0] == "funding-arb-monitor" {
        let options = parse_funding_arb_monitor_args(&args[1..])?;
        let once = options.once;
        let bind_addr = options.bind_addr.clone();
        let output_dir = options.output_dir.clone();
        run_funding_arb_monitor(options)?;
        if once {
            let output_note = output_dir
                .as_ref()
                .map(|path| format!("; wrote snapshot to {}", path.display()))
                .unwrap_or_else(|| "; no snapshot written, pass --out <dir>".to_owned());
            return Ok(format!(
                "ok: ran one funding arb monitor refresh; api_bind={bind_addr}; mutable_execution_started=false{output_note}"
            ));
        }
        return Ok(format!(
            "ok: funding arb monitor stopped; api_bind={bind_addr}; mutable_execution_started=false"
        ));
    }
    if args[0] == "portfolio-dashboard" {
        return run_portfolio_dashboard_command(&args[1..]);
    }
    if args[0] == "wallet-signer-preflight" {
        let options = parse_wallet_signer_preflight_args(&args[1..])?;
        let output_dir = options.output_dir.clone();
        let report = run_wallet_signer_preflight(options)?;
        let output_note = output_dir
            .as_ref()
            .map(|_| {
                report
                    .summary_path
                    .as_ref()
                    .map(|path| format!("; wrote summary={}", path.display()))
                    .unwrap_or_else(|| "; no summary written".to_owned())
            })
            .unwrap_or_else(|| "; no summary written, pass --out <dir>".to_owned());
        return Ok(format!(
            "ok: wallet signer preflight completed; aster_status={}; hyperliquid_status={}; blocking_reasons={}; mutable_execution_started=false{}",
            report.aster_status,
            report.hyperliquid_status,
            report.blocking_reasons.len(),
            output_note
        ));
    }
    if args[0] == "funding-arb-private-readonly-snapshot-once" {
        let options = parse_funding_arb_private_readonly_snapshot_once_args(&args[1..])?;
        let pair_id = options.pair_id.clone();
        let report = run_funding_arb_private_readonly_snapshot_once(options)?;
        return Ok(format!(
            "ok: funding arb private read-only snapshot completed; pair_id={pair_id}; symbol={}; venues={}; account_statements={}; position_statements={}; settlement_statements={}; mutable_execution_started=false; wrote account_raw_snapshot={}; position_raw_snapshot={}; settlement_raw_snapshot={}; summary={}",
            report.symbol,
            report.venue_families.join(","),
            report.account_statement_count,
            report.position_statement_count,
            report.settlement_statement_count,
            report.account_raw_snapshot_path.display(),
            report.position_raw_snapshot_path.display(),
            report.settlement_raw_snapshot_path.display(),
            report.summary_path.display()
        ));
    }
    if args[0] == "portfolio-private-readonly-snapshot"
        || args[0] == "portfolio-private-readonly-snapshot-once"
    {
        let force_once = args[0] == "portfolio-private-readonly-snapshot-once";
        let options = parse_portfolio_private_readonly_snapshot_args(&args[1..], force_once)?;
        let report = run_portfolio_private_readonly_snapshot(options)?;
        return Ok(format!(
            "ok: portfolio private read-only snapshot completed; venues={}; account_statements={}; position_statements={}; source_errors={}; mutable_execution_started=false; wrote account_raw_snapshot={}; position_raw_snapshot={}; summary={}",
            report.venue_families.join(","),
            report.account_statement_count,
            report.position_statement_count,
            report.source_errors.len(),
            report.account_raw_snapshot_path.display(),
            report.position_raw_snapshot_path.display(),
            report.summary_path.display()
        ));
    }
    if args[0] == "funding-arb-guarded-dry-run-once" {
        let options = parse_funding_arb_guarded_dry_run_once_args(&args[1..])?;
        let pair_id = options.pair_id.clone();
        let output_dir = options.output_dir.clone();
        let report = run_funding_arb_guarded_dry_run_once(options)?;
        let output_note = output_dir
            .as_ref()
            .map(|path| format!("; wrote artifacts to {}", path.display()))
            .unwrap_or_else(|| {
                "; no artifacts written, pass --out <dir> to persist them".to_owned()
            });
        return Ok(format!(
            "ok: funding arb guarded dry-run completed; pair_id={pair_id}; signal_allowed={}; risk_decision={}; funding_settlement_status={}; funding_settlement_ingestion_status={}; private_account_status={}; private_position_status={}; private_execution_status={}; snapshot_lineage_status={}; venue_execution_capability_status={}; execution_constraints_status={}; exit_readiness_status={}; execution_preflight_status={}; execution_preflight_plan_preview_status={}; live_readiness_status={}; live_readiness_blocking_checks={}; manual_gate_released={}; dispatch_plan_built={}; dispatch_requests={}; live_ready={}; live_blocking_reasons={}; dispatch_attempted={}; mutable_execution_started=false{}",
            report.signal_allowed,
            report.risk_decision,
            report.funding_settlement.status.as_str(),
            report.funding_settlement_ingestion.status.as_str(),
            report.private_accounts.status.as_str(),
            report.private_positions.status.as_str(),
            report.private_execution.status.as_str(),
            report.snapshot_lineage.status.as_str(),
            report.venue_execution_capability.status.as_str(),
            report.execution_constraints.status.as_str(),
            report.exit_readiness.status.as_str(),
            report.execution_preflight.status.as_str(),
            report.execution_preflight.plan_preview_status.as_str(),
            report.live_readiness.status.as_str(),
            report.live_readiness.blocking_check_count,
            report.manual_gate_released,
            report.dispatch_plan_built,
            report.dispatch_request_count,
            report.live_ready,
            report.live_blocking_reasons.len(),
            report.dispatch_attempted,
            output_note
        ));
    }
    if args[0] == "funding-arb-guarded-live-canary-once" {
        let options = parse_funding_arb_guarded_live_canary_once_args(&args[1..])?;
        let pair_id = options.dry_run.pair_id.clone();
        let execute_live = options.execute_live;
        let output_dir = options.dry_run.output_dir.clone();
        let report = run_funding_arb_guarded_live_canary_once(options)?;
        let output_note = output_dir
            .as_ref()
            .map(|path| {
                format!(
                    "; wrote artifacts to {}",
                    path.join("live-canary").display()
                )
            })
            .unwrap_or_else(|| {
                "; no artifacts written, pass --out <dir> to persist them".to_owned()
            });
        return Ok(format!(
            "ok: funding arb guarded live canary completed; pair_id={pair_id}; symbol={}; execute_live={execute_live}; manual_gate_released={}; dispatch_plan_built={}; dispatch_requests={}; dispatch_allowed={}; dispatch_attempted={}; submitted_receipts={}; private_confirmations={}; protection_attempted={}; protection_receipts={}; residual_risk={}; execution_report_status={}; blocking_reasons={}; mutable_execution_started={}{}",
            report.symbol,
            report.manual_gate_released,
            report.dispatch_plan_built,
            report.dispatch_request_count,
            report.dispatch_allowed,
            report.dispatch_attempted,
            report.submitted_receipt_count,
            report.private_confirmation_count,
            report.protection_attempted,
            report.protection_receipt_count,
            report.residual_risk.as_deref().unwrap_or("none"),
            report.execution_report_status.as_deref().unwrap_or("none"),
            report.blocking_reasons.len(),
            report.mutable_execution_started,
            output_note
        ));
    }
    if args[0] == "funding-arb-resident-live" {
        let options = parse_funding_arb_resident_live_args(&args[1..])?;
        let output_dir = options.output_dir.clone();
        let report = run_funding_arb_resident_live(options)?;
        let output_note = output_dir
            .or_else(|| report.output_dir.clone())
            .as_ref()
            .map(|path| format!("; wrote artifacts to {}", path.display()))
            .unwrap_or_else(|| "; no artifacts written".to_owned());
        return Ok(format!(
            "ok: funding arb resident live completed; phase={}; cycles={}; last_pair_id={}; last_symbol={}; last_net_funding_bps={}; dispatch_attempted={}; live_entry_count={}; open_positions={}; closed_positions={}; unknown_positions={}; halt_reason={}; mutable_execution_started={}{}",
            report.phase,
            report.cycles,
            report.last_pair_id.as_deref().unwrap_or("none"),
            report.last_symbol.as_deref().unwrap_or("none"),
            report
                .last_net_funding_bps
                .map(|value| value.to_string())
                .unwrap_or_else(|| "none".to_owned()),
            report.dispatch_attempted,
            report.live_entry_count,
            report.open_position_count,
            report.closed_position_count,
            report.unknown_position_count,
            report.halt_reason.as_deref().unwrap_or("none"),
            report.mutable_execution_started,
            output_note
        ));
    }
    #[cfg(feature = "live-exec")]
    if args[0] == "opportunity-recorder" {
        let options = parse_opportunity_recorder_args(&args[1..])?;
        let report = run_opportunity_recorder(options)?;
        return Ok(format!(
            "ok: opportunity recorder completed; cycles={}; spot_sources={}; funding_arb_enabled={}; mutable_execution_started=false",
            report.cycles, report.spot_source_count, report.funding_arb_enabled
        ));
    }
    if args[0] == "health" {
        let fixture_root = args.get(1).map_or_else(
            || PathBuf::from(DEFAULT_FULL_PIPELINE_FIXTURE),
            PathBuf::from,
        );
        let service = start_runtime_for_fixture(&fixture_root)?;
        let health = service.health();
        return Ok(format!(
            "health: {}; execution_mode={}; kill_switch_triggered={}; mutable_execution_started={}; tasks={}",
            health.status.as_str(),
            health.execution_mode,
            health.kill_switch_triggered,
            health.mutable_execution_started,
            health.tasks.len()
        ));
    }
    if args[0] == "health-config" {
        let Some(config_path) = args.get(1) else {
            return Err(cli_arg_error("health-config requires a config path"));
        };
        if args.len() > 2 {
            return Err(cli_arg_error(
                "health-config accepts exactly one config path",
            ));
        }
        let service = start_runtime_from_config_path(config_path)?;
        let health = service.health();
        return Ok(format!(
            "health-config: {}; execution_mode={}; kill_switch_triggered={}; mutable_execution_started={}; tasks={}",
            health.status.as_str(),
            health.execution_mode,
            health.kill_switch_triggered,
            health.mutable_execution_started,
            health.tasks.len()
        ));
    }

    if args[0] != "replay" {
        return Err(RuntimeError::Module {
            module: "arb-runtime",
            message: format!(
                "unknown command `{}`; supported commands: replay, health, health-config, live-market-sim, binance-basis-scan, binance-basis-pipeline, bybit-basis-scan, bybit-basis-pipeline, binance-guarded-live-preview, binance-guarded-live-gate-release-preview, binance-guarded-live-pre-dispatch-dry-run, binance-guarded-live-dispatch, binance-basis-live-stack, binance-basis-resident-live, multi-venue-basis-resident-live, multi-venue-basis-live-stack, binance-wss-book-ticker, bybit-wss-book-ticker, okx-wss-book-ticker, bitget-wss-book-ticker, aster-wss-book-ticker, hyperliquid-wss-book-ticker, binance-basis-monitor, bybit-basis-monitor, okx-basis-monitor, bitget-basis-monitor, hyperliquid-basis-monitor, aster-basis-monitor, funding-arb-monitor, portfolio-dashboard, wallet-signer-preflight, funding-arb-private-readonly-snapshot-once, portfolio-private-readonly-snapshot, portfolio-private-readonly-snapshot-once, funding-arb-guarded-dry-run-once, funding-arb-guarded-live-canary-once, funding-arb-resident-live, opportunity-recorder",
                args[0]
            ),
        });
    }

    let accept_golden = args.iter().skip(1).any(|arg| arg == "--accept");
    let fixture_root = args
        .iter()
        .skip(1)
        .find(|arg| arg.as_str() != "--accept")
        .map_or_else(
            || PathBuf::from(DEFAULT_FULL_PIPELINE_FIXTURE),
            PathBuf::from,
        );
    let report =
        run_full_pipeline_fixture_with_options(&fixture_root, RuntimeOptions { accept_golden })?;
    let action = if accept_golden { "wrote" } else { "matched" };
    Ok(format!(
        "ok: {action} {} S9-01 artifacts for {}",
        report.comparisons.len(),
        report.fixture_root.display()
    ))
}

fn run_portfolio_dashboard_command(args: &[String]) -> RuntimeResult<String> {
    let options = parse_portfolio_dashboard_args(args)?;
    let once = options.once;
    let bind_addr = options.bind_addr.clone();
    let report = run_portfolio_dashboard(options)?;
    if once {
        return Ok(format!(
            "ok: portfolio dashboard snapshot loaded; status={}; balances={}; positions={}; source_errors={}; api_bind={}; mutable_execution_started=false",
            report.status,
            report.balance_count,
            report.position_count,
            report.source_error_count,
            report.bind_addr
        ));
    }
    Ok(format!(
        "ok: portfolio dashboard stopped; api_bind={bind_addr}; mutable_execution_started=false"
    ))
}
