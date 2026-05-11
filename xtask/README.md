# xtask

中文说明：本目录提供阶段 0 的 Rust 本地检查入口。所有命令默认离线、只读，不访问真实交易 API，不读取凭证，也不实现真实执行或签名。

当前命令：

- `cargo xtask check-schema`：解析文档 schema 和 schema fixture 的 JSON 语法。
- `cargo xtask check-crate-boundaries`：解析 `cargo metadata`，按 `docs/23_Module_Architecture_Map.md` 的禁止依赖表检查 crate 边界。
- `cargo xtask check-docs`：检查必读文档和 fixture 说明存在，并确认包含中文说明。
