use crate::*;

pub(crate) struct SystemNavigationEntry {
    pub(crate) category: &'static str,
    pub(crate) title: &'static str,
    pub(crate) description: &'static str,
    pub(crate) url: &'static str,
    pub(crate) health_url: &'static str,
}

pub(crate) fn system_navigation_entries() -> &'static [SystemNavigationEntry] {
    // 中文说明：HTML dashboard 已迁移到 Easy Tool；这里仅登记只读 JSON API。
    // /api/navigation/pages 会自动展示该清单。
    &[
        SystemNavigationEntry {
            category: "总览",
            title: "系统导航",
            description: "统一入口，列出系统内所有只读 JSON API 和状态接口。",
            url: "http://127.0.0.1:8805/api/navigation/pages",
            health_url: "http://127.0.0.1:8805/health",
        },
        SystemNavigationEntry {
            category: "总览",
            title: "组合余额与仓位",
            description: "账户余额、开仓仓位、来源错误和资金费率上下文。",
            url: "http://127.0.0.1:8805/api/portfolio/status",
            health_url: "http://127.0.0.1:8805/health",
        },
        SystemNavigationEntry {
            category: "总览",
            title: "错误日志",
            description: "聚合本次运行的健康事件、resident 事件、live report 和本地日志错误。",
            url: "http://127.0.0.1:8805/api/errors/logs",
            health_url: "http://127.0.0.1:8805/health",
        },
        SystemNavigationEntry {
            category: "basis",
            title: "Binance basis",
            description: "Binance 公开 spot/perp basis 全市场监控。",
            url: "http://127.0.0.1:8796/api/basis/status",
            health_url: "http://127.0.0.1:8796/health",
        },
        SystemNavigationEntry {
            category: "basis",
            title: "Bybit basis",
            description: "Bybit 公开 spot/linear-perp basis 全市场监控。",
            url: "http://127.0.0.1:8797/api/bybit-basis/status",
            health_url: "http://127.0.0.1:8797/health",
        },
        SystemNavigationEntry {
            category: "basis",
            title: "OKX basis",
            description: "OKX 公开 spot/swap basis 全市场监控。",
            url: "http://127.0.0.1:8798/api/okx-basis/status",
            health_url: "http://127.0.0.1:8798/health",
        },
        SystemNavigationEntry {
            category: "basis",
            title: "Bitget basis",
            description: "Bitget 公开 spot/USDT-FUTURES basis 全市场监控。",
            url: "http://127.0.0.1:8803/api/bitget-basis/status",
            health_url: "http://127.0.0.1:8803/health",
        },
        SystemNavigationEntry {
            category: "basis",
            title: "Aster basis",
            description: "Aster 公开 spot/perp basis 全市场监控。",
            url: "http://127.0.0.1:8800/api/aster-basis/status",
            health_url: "http://127.0.0.1:8800/health",
        },
        SystemNavigationEntry {
            category: "basis",
            title: "Hyperliquid basis",
            description: "Hyperliquid 公开 spot/perp basis 全市场监控。",
            url: "http://127.0.0.1:8799/api/hyperliquid-basis/status",
            health_url: "http://127.0.0.1:8799/health",
        },
        SystemNavigationEntry {
            category: "资金费率",
            title: "Funding arb",
            description: "跨交易所 funding arb 机会聚合和候选记录。",
            url: "http://127.0.0.1:8804/api/funding-arb/status",
            health_url: "http://127.0.0.1:8804/health",
        },
        SystemNavigationEntry {
            category: "WSS",
            title: "Binance spot WSS",
            description: "Binance spot bookTicker 实时前置行情。",
            url: "http://127.0.0.1:8786/api/binance-wss-book-ticker/status",
            health_url: "http://127.0.0.1:8786/health",
        },
        SystemNavigationEntry {
            category: "WSS",
            title: "Binance perp WSS",
            description: "Binance USD-M perp bookTicker 实时前置行情。",
            url: "http://127.0.0.1:8806/api/binance-wss-book-ticker/status",
            health_url: "http://127.0.0.1:8806/health",
        },
        SystemNavigationEntry {
            category: "WSS",
            title: "Bybit spot WSS",
            description: "Bybit spot orderbook.1 实时前置行情。",
            url: "http://127.0.0.1:8788/api/bybit-wss-book-ticker/status",
            health_url: "http://127.0.0.1:8788/health",
        },
        SystemNavigationEntry {
            category: "WSS",
            title: "Bybit perp WSS",
            description: "Bybit linear-perp orderbook.1 实时前置行情。",
            url: "http://127.0.0.1:8789/api/bybit-wss-book-ticker/status",
            health_url: "http://127.0.0.1:8789/health",
        },
        SystemNavigationEntry {
            category: "WSS",
            title: "OKX spot WSS",
            description: "OKX spot tickers 实时前置行情。",
            url: "http://127.0.0.1:8790/api/okx-wss-book-ticker/status",
            health_url: "http://127.0.0.1:8790/health",
        },
        SystemNavigationEntry {
            category: "WSS",
            title: "OKX swap WSS",
            description: "OKX swap tickers 实时前置行情。",
            url: "http://127.0.0.1:8791/api/okx-wss-book-ticker/status",
            health_url: "http://127.0.0.1:8791/health",
        },
        SystemNavigationEntry {
            category: "WSS",
            title: "Bitget spot WSS",
            description: "Bitget spot ticker 实时前置行情。",
            url: "http://127.0.0.1:8792/api/bitget-wss-book-ticker/status",
            health_url: "http://127.0.0.1:8792/health",
        },
        SystemNavigationEntry {
            category: "WSS",
            title: "Bitget futures WSS",
            description: "Bitget USDT-FUTURES ticker 实时前置行情。",
            url: "http://127.0.0.1:8793/api/bitget-wss-book-ticker/status",
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
