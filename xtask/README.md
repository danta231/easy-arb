# xtask

中文说明：本目录提供阶段 0 的 Rust 本地检查入口。所有命令默认离线、只读，不访问真实交易 API，不读取凭证，也不实现真实执行或签名。

当前命令：

- `cargo xtask check-schema`：解析文档 schema 和 schema fixture 的 JSON 语法。
- `cargo xtask check-crate-boundaries`：解析 `cargo metadata`（Cargo 元数据），按 `xtask` 内置禁止依赖表检查 `crate`（Rust 包）边界。
- `cargo xtask check-docs`：检查 `AGENTS.md`（协作规则）、资料入口和 `fixture`（测试样例）说明存在，并确认包含中文说明。
