# 中文操作手册 / Operations Manual

本文面向日常操作者、开发者和验收者，说明如何在本仓库中安全地运行离线检查、只读健康检查、端到端模拟回放、异常路径验收、报告查看和任务交接。

中文说明：本手册不替代 `24_Codex_Development_Runbook.md`、`22_Development_Execution_Plan.md`、`23_Module_Architecture_Map.md` 和 `25_Core_Architecture_Reference.md`。涉及开发任务时，仍以 runbook 和任务包为准。

## 1. 适用范围

当前手册覆盖的操作：

- 本地 Rust workspace 检查。
- schema、fixture 和文档检查。
- crate 依赖边界检查。
- `arb-runtime` 只读健康检查。
- `fixtures/replay/full_pipeline_simulated` 端到端模拟回放。
- 异常路径 fixture 回放。
- 黄金输出文件的审慎更新。
- 运营日报、事故、风控拒绝和对账结果的查看。
- Codex 任务包开发前后的操作流程。

当前手册不覆盖的操作：

- 真实下单。
- 真实撤单。
- 真实转账。
- 真实签名。
- 真实资金调拨。
- 自动实盘运行。
- 密钥、API secret、私钥、助记词、session 或 token 配置。

如需讨论个人小额受控试运行，只能从 `review/personal_guarded_live_governance.md` 和相关 review 清单进入；该路径也不等于自动实盘。

## 2. 核心安全规则

日常操作先记住以下停止线：

1. 默认只运行只读、模拟和离线回放。
2. 策略不能下单、签名、转账、写账本或绕过风控。
3. `arb-venue-data` 只能读取外部数据，不能包含下单、撤单、转账或签名能力。
4. `arb-venue-exec` 和 `arb-signing` 即使存在，也必须默认关闭真实执行和真实签名。
5. 未知外部状态必须 fail closed，不能按成功继续。
6. 账本只能追加，修正必须用冲销或调整分录。
7. 任何密钥、API secret、私钥、助记词、session、token 或 webhook secret 都不能写入代码、日志、fixture、报告、文档或提示词。
8. `--accept` 只能在明确要更新黄金输出时使用，不能用于掩盖失败。

安全信号示例：

```text
execution_mode=Simulated
mutable_execution_started=false
real_signing_enabled=false
```

如果看到运行模式请求可变账户权限、真实签名开启、熔断被绕过、未知状态被当作成功，立即停止当前操作。

## 3. 路径约定

在本仓库中使用以下路径约定：

| 名称 | 路径 | 用途 |
|---|---|---|
| `REPO_ROOT` | 仓库根目录 | 当前 Rust workspace 根目录。 |
| `DOC_ROOT` | `universal_arb_platform_v2_immutable_core_docs` | 文档、schema、模板和 review 材料。 |
| `CODE_ROOT` | 仓库根目录 | `crates/`、`fixtures/`、`xtask/` 所在位置。 |
| `docs/...` | `DOC_ROOT/docs/...` | 文档短路径解析位置。 |
| `schemas/...` | `DOC_ROOT/schemas/...` | 权威 JSON schema。 |
| `templates/...` | `DOC_ROOT/templates/...` 或根 `templates/` | 文档模板和运行时代码侧模板。 |
| `fixtures/replay/...` | 仓库根目录下的 replay fixture | 离线回放输入和期望输出。 |

所有命令默认从仓库根目录执行：

```bash
cd /Users/danta/WebstormProjects/easy-arb
```

## 4. 操作前检查

每次操作前先确认工作区和工具链状态：

```bash
git status --short
cargo --version
```

判断方式：

- `git status --short` 为空：当前工作区干净。
- 有输出：先确认这些改动是否属于当前任务。不要回退不属于自己的改动。
- `cargo --version` 能输出版本：Rust 工具链可用。

如果只是查看报告或阅读文档，不需要跑完整质量门；如果要交付开发改动，应运行第 5 节的质量门。

## 5. 本地质量门

正式开发或验收前，建议按以下顺序运行：

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo xtask quality-gate
cargo xtask replay-full-pipeline
```

当前 `cargo xtask quality-gate` 覆盖：

- `cargo xtask check-schema`
- `cargo xtask check-crate-boundaries`
- `cargo xtask check-docs`

中文说明：当前 `quality-gate` 是 xtask 聚合检查，不替代 `cargo fmt`、`cargo clippy` 和 `cargo test`。交付代码改动时仍应显式运行 Rust 格式化、静态检查和测试。

单项命令说明：

| 命令 | 作用 | 正常结果 |
|---|---|---|
| `cargo xtask check-schema` | 解析 schema JSON 和 schema fixture JSON | 输出解析通过的文件数量。 |
| `cargo xtask check-crate-boundaries` | 使用 `cargo metadata` 检查禁止依赖 | 输出检查规则数量和 workspace package 数量。 |
| `cargo xtask check-docs` | 检查必读文档和 fixture 说明 | 确认文档存在且包含中文说明。 |
| `cargo xtask quality-gate` | 聚合当前 xtask 检查 | 三项 xtask 检查均通过。 |
| `cargo xtask replay-full-pipeline` | 运行阶段 9 端到端模拟 fixture | 黄金输出匹配。 |

任何质量门失败时，不要删除测试、放宽 schema 或移除边界检查。先判断是实现错误、fixture 错误、环境缺失还是需求冲突。

## 6. 只读健康检查

运行默认端到端 fixture 的健康检查：

```bash
cargo run -p arb-runtime -- health fixtures/replay/full_pipeline_simulated
```

当前正常输出形态：

```text
health: healthy; execution_mode=Simulated; kill_switch_triggered=false; mutable_execution_started=false; tasks=6
```

字段解释：

| 字段 | 正常含义 | 需要处理的情况 |
|---|---|---|
| `health` | `healthy` 表示启动检查通过 | `degraded` 需要看是否是预期熔断；`unhealthy` 必须停止。 |
| `execution_mode` | 当前 fixture 使用 `Simulated` | 若出现 live 模式，确认是否被熔断阻止；阶段 9 默认不允许启动可变执行。 |
| `kill_switch_triggered` | 默认主路径为 `false` | `true` 不一定错误，但表示被熔断降级，需要看任务预期。 |
| `mutable_execution_started` | 必须为 `false` | 若为 `true`，停止并检查配置和代码。 |
| `tasks` | 运行时装配任务数量 | 数量变化时确认是否是有意的运行时装配改动。 |

健康检查只读取 fixture 和已校验配置，不连接真实交易 API，不签名，不执行账户变更。

## 7. 端到端模拟回放

运行主路径回放：

```bash
cargo run -p arb-runtime -- replay fixtures/replay/full_pipeline_simulated
```

当前正常输出形态：

```text
ok: matched 10 S9-01 artifacts for fixtures/replay/full_pipeline_simulated
```

该命令会装配以下离线链路：

```text
fixture events
  -> read-only venue data fixture
  -> event store
  -> portfolio state
  -> sample strategy
  -> risk evaluator
  -> execution plan
  -> simulated execution report
  -> simulated ledger entries
  -> reconciliation report
  -> operations daily report
```

主路径期望输出位于：

```text
fixtures/replay/full_pipeline_simulated/expected/
```

重要文件：

| 文件 | 含义 |
|---|---|
| `replay_smoke.txt` | 回放烟测稳定文本。 |
| `stored_events.jsonl` | 事件存储中的标准化事件。 |
| `candidate_transitions.jsonl` | 策略输出的候选组合转换。 |
| `risk_decisions.jsonl` | 风控决策。 |
| `execution_plans.jsonl` | 执行计划。 |
| `execution_reports.jsonl` | 模拟执行报告。 |
| `ledger_entries.jsonl` | 模拟账本分录。 |
| `reconciliation_reports.jsonl` | 对账报告。 |
| `incidents.jsonl` | 事故记录；主成功路径通常为空。 |
| `operations_daily_report.md` | 由结构化事实生成的运营日报。 |

## 8. 异常路径回放

异常路径用于证明失败不会被静默当作成功。运行命令：

```bash
cargo run -p arb-runtime -- replay fixtures/replay/full_pipeline_simulated/cases/unknown_state
cargo run -p arb-runtime -- replay fixtures/replay/full_pipeline_simulated/cases/stale_data
cargo run -p arb-runtime -- replay fixtures/replay/full_pipeline_simulated/cases/insufficient_balance
```

当前三个异常路径都应输出 `ok: matched 10 S9-01 artifacts ...`。

异常路径的预期行为：

| fixture | 风控拒绝原因 | 必须成立 |
|---|---|---|
| `unknown_state` | `UNKNOWN_STATE` | 生成可追溯事故，不生成执行计划。 |
| `stale_data` | `DATA_STALE` | 生成可追溯事故，不生成执行计划。 |
| `insufficient_balance` | `INSUFFICIENT_BALANCE` | 生成可追溯事故，不生成执行计划。 |

验收时重点查看：

- `expected/risk_decisions.jsonl` 是否包含预期拒绝原因。
- `expected/execution_plans.jsonl` 是否为空。
- `expected/execution_reports.jsonl` 是否为空。
- `expected/incidents.jsonl` 是否包含 `source_event_refs`。
- `expected/operations_daily_report.md` 是否描述了拒绝和事故事实。

如果异常路径生成了执行计划，必须视为阻塞问题。

## 9. 真实公开行情 + 模拟执行

如果要先接入真实市场数据，但仍然不使用真实资金，可以运行一次性只读行情模拟：

```bash
cargo run -p arb-runtime -- live-market-sim fixtures/replay/full_pipeline_simulated --symbol BTCUSDT --out target/live-market-sim
```

当前命令语义：

- 从 Binance 官方公开市场数据端点读取一次 `BTCUSDT` 24hr ticker。
- 不使用 API key、secret、私钥、session 或 token。
- 只把公开行情标准化为 `NormalizedEvent`。
- 复用主 fixture 的组合状态、策略清单、风控策略和场所能力。
- 执行模式保持 `Simulated`，只生成模拟执行报告、模拟账本分录、对账报告和运营日报。
- 输出写入 `--out` 指定目录；不要写入 `fixtures/.../expected`，除非是明确维护黄金输出。

正常安全信号示例：

```text
ok: fetched live public market data for BTCUSDT; execution_mode=Simulated; mutable_execution_started=false
```

中文说明：该命令不是确定性 replay，因为输入来自当前外部市场数据。它只能作为只读集成烟测，不能替代第 7 节的黄金回放，也不能成为默认质量门。若外部网络失败、限频、字段缺失、数据过期或未知状态出现，命令必须失败或输出风控拒绝/事故，不能按成功继续。

当前限制：

- 只支持与主端到端 fixture 对齐的 `BTCUSDT`。
- 样例策略仍是演示策略，不代表真实套利收益模型。
- 真实下单、撤单、转账和真实签名仍然禁止。

查看本次模拟结果：

```bash
sed -n '1,220p' target/live-market-sim/operations_daily_report.md
sed -n '1,120p' target/live-market-sim/risk_decisions.jsonl
sed -n '1,120p' target/live-market-sim/execution_reports.jsonl
```

## 10. 黄金输出更新

只有在有意改变业务输出、schema、规范序列化、策略、风控、执行、账本、对账或报告格式时，才允许更新黄金输出。

更新主路径黄金输出：

```bash
cargo run -p arb-runtime -- replay fixtures/replay/full_pipeline_simulated --accept
```

更新异常路径黄金输出：

```bash
cargo run -p arb-runtime -- replay fixtures/replay/full_pipeline_simulated/cases/unknown_state --accept
cargo run -p arb-runtime -- replay fixtures/replay/full_pipeline_simulated/cases/stale_data --accept
cargo run -p arb-runtime -- replay fixtures/replay/full_pipeline_simulated/cases/insufficient_balance --accept
```

更新后必须执行：

```bash
git diff -- fixtures/replay/full_pipeline_simulated
cargo run -p arb-runtime -- replay fixtures/replay/full_pipeline_simulated
cargo run -p arb-runtime -- replay fixtures/replay/full_pipeline_simulated/cases/unknown_state
cargo run -p arb-runtime -- replay fixtures/replay/full_pipeline_simulated/cases/stale_data
cargo run -p arb-runtime -- replay fixtures/replay/full_pipeline_simulated/cases/insufficient_balance
```

检查原则：

- 只接受能用需求解释的差异。
- 不接受为了让测试通过而清空事故、清空拒绝原因或删除边界字段。
- 如果 `execution_plans.jsonl` 在异常路径中从空变成非空，必须停止。
- 如果 `mutable_execution_started` 变成 `true`，必须停止。
- 如果输出包含密钥、token、签名材料或未脱敏账户信息，必须停止并删除该输出。

## 11. 配置操作

运行时代码侧配置模板位于：

```text
templates/config.template.yaml
```

默认安全配置应保持：

```yaml
execution:
  mode: "ReadOnly"
  live_execution_enabled: false
  auto_live_enabled: false

signing:
  policy_ref: "signing-policy/null-signer-v1"
  real_signing_enabled: false
```

主模拟 fixture 使用：

```text
fixtures/replay/full_pipeline_simulated/config.yaml
```

当前主模拟 fixture 的关键安全语义：

- `execution.mode` 为 `Simulated`。
- `live_execution_enabled` 为 `false`。
- `auto_live_enabled` 为 `false`。
- `real_signing_enabled` 为 `false`。
- kill switch 未打开，但执行模式本身不允许真实账户变化。

配置变更检查清单：

1. 是否仍然默认不启用真实执行。
2. 是否仍然默认不启用真实签名。
3. 是否没有密钥或凭证明文。
4. 是否保留 kill switch 范围。
5. 是否能通过 `cargo run -p arb-runtime -- health <fixture>`。
6. 是否能通过对应 replay。

## 12. fixture 操作

新增或维护 replay fixture 时，遵守以下目录形态：

```text
fixtures/replay/<case_name>/
  README.md
  config.yaml
  replay.yaml
  events.jsonl
  portfolio_state.json
  risk_policy.yaml
  strategy_manifest.yaml
  venue_capabilities.jsonl
  raw/
  expected/
```

不是每个早期阶段 fixture 都必须包含所有文件，但端到端运行时 fixture 应保持上下文完整。

fixture 内容要求：

- 固定输入，固定时间源，固定策略版本，固定风控版本。
- 不访问外部 API。
- 不依赖本机私有状态。
- 不包含真实密钥、API key、私钥、token 或真实签名材料。
- 原始响应必须脱敏，并放在 `raw/` 下。
- 标准化输出必须能进入事件存储和回放。
- 异常路径必须能解释为什么停止。

新增 fixture 后，至少运行：

```bash
cargo xtask check-schema
cargo run -p arb-runtime -- replay fixtures/replay/<case_name>
```

如果该 fixture 不是端到端运行时 fixture，而是某个模块单测 fixture，应运行对应模块测试，并在 `README.md` 中说明用途和限制。

## 13. 报告和事故查看

主路径和异常路径的运营报告位于：

```text
fixtures/replay/full_pipeline_simulated/expected/operations_daily_report.md
fixtures/replay/full_pipeline_simulated/cases/<case_name>/expected/operations_daily_report.md
```

事故记录位于：

```text
fixtures/replay/full_pipeline_simulated/expected/incidents.jsonl
fixtures/replay/full_pipeline_simulated/cases/<case_name>/expected/incidents.jsonl
```

查看报告：

```bash
sed -n '1,220p' fixtures/replay/full_pipeline_simulated/expected/operations_daily_report.md
```

查看事故：

```bash
sed -n '1,120p' fixtures/replay/full_pipeline_simulated/cases/unknown_state/expected/incidents.jsonl
```

事故验收重点：

- 是否有 `incident_id`。
- 是否有 `severity`。
- 是否有 `source_event_refs`。
- 是否有自动动作和人工动作。
- 是否没有泄露密钥、token 或签名材料。
- 是否没有把未知状态当作成功。

## 14. Codex 开发操作

让 Codex 执行开发任务时，优先使用任务包入口：

```text
请按 universal_arb_platform_v2_immutable_core_docs/docs/24_Codex_Development_Runbook.md 执行任务包 <ID>。
只修改该任务包允许范围内的文件。
完成后给出验证结果、交付文件和剩余风险。
```

开发任务开始前应读取：

1. `docs/24_Codex_Development_Runbook.md`
2. `docs/22_Development_Execution_Plan.md`
3. `docs/23_Module_Architecture_Map.md`
4. `docs/25_Core_Architecture_Reference.md`

若任务涉及 schema 或状态机，还应读取：

- `docs/14_Data_Schemas_and_Contracts.md`
- `docs/21_State_Machines_and_Replay_Fixtures.md`

开发任务完成后，交付说明至少包含：

- 已完成内容。
- 运行过的验证命令和结果。
- 修改的文件。
- 剩余风险。
- 下一步建议。

如果任务修改了模块依赖、schema 字段、状态机、风控检查、执行流程、账本规则或实盘能力开关，必须同步检查相关文档是否需要更新。

## 15. 常见故障处理

| 现象 | 常见原因 | 处理方式 |
|---|---|---|
| `check-schema` 失败 | JSON 语法错误或 schema 目录缺失 | 定位报错文件，修复 JSON，不要删除反例 fixture。 |
| `check-crate-boundaries` 失败 | crate 引入了禁止依赖 | 移除越界依赖，通过 trait 或上层装配重新设计。 |
| `check-docs` 失败 | 必读文档缺失或缺中文说明 | 补齐文档，不要放宽检查。 |
| `replay` 黄金不匹配 | 输出逻辑、规范序列化或 fixture 变化 | 先审查 diff；只有有意变化才用 `--accept`。 |
| `health` 为 `degraded` | 熔断打开或有 warning | 判断是否是预期熔断；确认 `mutable_execution_started=false`。 |
| `health` 为 `unhealthy` | 启动检查失败或任务失败 | 停止，查看错误，不能继续模拟为成功。 |
| 异常路径生成执行计划 | 风控或运行时停止逻辑失效 | 阻塞处理，修复后重跑异常路径。 |
| 输出中出现凭证 | fixture 或日志污染 | 立即移除并轮换相关凭证；不要提交该输出。 |
| `cargo clippy` 失败 | 代码警告被当作错误 | 修复代码或测试，不要降低 `-D warnings`。 |

## 16. 实盘相关停止线

本仓库当前默认操作手册不提供真实资金上线步骤。以下动作必须停止并走 review 路径：

- 新增真实交易 API 下单实现。
- 新增真实撤单、转账、提现或链上交易提交。
- 新增真实签名器或真实密钥读取。
- 启用 live execution feature。
- 把自动实盘设置为默认。
- 把外部未知状态处理为成功。
- 移除 kill switch、对账或事故记录。

个人小额受控试运行也必须满足：

- 只使用本人小额资金。
- 不管理他人、团队、客户或商业资金。
- 无提现权限。
- 隔离账户或隔离钱包。
- 每笔人工确认。
- kill switch 覆盖全局、执行、场所、策略、账户、工具、资产、链和执行模式。
- 每个真实动作后强制对账。
- mismatch、unknown state、permission failure、signer failure 都生成事故记录。

任一证据缺失时，保持只读、模拟或人工手动执行。

## 17. 日常操作清单

只读检查：

```bash
git status --short
cargo xtask quality-gate
cargo run -p arb-runtime -- health fixtures/replay/full_pipeline_simulated
```

模拟回放：

```bash
cargo run -p arb-runtime -- replay fixtures/replay/full_pipeline_simulated
```

真实公开行情 + 模拟执行：

```bash
cargo run -p arb-runtime -- live-market-sim fixtures/replay/full_pipeline_simulated --symbol BTCUSDT --out target/live-market-sim
```

异常路径验收：

```bash
cargo run -p arb-runtime -- replay fixtures/replay/full_pipeline_simulated/cases/unknown_state
cargo run -p arb-runtime -- replay fixtures/replay/full_pipeline_simulated/cases/stale_data
cargo run -p arb-runtime -- replay fixtures/replay/full_pipeline_simulated/cases/insufficient_balance
```

代码交付前：

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo xtask quality-gate
cargo xtask replay-full-pipeline
```

手工验收关注点：

- 是否仍然离线可运行。
- 是否仍然不访问真实 API。
- 是否仍然不读取凭证。
- 是否仍然不启动可变执行。
- 异常路径是否停止在风控拒绝和事故记录。
- 报告是否由结构化事实生成。
- 文档是否同步更新。

## 18. 交接模板

长任务或上下文切换前，使用以下模板：

```text
当前阶段：
已完成任务：
已修改文件：
已运行验证：
未完成事项：
已知风险：
下一步建议：
禁止回退的用户改动：
```

交接时不要粘贴密钥、token、真实账户信息或未脱敏日志。
