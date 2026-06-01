use std::io::Read;
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, RwLock};
use std::thread;

use crate::*;

pub(crate) fn start_binance_basis_http_api(
    bind_addr: &str,
    state: Arc<RwLock<BinanceBasisMonitorSnapshot>>,
) -> RuntimeResult<thread::JoinHandle<()>> {
    let listener = TcpListener::bind(bind_addr).map_err(|error| RuntimeError::LiveMarketData {
        message: format!("cannot bind monitor HTTP API on {bind_addr}: {error}"),
    })?;
    let handle = thread::spawn(move || {
        for stream in listener.incoming() {
            match stream {
                Ok(stream) => handle_basis_http_connection(stream, &state),
                Err(error) => eprintln!("binance-basis-monitor api accept failed: {error}"),
            }
        }
    });
    Ok(handle)
}

pub(crate) fn start_bybit_basis_http_api(
    bind_addr: &str,
    state: Arc<RwLock<BybitBasisMonitorSnapshot>>,
) -> RuntimeResult<thread::JoinHandle<()>> {
    let listener = TcpListener::bind(bind_addr).map_err(|error| RuntimeError::LiveMarketData {
        message: format!("cannot bind Bybit monitor HTTP API on {bind_addr}: {error}"),
    })?;
    let handle = thread::spawn(move || {
        for stream in listener.incoming() {
            match stream {
                Ok(stream) => handle_bybit_basis_http_connection(stream, &state),
                Err(error) => eprintln!("bybit-basis-monitor api accept failed: {error}"),
            }
        }
    });
    Ok(handle)
}

pub(crate) fn start_okx_basis_http_api(
    bind_addr: &str,
    state: Arc<RwLock<OkxBasisMonitorSnapshot>>,
) -> RuntimeResult<thread::JoinHandle<()>> {
    let listener = TcpListener::bind(bind_addr).map_err(|error| RuntimeError::LiveMarketData {
        message: format!("cannot bind OKX monitor HTTP API on {bind_addr}: {error}"),
    })?;
    let handle = thread::spawn(move || {
        for stream in listener.incoming() {
            match stream {
                Ok(stream) => handle_okx_basis_http_connection(stream, &state),
                Err(error) => eprintln!("okx-basis-monitor api accept failed: {error}"),
            }
        }
    });
    Ok(handle)
}

pub(crate) fn start_bitget_basis_http_api(
    bind_addr: &str,
    state: Arc<RwLock<BitgetBasisMonitorSnapshot>>,
) -> RuntimeResult<thread::JoinHandle<()>> {
    let listener = TcpListener::bind(bind_addr).map_err(|error| RuntimeError::LiveMarketData {
        message: format!("cannot bind Bitget monitor HTTP API on {bind_addr}: {error}"),
    })?;
    let handle = thread::spawn(move || {
        for stream in listener.incoming() {
            match stream {
                Ok(stream) => handle_bitget_basis_http_connection(stream, &state),
                Err(error) => eprintln!("bitget-basis-monitor api accept failed: {error}"),
            }
        }
    });
    Ok(handle)
}

pub(crate) fn start_hyperliquid_basis_http_api(
    bind_addr: &str,
    state: Arc<RwLock<HyperliquidBasisMonitorSnapshot>>,
) -> RuntimeResult<thread::JoinHandle<()>> {
    let listener = TcpListener::bind(bind_addr).map_err(|error| RuntimeError::LiveMarketData {
        message: format!("cannot bind Hyperliquid monitor HTTP API on {bind_addr}: {error}"),
    })?;
    let handle = thread::spawn(move || {
        for stream in listener.incoming() {
            match stream {
                Ok(stream) => handle_hyperliquid_basis_http_connection(stream, &state),
                Err(error) => eprintln!("hyperliquid-basis-monitor api accept failed: {error}"),
            }
        }
    });
    Ok(handle)
}

pub(crate) fn start_aster_basis_http_api(
    bind_addr: &str,
    state: Arc<RwLock<AsterBasisMonitorSnapshot>>,
) -> RuntimeResult<thread::JoinHandle<()>> {
    let listener = TcpListener::bind(bind_addr).map_err(|error| RuntimeError::LiveMarketData {
        message: format!("cannot bind Aster monitor HTTP API on {bind_addr}: {error}"),
    })?;
    let handle = thread::spawn(move || {
        for stream in listener.incoming() {
            match stream {
                Ok(stream) => handle_aster_basis_http_connection(stream, &state),
                Err(error) => eprintln!("aster-basis-monitor api accept failed: {error}"),
            }
        }
    });
    Ok(handle)
}

pub(crate) fn start_funding_arb_http_api(
    bind_addr: &str,
    state: Arc<RwLock<FundingArbMonitorSnapshot>>,
    context: Arc<FundingArbDashboardContext>,
) -> RuntimeResult<thread::JoinHandle<()>> {
    let listener = TcpListener::bind(bind_addr).map_err(|error| RuntimeError::LiveMarketData {
        message: format!("cannot bind funding arb monitor HTTP API on {bind_addr}: {error}"),
    })?;
    let handle = thread::spawn(move || {
        for stream in listener.incoming() {
            match stream {
                Ok(stream) => handle_funding_arb_http_connection(stream, &state, &context),
                Err(error) => eprintln!("funding-arb-monitor api accept failed: {error}"),
            }
        }
    });
    Ok(handle)
}

#[derive(Clone, Copy, Debug, Default)]
struct MonitorHttpQuery {
    row_limit: Option<usize>,
    summary_only: bool,
}

fn parse_monitor_http_query(path: &str) -> (&str, MonitorHttpQuery) {
    let Some((route, query)) = path.split_once('?') else {
        return (path, MonitorHttpQuery::default());
    };
    let mut parsed = MonitorHttpQuery::default();
    for pair in query.split('&').filter(|pair| !pair.is_empty()) {
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        let key = key.trim();
        let value = value.trim();
        if matches!(key, "summary" | "summaryOnly" | "summary_only") {
            parsed.summary_only = matches!(value, "1" | "true" | "yes" | "y" | "");
        } else if matches!(key, "limit" | "rows" | "row_limit") {
            if matches!(value, "all" | "full" | "debug") {
                parsed.row_limit = None;
            } else if let Ok(limit) = value.parse::<usize>() {
                parsed.row_limit = Some(limit);
            }
        }
    }
    (route, parsed)
}

fn handle_basis_http_connection(
    mut stream: TcpStream,
    state: &Arc<RwLock<BinanceBasisMonitorSnapshot>>,
) {
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
    let (route, query) = parse_monitor_http_query(path);
    if method != "GET" {
        let _ = write_http_json(&mut stream, 405, "{\"error\":\"method_not_allowed\"}");
        return;
    }
    if route == "/" || route == "/dashboard" {
        let _ = write_http_json(
            &mut stream,
            410,
            &static_dashboard_gone_json("Binance basis"),
        );
        return;
    }

    let snapshot = state.read().expect("monitor state lock poisoned");
    let (status, body) = if route == "/health" {
        (
            200,
            format!(
                "{{\"status\":{},\"updated_at\":{}}}",
                json_string(&snapshot.status),
                json_string(&snapshot.updated_at)
            ),
        )
    } else if route == "/api/basis/status" {
        (
            200,
            snapshot.to_json_limited(query.row_limit, query.summary_only),
        )
    } else if route == "/api/basis/opportunities" {
        (
            200,
            snapshot.opportunities_json_limited(query.row_limit, query.summary_only),
        )
    } else if let Some(symbol) = route.strip_prefix("/api/basis/status/") {
        match snapshot.symbol_json(symbol.trim_matches('/')) {
            Some(row) => (200, row),
            None => (
                404,
                format!(
                    "{{\"error\":\"symbol_not_found\",\"symbol\":{}}}",
                    json_string(symbol.trim_matches('/'))
                ),
            ),
        }
    } else {
        (
            404,
            "{\"error\":\"not_found\",\"paths\":[\"/health\",\"/api/basis/status\",\"/api/basis/opportunities\",\"/api/basis/status/<SYMBOL>\"]}".to_owned(),
        )
    };
    let _ = write_http_json(&mut stream, status, &body);
}

fn handle_bybit_basis_http_connection(
    mut stream: TcpStream,
    state: &Arc<RwLock<BybitBasisMonitorSnapshot>>,
) {
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
    let (route, query) = parse_monitor_http_query(path);
    if method != "GET" {
        let _ = write_http_json(&mut stream, 405, "{\"error\":\"method_not_allowed\"}");
        return;
    }
    if route == "/" || route == "/dashboard" {
        let _ = write_http_json(&mut stream, 410, &static_dashboard_gone_json("Bybit basis"));
        return;
    }

    let snapshot = state.read().expect("monitor state lock poisoned");
    let (status, body) = if route == "/health" {
        (
            200,
            format!(
                "{{\"status\":{},\"updated_at\":{}}}",
                json_string(&snapshot.status),
                json_string(&snapshot.updated_at)
            ),
        )
    } else if route == "/api/bybit-basis/status" {
        (
            200,
            snapshot.to_json_limited(query.row_limit, query.summary_only),
        )
    } else if route == "/api/bybit-basis/opportunities" {
        (
            200,
            snapshot.opportunities_json_limited(query.row_limit, query.summary_only),
        )
    } else if let Some(symbol) = route.strip_prefix("/api/bybit-basis/status/") {
        match snapshot.symbol_json(symbol.trim_matches('/')) {
            Some(row) => (200, row),
            None => (
                404,
                format!(
                    "{{\"error\":\"symbol_not_found\",\"symbol\":{}}}",
                    json_string(symbol.trim_matches('/'))
                ),
            ),
        }
    } else {
        (
            404,
            "{\"error\":\"not_found\",\"paths\":[\"/health\",\"/api/bybit-basis/status\",\"/api/bybit-basis/opportunities\",\"/api/bybit-basis/status/<SYMBOL>\"]}".to_owned(),
        )
    };
    let _ = write_http_json(&mut stream, status, &body);
}

fn handle_okx_basis_http_connection(
    mut stream: TcpStream,
    state: &Arc<RwLock<OkxBasisMonitorSnapshot>>,
) {
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
    let (route, query) = parse_monitor_http_query(path);
    if method != "GET" {
        let _ = write_http_json(&mut stream, 405, "{\"error\":\"method_not_allowed\"}");
        return;
    }
    if route == "/" || route == "/dashboard" {
        let _ = write_http_json(&mut stream, 410, &static_dashboard_gone_json("OKX basis"));
        return;
    }

    let snapshot = state.read().expect("monitor state lock poisoned");
    let (status, body) = if route == "/health" {
        (
            200,
            format!(
                "{{\"status\":{},\"updated_at\":{}}}",
                json_string(&snapshot.status),
                json_string(&snapshot.updated_at)
            ),
        )
    } else if route == "/api/okx-basis/status" {
        (
            200,
            snapshot.to_json_limited(query.row_limit, query.summary_only),
        )
    } else if route == "/api/okx-basis/opportunities" {
        (
            200,
            snapshot.opportunities_json_limited(query.row_limit, query.summary_only),
        )
    } else if let Some(symbol) = route.strip_prefix("/api/okx-basis/status/") {
        match snapshot.symbol_json(symbol.trim_matches('/')) {
            Some(row) => (200, row),
            None => (
                404,
                format!(
                    "{{\"error\":\"symbol_not_found\",\"symbol\":{}}}",
                    json_string(symbol.trim_matches('/'))
                ),
            ),
        }
    } else {
        (
            404,
            "{\"error\":\"not_found\",\"paths\":[\"/health\",\"/api/okx-basis/status\",\"/api/okx-basis/opportunities\",\"/api/okx-basis/status/<SYMBOL>\"]}".to_owned(),
        )
    };
    let _ = write_http_json(&mut stream, status, &body);
}

fn handle_bitget_basis_http_connection(
    mut stream: TcpStream,
    state: &Arc<RwLock<BitgetBasisMonitorSnapshot>>,
) {
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
    let (route, query) = parse_monitor_http_query(path);
    if method != "GET" {
        let _ = write_http_json(&mut stream, 405, "{\"error\":\"method_not_allowed\"}");
        return;
    }
    if route == "/" || route == "/dashboard" {
        let _ = write_http_json(
            &mut stream,
            410,
            &static_dashboard_gone_json("Bitget basis"),
        );
        return;
    }

    let snapshot = state.read().expect("monitor state lock poisoned");
    let (status, body) = if route == "/health" {
        (
            200,
            format!(
                "{{\"status\":{},\"updated_at\":{}}}",
                json_string(&snapshot.status),
                json_string(&snapshot.updated_at)
            ),
        )
    } else if route == "/api/bitget-basis/status" {
        (
            200,
            snapshot.to_json_limited(query.row_limit, query.summary_only),
        )
    } else if route == "/api/bitget-basis/opportunities" {
        (
            200,
            snapshot.opportunities_json_limited(query.row_limit, query.summary_only),
        )
    } else if let Some(symbol) = route.strip_prefix("/api/bitget-basis/status/") {
        match snapshot.symbol_json(symbol.trim_matches('/')) {
            Some(row) => (200, row),
            None => (
                404,
                format!(
                    "{{\"error\":\"symbol_not_found\",\"symbol\":{}}}",
                    json_string(symbol.trim_matches('/'))
                ),
            ),
        }
    } else {
        (
            404,
            "{\"error\":\"not_found\",\"paths\":[\"/health\",\"/api/bitget-basis/status\",\"/api/bitget-basis/opportunities\",\"/api/bitget-basis/status/<SYMBOL>\"]}".to_owned(),
        )
    };
    let _ = write_http_json(&mut stream, status, &body);
}

fn handle_hyperliquid_basis_http_connection(
    mut stream: TcpStream,
    state: &Arc<RwLock<HyperliquidBasisMonitorSnapshot>>,
) {
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
    let (route, query) = parse_monitor_http_query(path);
    if method != "GET" {
        let _ = write_http_json(&mut stream, 405, "{\"error\":\"method_not_allowed\"}");
        return;
    }
    if route == "/" || route == "/dashboard" {
        let _ = write_http_json(
            &mut stream,
            410,
            &static_dashboard_gone_json("Hyperliquid basis"),
        );
        return;
    }

    let snapshot = state.read().expect("monitor state lock poisoned");
    let (status, body) = if route == "/health" {
        (
            200,
            format!(
                "{{\"status\":{},\"updated_at\":{}}}",
                json_string(&snapshot.status),
                json_string(&snapshot.updated_at)
            ),
        )
    } else if route == "/api/hyperliquid-basis/status" {
        (
            200,
            snapshot.to_json_limited(query.row_limit, query.summary_only),
        )
    } else if route == "/api/hyperliquid-basis/opportunities" {
        (
            200,
            snapshot.opportunities_json_limited(query.row_limit, query.summary_only),
        )
    } else if let Some(symbol) = route.strip_prefix("/api/hyperliquid-basis/status/") {
        match snapshot.symbol_json(symbol.trim_matches('/')) {
            Some(row) => (200, row),
            None => (
                404,
                format!(
                    "{{\"error\":\"symbol_not_found\",\"symbol\":{}}}",
                    json_string(symbol.trim_matches('/'))
                ),
            ),
        }
    } else {
        (
            404,
            "{\"error\":\"not_found\",\"paths\":[\"/health\",\"/api/hyperliquid-basis/status\",\"/api/hyperliquid-basis/opportunities\",\"/api/hyperliquid-basis/status/<SYMBOL>\"]}".to_owned(),
        )
    };
    let _ = write_http_json(&mut stream, status, &body);
}

fn handle_aster_basis_http_connection(
    mut stream: TcpStream,
    state: &Arc<RwLock<AsterBasisMonitorSnapshot>>,
) {
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
    let (route, query) = parse_monitor_http_query(path);
    if method != "GET" {
        let _ = write_http_json(&mut stream, 405, "{\"error\":\"method_not_allowed\"}");
        return;
    }
    if route == "/" || route == "/dashboard" {
        let _ = write_http_json(&mut stream, 410, &static_dashboard_gone_json("Aster basis"));
        return;
    }

    let snapshot = state.read().expect("monitor state lock poisoned");
    let (status, body) = if route == "/health" {
        (
            200,
            format!(
                "{{\"status\":{},\"updated_at\":{}}}",
                json_string(&snapshot.status),
                json_string(&snapshot.updated_at)
            ),
        )
    } else if route == "/api/aster-basis/status" {
        (
            200,
            snapshot.to_json_limited(query.row_limit, query.summary_only),
        )
    } else if route == "/api/aster-basis/opportunities" {
        (
            200,
            snapshot.opportunities_json_limited(query.row_limit, query.summary_only),
        )
    } else if let Some(symbol) = route.strip_prefix("/api/aster-basis/status/") {
        match snapshot.symbol_json(symbol.trim_matches('/')) {
            Some(row) => (200, row),
            None => (
                404,
                format!(
                    "{{\"error\":\"symbol_not_found\",\"symbol\":{}}}",
                    json_string(symbol.trim_matches('/'))
                ),
            ),
        }
    } else {
        (
            404,
            "{\"error\":\"not_found\",\"paths\":[\"/health\",\"/api/aster-basis/status\",\"/api/aster-basis/opportunities\",\"/api/aster-basis/status/<SYMBOL>\"]}".to_owned(),
        )
    };
    let _ = write_http_json(&mut stream, status, &body);
}

fn handle_funding_arb_http_connection(
    mut stream: TcpStream,
    state: &Arc<RwLock<FundingArbMonitorSnapshot>>,
    context: &Arc<FundingArbDashboardContext>,
) {
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
    let (route, query) = parse_monitor_http_query(path);
    if method != "GET" {
        let _ = write_http_json(&mut stream, 405, "{\"error\":\"method_not_allowed\"}");
        return;
    }
    if route == "/" || route == "/dashboard" {
        let _ = write_http_json(&mut stream, 410, &static_dashboard_gone_json("Funding arb"));
        return;
    }

    let snapshot = state.read().expect("monitor state lock poisoned");
    let (status, body) = if route == "/health" {
        (
            200,
            format!(
                "{{\"status\":{},\"updated_at\":{},\"mutable_execution_started\":false}}",
                json_string(&snapshot.status),
                json_string(&snapshot.updated_at)
            ),
        )
    } else if route == "/api/funding-arb/status" {
        (
            200,
            snapshot.to_json_limited(query.row_limit, query.summary_only),
        )
    } else if route == "/api/funding-arb/opportunities" {
        (
            200,
            snapshot.opportunities_json_limited(query.row_limit, query.summary_only),
        )
    } else if route == "/api/funding-arb/execution-status" {
        (200, funding_arb_execution_status_json(context.as_ref()))
    } else if route == "/api/funding-arb/history" {
        (200, funding_arb_history_json(context.as_ref()))
    } else if route == "/api/funding-arb/exchange-pnl/reconcile" {
        funding_arb_exchange_pnl_reconcile_http_json(context.as_ref(), path)
    } else if route == "/api/funding-arb/exchange-history/records" {
        funding_arb_exchange_history_records_http_json(context.as_ref(), path)
    } else {
        (
            404,
            "{\"error\":\"not_found\",\"paths\":[\"/health\",\"/api/funding-arb/status\",\"/api/funding-arb/opportunities\",\"/api/funding-arb/execution-status\",\"/api/funding-arb/history\",\"/api/funding-arb/exchange-pnl/reconcile\",\"/api/funding-arb/exchange-history/records\"]}".to_owned(),
        )
    };
    let _ = write_http_json(&mut stream, status, &body);
}
