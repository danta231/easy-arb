# 通用套利平台 V2：资料入口

本目录已清理旧的 `docs/`（开发文档）和 `review/`（评审材料）内容。当前仅保留仍被代码和测试使用的结构化资料。

## 当前内容

- `schemas/`：`schema`（数据结构约束）文件，用于描述合同数据格式。
- `templates/`：`template`（模板）文件，用于配置和运营报告样例。
- `README.md`：当前资料入口，说明保留内容和安全规则。

开发协作规则见仓库根目录的 `AGENTS.md`（协作规则）。

## 常用命令

```bash
cargo test --workspace
cargo xtask check-schema
cargo xtask check-docs
cargo xtask replay-full-pipeline
cargo run -p arb-runtime -- health fixtures/replay/full_pipeline_simulated
cargo run -p arb-runtime -- replay fixtures/replay/full_pipeline_simulated
```

中文说明：这些默认命令只跑离线测试和 fixture，不访问真实交易 API，不使用真实凭证。

## 不可变规则

- 第一天就采用最终架构；可以只启用一个适配器或一个策略，但不能临时合并核心边界。
- 策略永远不拥有执行权限。
- 账本只能追加，修正必须用冲销或调整分录。
- 外部未知状态必须按风险处理，不能当作成功。
- 默认不开启实盘执行或真实签名。
- 密钥、`API secret`（接口密钥）、私钥、助记词、`session`（会话）、`token`（令牌）和 `webhook secret`（回调密钥）不能进入代码、日志、`fixture`（测试样例）、报告或提示词。
