use crate::*;

pub(crate) fn basis_dashboard_html() -> &'static str {
    include_str!(
        "../../../../universal_arb_platform_v2_immutable_core_docs/templates/basis_dashboard.html"
    )
}

pub(crate) fn bybit_basis_dashboard_html() -> String {
    basis_dashboard_html()
        .replace("Binance public basis", "Bybit public basis")
        .replace("/api/basis/status", "/api/bybit-basis/status")
        .replace("/api/basis/opportunities", "/api/bybit-basis/opportunities")
}

pub(crate) fn okx_basis_dashboard_html() -> String {
    basis_dashboard_html()
        .replace("Binance public basis", "OKX public basis")
        .replace("/api/basis/status", "/api/okx-basis/status")
        .replace("/api/basis/opportunities", "/api/okx-basis/opportunities")
}

pub(crate) fn bitget_basis_dashboard_html() -> String {
    basis_dashboard_html()
        .replace("Binance public basis", "Bitget public basis")
        .replace("/api/basis/status", "/api/bitget-basis/status")
        .replace(
            "/api/basis/opportunities",
            "/api/bitget-basis/opportunities",
        )
}

pub(crate) fn hyperliquid_basis_dashboard_html() -> String {
    basis_dashboard_html()
        .replace("Binance public basis", "Hyperliquid public basis")
        .replace("Spot Bid", "Spot Mid")
        .replace("Spot Ask", "Spot Mid")
        .replace("Perp Bid", "Perp Mid")
        .replace("Perp Ask", "Perp Mid")
        .replace("/api/basis/status", "/api/hyperliquid-basis/status")
        .replace(
            "/api/basis/opportunities",
            "/api/hyperliquid-basis/opportunities",
        )
}

pub(crate) fn aster_basis_dashboard_html() -> String {
    basis_dashboard_html()
        .replace("Binance public basis", "Aster public basis")
        .replace("/api/basis/status", "/api/aster-basis/status")
        .replace("/api/basis/opportunities", "/api/aster-basis/opportunities")
}

pub(crate) fn funding_arb_dashboard_html() -> &'static str {
    include_str!(
        "../../../../universal_arb_platform_v2_immutable_core_docs/templates/funding_arb_dashboard.html"
    )
}

pub(crate) fn portfolio_dashboard_html() -> &'static str {
    include_str!(
        "../../../../universal_arb_platform_v2_immutable_core_docs/templates/portfolio_dashboard.html"
    )
}

pub(crate) fn error_logs_dashboard_html() -> &'static str {
    include_str!(
        "../../../../universal_arb_platform_v2_immutable_core_docs/templates/error_logs_dashboard.html"
    )
}

pub(crate) fn navigation_dashboard_html() -> &'static str {
    include_str!(
        "../../../../universal_arb_platform_v2_immutable_core_docs/templates/navigation_dashboard.html"
    )
}

pub(crate) struct SystemNavigationEntry {
    pub(crate) category: &'static str,
    pub(crate) title: &'static str,
    pub(crate) description: &'static str,
    pub(crate) url: &'static str,
    pub(crate) health_url: &'static str,
}

pub(crate) fn system_navigation_entries() -> &'static [SystemNavigationEntry] {
    // 中文说明：新增 dashboard 或可访问页面时必须登记到这里，导航页和
    // /api/navigation/pages 会自动展示该清单。
    &[
        SystemNavigationEntry {
            category: "总览",
            title: "系统导航",
            description: "统一入口，管理系统内所有 dashboard 和状态页面。",
            url: "http://127.0.0.1:8805/nav",
            health_url: "http://127.0.0.1:8805/health",
        },
        SystemNavigationEntry {
            category: "总览",
            title: "组合余额与仓位",
            description: "账户余额、开仓仓位、来源错误和资金费率上下文。",
            url: "http://127.0.0.1:8805/dashboard",
            health_url: "http://127.0.0.1:8805/health",
        },
        SystemNavigationEntry {
            category: "总览",
            title: "错误日志",
            description: "聚合本次运行的健康事件、resident 事件、live report 和本地日志错误。",
            url: "http://127.0.0.1:8805/errors",
            health_url: "http://127.0.0.1:8805/health",
        },
        SystemNavigationEntry {
            category: "basis",
            title: "Binance basis",
            description: "Binance 公开 spot/perp basis 全市场监控。",
            url: "http://127.0.0.1:8796/dashboard",
            health_url: "http://127.0.0.1:8796/health",
        },
        SystemNavigationEntry {
            category: "basis",
            title: "Bybit basis",
            description: "Bybit 公开 spot/linear-perp basis 全市场监控。",
            url: "http://127.0.0.1:8797/dashboard",
            health_url: "http://127.0.0.1:8797/health",
        },
        SystemNavigationEntry {
            category: "basis",
            title: "OKX basis",
            description: "OKX 公开 spot/swap basis 全市场监控。",
            url: "http://127.0.0.1:8798/dashboard",
            health_url: "http://127.0.0.1:8798/health",
        },
        SystemNavigationEntry {
            category: "basis",
            title: "Bitget basis",
            description: "Bitget 公开 spot/USDT-FUTURES basis 全市场监控。",
            url: "http://127.0.0.1:8803/dashboard",
            health_url: "http://127.0.0.1:8803/health",
        },
        SystemNavigationEntry {
            category: "basis",
            title: "Aster basis",
            description: "Aster 公开 spot/perp basis 全市场监控。",
            url: "http://127.0.0.1:8800/dashboard",
            health_url: "http://127.0.0.1:8800/health",
        },
        SystemNavigationEntry {
            category: "basis",
            title: "Hyperliquid basis",
            description: "Hyperliquid 公开 spot/perp basis 全市场监控。",
            url: "http://127.0.0.1:8799/dashboard",
            health_url: "http://127.0.0.1:8799/health",
        },
        SystemNavigationEntry {
            category: "资金费率",
            title: "Funding arb",
            description: "跨交易所 funding arb 机会聚合和候选记录。",
            url: "http://127.0.0.1:8804/dashboard",
            health_url: "http://127.0.0.1:8804/health",
        },
        SystemNavigationEntry {
            category: "WSS",
            title: "Binance spot WSS",
            description: "Binance spot bookTicker 实时前置行情。",
            url: "http://127.0.0.1:8786/dashboard",
            health_url: "http://127.0.0.1:8786/health",
        },
        SystemNavigationEntry {
            category: "WSS",
            title: "Binance perp WSS",
            description: "Binance USD-M perp bookTicker 实时前置行情。",
            url: "http://127.0.0.1:8787/dashboard",
            health_url: "http://127.0.0.1:8787/health",
        },
        SystemNavigationEntry {
            category: "WSS",
            title: "Bybit spot WSS",
            description: "Bybit spot orderbook.1 实时前置行情。",
            url: "http://127.0.0.1:8788/dashboard",
            health_url: "http://127.0.0.1:8788/health",
        },
        SystemNavigationEntry {
            category: "WSS",
            title: "Bybit perp WSS",
            description: "Bybit linear-perp orderbook.1 实时前置行情。",
            url: "http://127.0.0.1:8789/dashboard",
            health_url: "http://127.0.0.1:8789/health",
        },
        SystemNavigationEntry {
            category: "WSS",
            title: "OKX spot WSS",
            description: "OKX spot tickers 实时前置行情。",
            url: "http://127.0.0.1:8790/dashboard",
            health_url: "http://127.0.0.1:8790/health",
        },
        SystemNavigationEntry {
            category: "WSS",
            title: "OKX swap WSS",
            description: "OKX swap tickers 实时前置行情。",
            url: "http://127.0.0.1:8791/dashboard",
            health_url: "http://127.0.0.1:8791/health",
        },
        SystemNavigationEntry {
            category: "WSS",
            title: "Bitget spot WSS",
            description: "Bitget spot ticker 实时前置行情。",
            url: "http://127.0.0.1:8792/dashboard",
            health_url: "http://127.0.0.1:8792/health",
        },
        SystemNavigationEntry {
            category: "WSS",
            title: "Bitget futures WSS",
            description: "Bitget USDT-FUTURES ticker 实时前置行情。",
            url: "http://127.0.0.1:8793/dashboard",
            health_url: "http://127.0.0.1:8793/health",
        },
    ]
}

pub(crate) fn system_navigation_pages_json() -> String {
    format!(
        "{{\"pages\":[{}],\"schema_version\":\"1.0.0\",\"updated_at\":{}}}",
        system_navigation_entries()
            .iter()
            .map(system_navigation_entry_json)
            .collect::<Vec<_>>()
            .join(","),
        json_string(&current_utc_timestamp_string()),
    )
}

pub(crate) fn system_navigation_entry_json(entry: &SystemNavigationEntry) -> String {
    format!(
        "{{\"category\":{},\"description\":{},\"health_url\":{},\"title\":{},\"url\":{}}}",
        json_string(entry.category),
        json_string(entry.description),
        json_string(entry.health_url),
        json_string(entry.title),
        json_string(entry.url),
    )
}
