# AGENTS.md

本仓库已清理 `universal_arb_platform_v2_immutable_core_docs/docs` 和 `universal_arb_platform_v2_immutable_core_docs/review` 下的旧文档，不再以这些目录中的运行手册或评审材料作为开发入口。后续协作以本文件、当前代码、测试和明确的用户指令为准。

## 路径规则

- `CODE_ROOT`（代码根目录）是仓库根目录。
- Rust（系统编程语言）工作区、`crates/`、`fixtures/` 和 `xtask/` 放在 `CODE_ROOT` 下。
- `universal_arb_platform_v2_immutable_core_docs/schemas` 保存 `schema`（数据结构约束）。
- `universal_arb_platform_v2_immutable_core_docs/templates` 保存 `template`（模板）文件。
- `docs/` 和 `review/` 当前不作为可引用文档入口；不要新增对这些旧入口的依赖，除非用户明确要求恢复。

## 硬规则

- 阶段 0 之后优先使用 Rust 工具链和 `cargo xtask ...`（项目本地检查命令）做验证。
- 不要把 Node.js（JavaScript 运行时）引入为正式项目依赖。
- 所有新生成或更新的文档必须使用中文；代码标识符、命令名、`schema`（数据结构约束）名称和必要技术术语如需保留英文，必须同时提供中文解释。
- 不要把密钥、接口密钥、私钥、令牌或凭证写入代码、日志、样例、文档或报告。
- 外部状态未知时必须按失败或风险状态处理，不能当作成功。

## 协作约定

- 当用户说“检查服务器日志”时，默认含义是使用 `ssh easy-arb-logreader`（通过 SSH，安全外壳远程登录命令，使用本机 SSH 配置中的 `easy-arb-logreader` 别名连接日志读取目标）检查服务器上 `/opt/easy-arb` 项目中的相关日志；不要把本地日志或其他服务器日志当作默认目标。
- 修复问题时，如果定位为交易所相关问题，必须反推并检查所有已接入交易所是否存在同类问题，不能只修复单一交易所路径。
- 如果修改内容中包含页面相关信息，必须同步检查 Easy Tool 中的 easy-arb 相关页面是否也需要修改，不能只更新 easy-arb 侧的运行时、接口或文案。

## 工作流

每次开发先检查当前工作区状态和相关代码，再小步修改并运行与改动匹配的验证。遇到已有用户改动时必须保留并兼容，不能擅自回退。

涉及 Rust 工作区的变更，优先使用：

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

如果变更涉及本地项目检查入口，再运行相应的 `cargo xtask ...` 命令，并在交付说明中写明结果。
