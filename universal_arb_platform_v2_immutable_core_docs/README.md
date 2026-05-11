# 通用套利平台 V2：中文文档入口

本目录包含通用多策略套利平台的不可变核心架构、开发计划、模块边界、schema 合同、
回放规则和实盘治理材料。

中文用户请先读：

1. `docs/00_Start_Here_CN.md`：中文用户入口。告诉你不同场景该读哪些文档、怎么跑离线命令、哪些实盘资料不要提前读。
2. `docs/24_Codex_Development_Runbook.md`：Codex 开发运行手册。真正让 Codex 开发任务时，从这里进入。

中文说明：不要从头读完所有文档。日常开发先读中文入口，再按任务跳到阶段计划、
模块地图、schema 或 review 材料。

## 最短路径

| 场景 | 入口 |
|---|---|
| 我只想知道怎么读这些文档 | `docs/00_Start_Here_CN.md` |
| 我要让 Codex 开发任务 | `docs/24_Codex_Development_Runbook.md` |
| 我要看阶段计划 | `docs/22_Development_Execution_Plan.md` |
| 我要看模块边界 | `docs/23_Module_Architecture_Map.md` |
| 我要理解核心架构原则 | `docs/25_Core_Architecture_Reference.md` |
| 我要改 schema 或合同类型 | `docs/14_Data_Schemas_and_Contracts.md` |
| 我要看状态机和回放 fixture | `docs/21_State_Machines_and_Replay_Fixtures.md` |
| 我要准备个人小额受控试运行 | `review/personal_guarded_live_evidence_collection_guide.md` |

## 核心文档

- `docs/00_Start_Here_CN.md`：中文用户导航。
- `docs/24_Codex_Development_Runbook.md`：Codex 开发入口，包含任务模板、阶段任务包、验证命令和交接格式。
- `docs/22_Development_Execution_Plan.md`：阶段计划、交付点、测试方案和验收方案。
- `docs/23_Module_Architecture_Map.md`：模块职责、输入输出、接口边界、测试责任和阶段对应关系。
- `docs/25_Core_Architecture_Reference.md`：核心架构参考，说明不可变原则、安全治理和验收清单。
- `docs/14_Data_Schemas_and_Contracts.md`：权威数据合同说明。
- `docs/21_State_Machines_and_Replay_Fixtures.md`：状态机与回放样例。

## 常用命令

```bash
cargo test --workspace
cargo xtask check-docs
cargo xtask replay-full-pipeline
cargo run -p arb-runtime -- health fixtures/replay/full_pipeline_simulated
cargo run -p arb-runtime -- replay fixtures/replay/full_pipeline_simulated
```

中文说明：这些默认命令只跑离线测试和 fixture，不访问真实交易 API，不使用真实凭证。

## 实盘和个人试运行

默认规则：先只读和模拟，后实盘。真实执行、真实签名、真实资金动作必须走 review
材料和安全门。

个人小额受控试运行相关文档：

- `review/personal_guarded_live_governance.md`：个人自用小额受控试运行治理画像。
- `review/personal_guarded_live_evidence_collection_guide.md`：证据如何收集、脱敏和填写。
- `review/personal_guarded_live_evidence_index.md`：证据索引，只记录脱敏证据引用。
- `review/personal_guarded_live_pilot_checklist.md`：准入清单。
- `review/personal_guarded_live_checklist_audit_report.md`：清单审计报告。

外部受控实盘审查相关文档：

- `review/controlled_live_readiness_review.md`：受控实盘准备评审材料。
- `review/controlled_live_readiness_checklist.md`：外部审查清单。

中文说明：个人路径不是外部审查通过，不允许自动实盘，也不能用于他人资金、团队资金、
客户资金或商业服务。

## 不可变规则

- 第一天就采用最终架构；可以只启用一个适配器或一个策略，但不能临时合并核心边界。
- 策略永远不拥有执行权限。
- 账本只能追加，修正必须用冲销或调整分录。
- 外部未知状态必须按风险处理，不能当作成功。
- 默认不开启实盘执行或真实签名。
- 密钥、API secret、私钥、助记词、session、token 和 webhook secret 不能进入代码、日志、fixture、报告或提示词。
