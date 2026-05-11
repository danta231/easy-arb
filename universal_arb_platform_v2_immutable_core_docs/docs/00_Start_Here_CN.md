# 中文用户入口 / Start Here for Chinese Users

本文是中文用户的第一入口。它不替代权威架构文档，而是告诉你先读什么、什么时候
再读细节文档，以及常见任务应该从哪里开始。

中文说明：本项目文档较多，是因为它把架构、阶段计划、模块边界、schema、回放、
实盘治理和 Codex 开发流程拆开管理。日常使用时不要从头读完全部文档，按下面路径
阅读即可。

## 1. 先记住三条规则

1. 默认只能跑只读、模拟和离线回放，不接真实资金。
2. 策略不能下单、签名、转账或写账本。
3. 任何实盘、真实签名、真实资金动作，都必须先通过对应清单和安全门。

中文说明：如果你只想开发或跑模拟，通常不需要接触实盘审查文档；如果你想准备个人
小额受控试运行，必须先读第 6 节。

## 2. 最短阅读路径

| 你的目标 | 先读 | 再读 | 不需要先读 |
|---|---|---|---|
| 让 Codex 开发一个任务 | `docs/24_Codex_Development_Runbook.md` | 当前任务涉及的阶段/模块说明 | review 目录 |
| 理解项目阶段 | `docs/22_Development_Execution_Plan.md` | `docs/23_Module_Architecture_Map.md` | schema 细节 |
| 理解模块边界 | `docs/23_Module_Architecture_Map.md` | `docs/25_Core_Architecture_Reference.md` | review 目录 |
| 跑离线模拟 | 本文第 5 节 | `docs/21_State_Machines_and_Replay_Fixtures.md` | 实盘清单 |
| 改 schema 或合同类型 | `docs/14_Data_Schemas_and_Contracts.md` | `schemas/*.json` | 实盘清单 |
| 准备个人小额受控试运行 | 本文第 6 节 | `review/personal_guarded_live_evidence_collection_guide.md` | 外部审查清单，除非涉及他人/团队/商业资金 |

## 3. 文档分层

### 每次开发必读

- `docs/24_Codex_Development_Runbook.md`：Codex 开发流程、任务包、验证命令。
- `docs/22_Development_Execution_Plan.md`：阶段目标、交付点、测试和验收。
- `docs/23_Module_Architecture_Map.md`：模块职责、依赖边界、禁止行为。
- `docs/25_Core_Architecture_Reference.md`：不可变架构原则和安全治理。

中文说明：这些是开发任务的上层规则。让 Codex 做代码或文档改动时，优先以
`24` 为入口。

### 按需阅读

- `docs/14_Data_Schemas_and_Contracts.md`：改 schema、合同结构、fixture 时读。
- `docs/21_State_Machines_and_Replay_Fixtures.md`：改状态机、回放、事故样例时读。
- `schemas/*.json`：权威数据合同。
- `templates/*.yaml`、`templates/*.md`：配置和运营报告模板。

### 实盘和治理资料

- `review/controlled_live_readiness_review.md`：受控实盘准备评审材料。
- `review/controlled_live_readiness_checklist.md`：外部审查路径。
- `review/personal_guarded_live_governance.md`：个人小额受控试运行治理画像。
- `review/personal_guarded_live_pilot_checklist.md`：个人小额受控试运行准入清单。
- `review/personal_guarded_live_evidence_index.md`：个人路径证据索引。
- `review/personal_guarded_live_evidence_collection_guide.md`：每个证据如何收集、脱敏和填写。

中文说明：`review` 目录不是普通开发入口。只有准备实盘、安全审查、个人小额试运行
或事故演练时才需要读。

## 4. 常用 Codex 提示词

### 执行任务包

```text
请按 universal_arb_platform_v2_immutable_core_docs/docs/24_Codex_Development_Runbook.md 执行任务包 <ID>。
只修改该任务包允许范围内的文件。
完成后给出验证结果、交付文件和剩余风险。
```

### 做验收检查

```text
请按 docs/24_Codex_Development_Runbook.md 的任务包验收要求，验收 <阶段/任务包>。
重点检查：是否越界、是否缺测试、是否违反模块依赖、是否需要同步文档。
```

### 做文档审计

```text
请按 docs/24、22、23、25 对 <文档路径> 做审计。
输出阻塞问题、建议修订和需要同步更新的文档。
```

## 5. 离线运行和验证

常用离线命令：

```bash
cargo test --workspace
cargo xtask check-docs
cargo xtask replay-full-pipeline
cargo run -p arb-runtime -- health fixtures/replay/full_pipeline_simulated
cargo run -p arb-runtime -- replay fixtures/replay/full_pipeline_simulated
```

正常安全信号：

- `execution_mode=Simulated`
- `mutable_execution_started=false`
- replay 输出匹配预期 artifact

中文说明：这些命令只运行离线 fixture，不访问真实交易 API，不使用真实凭证，不启动
可变执行。

## 6. 个人小额受控试运行路径

如果你只使用自己的小额封顶资金，且不是团队、客户、他人资金或商业服务，可以走
个人路径。但它仍不是自动实盘，也不是外部审查通过。

阅读顺序：

1. `review/personal_guarded_live_governance.md`
2. `review/personal_guarded_live_evidence_collection_guide.md`
3. `review/personal_guarded_live_evidence_index.md`
4. `review/personal_guarded_live_pilot_checklist.md`

必须保持：

- 无提现权限。
- 隔离账户或隔离钱包。
- 小额资金上限、单笔上限、日亏损上限、最大开放动作上限。
- 每笔人工确认。
- kill switch 覆盖全局、执行、场所、策略、账户、工具、资产、链和执行模式。
- 未知状态停机。
- 每个 live action 后强制对账。
- mismatch、unknown state、permission failure、signer failure 都要有事故记录。

中文说明：任一证据缺失时，保持 `ReadOnlyPersonal`、`ManualExecutionPersonal` 或模拟模式。

## 7. 不建议做的事

- 不要把真实 API key、secret、私钥、助记词、session、token、webhook secret 写进文档。
- 不要为了“跑起来”改掉 feature gate、kill switch、对账或签名失败检查。
- 不要把个人小额试运行清单当成外部审查。
- 不要让策略模块直接接触执行、签名、账本写入或运行时装配。
- 不要把所有文档一次性读完；按任务读对应入口。

## 8. 如果你不知道该读哪份

按这个顺序问：

1. 我是在开发功能、验收功能、跑模拟，还是准备实盘？
2. 这个任务属于哪个阶段或模块？
3. 是否涉及 schema、状态机、实盘能力、签名、权限、账本或对账？

如果仍不确定，先读 `docs/24_Codex_Development_Runbook.md`，让 Codex 按任务包或模块
边界帮你定位需要读的文档。
