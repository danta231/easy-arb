# Architecture Review Log / 架构评审记录

This review log is an internal structured self-review, not an external audit.

中文说明：本记录是内部结构化自评，不是外部审计。实盘资金上线前仍需要独立安全、风控、账本和执行审查。

## Review round 1 - Scope completeness / 范围完整性

Finding: The architecture must not be limited to CEX/DEX spot arbitrage.

中文发现：架构不能只覆盖 CEX/DEX 现货套利。

Change: Expanded strategy model to portfolio transformations covering spot, perp, futures, options, borrow/lend, RFQ, bridge, funding, and carry strategies.

中文变更：将策略模型扩展为组合状态转换，覆盖现货、永续、期货、期权、借贷、RFQ、跨桥、资金费率和 carry 策略。

## Review round 2 - Venue neutrality / 场所中立

Finding: CEX/DEX taxonomy is insufficient for Hyperliquid, Aster, dYdX, GMX, RFQ systems, lending venues, and aggregators.

中文发现：CEX/DEX 二分不足以描述 Hyperliquid、Aster、dYdX、GMX、RFQ、借贷场所和聚合器。

Change: Replaced hard venue categories with capability-based venue model.

中文变更：用能力模型替代硬编码场所分类。

## Review round 3 - Language neutrality / 语言中立

Finding: Implementation-language suggestions can accidentally shape architecture.

中文发现：实现语言建议可能反向污染架构边界。

Change: Moved Rust/Python/TypeScript/Go into implementation profiles. Canonical contracts are schemas and state machines.

中文变更：将 Rust/Python/TypeScript/Go 放入实现语言画像；权威合同由 schema 和状态机定义。

## Review round 4 - No-frontend operation / 无前端运营

Finding: Without a frontend, logs and records are not support tools; they are the operator interface.

中文发现：没有前端时，日志和记录就是操作员界面，不是辅助材料。

Change: Elevated journals, reports, metrics, alerts, and replay to first-class architecture components.

中文变更：将日志、报告、指标、告警和回放提升为一等架构组件。

## Review round 5 - No incremental replacement / 不做渐进替换架构

Finding: Prior design language could be interpreted as starting with a simpler architecture and replacing it later.

中文发现：旧表述可能被理解为先做简单 bot，再替换为平台。

Change: Clarified that day one uses the final architecture. Future changes add plugins, adapters, policies, modes, and sinks without replacing the core.

中文变更：明确第一天就采用最终架构，未来只新增插件、适配器、策略、模式和事件出口，不替换核心。

## Review round 6 - Safety boundary / 安全边界

Finding: Strategy plugins must never be allowed to execute.

中文发现：策略插件绝不能拥有执行权限。

Change: Added explicit forbidden strategy behavior and required execution pipeline stages.

中文变更：新增策略禁止行为和必经执行管线阶段。

## Review round 7 - Audit honesty / 审计诚实性

Finding: The document must not imply external expert review.

中文发现：文档不能暗示已经过外部专家审查。

Change: Labeled the review log as structured self-review and recommended independent review before live funds.

中文变更：标明这是结构化自评，并建议实盘资金上线前进行独立审查。

## Review round 8 - Rust readiness / Rust 开发准备度

Finding: Before Rust development, core contracts need schema versions and capital reservation state must be represented in schema.

中文发现：Rust 开发前，核心合同必须有 schema 版本，资本预留状态必须落入 schema。

Change: Added `schema_version` to candidate transition, risk decision, and execution plan schemas; added reservation state to portfolio state.

中文变更：为候选转换、风控决策和执行计划补充 `schema_version`；为组合状态中的资本预留补充状态字段。

## Review round 9 - Module architecture completeness / 模块化完整性

Finding: Module information existed, but it was split across multiple documents and was not enough for direct task ownership and module-level acceptance.

中文发现：模块信息已经存在，但分散在多个文档中，不利于开发时直接分工和按模块验收。

Change: Added `23_Module_Architecture_Map.md` with module groups, fact flow, data ownership, module cards, interface boundaries, stage mapping, and module completion definition.

中文变更：新增模块化架构地图，集中说明模块分组、事实流、数据归属、模块卡片、接口边界、阶段对应关系和完成定义。

## Review round 10 - Codex development readiness / Codex 开发准备度

Finding: The project will be developed through Codex, so Codex needs a direct runbook instead of relying on scattered architecture documents.

中文发现：后续将全程使用 Codex 开发，因此需要一份 Codex 可直接执行的运行手册，而不是让 Codex 从分散架构文档中自行推断流程。

Change: Added `24_Codex_Development_Runbook.md` with task templates, stage task packages, verification commands, failure handling rules, documentation sync rules, and handoff format.

中文变更：新增 Codex 开发运行手册，包含任务模板、阶段任务包、验证命令、失败处理规则、文档同步规则和交接格式。

## Review round 11 - Documentation consolidation / 文档裁剪合并

Finding: The document set was too fragmented for Codex-driven development and repeated the same architecture rules in many small files.

中文发现：文档数量过多，且多份早期文档重复描述架构规则，不利于 Codex 按固定入口开发。

Change: Consolidated early architecture, security, Rust profile, validation, and review notes into `25_Core_Architecture_Reference.md`. The active Markdown entry set is now `24`, `22`, `23`, `25`, `14`, and `21`.

中文变更：将早期架构、安全、Rust 画像、验收清单和评审说明合并到核心架构参考文档。当前主动阅读入口压缩为六份核心 Markdown 文档。

## Review round 12 - Personal guarded live checklist audit / 个人小额受控清单审计

Finding: The personal guarded live pilot checklist needed tighter evidence binding, complete kill switch dimensions, stricter real-signing wording, and clearer separation between personal risk acceptance and external review.

中文发现：个人小额受控试运行清单需要更严格绑定证据索引、补齐熔断维度、收紧真实签名表述，并明确个人风险接受不等于外部审查通过。

Change: Updated `personal_guarded_live_pilot_checklist.md`, `personal_guarded_live_evidence_index.md`, and `personal_guarded_live_governance.md`; added `personal_guarded_live_checklist_audit_report.md`.

中文变更：更新个人试运行清单、证据索引和治理说明，并新增个人清单审计报告。

## Review round 13 - Evidence collection usability / 证据收集可操作性

Finding: The owner needed a step-by-step guide for how to start safe offline checks, where each evidence item comes from, how to redact it, and how to fill the evidence index and checklist.

中文发现：所有者需要逐项教程，说明如何启动安全离线检查、每个证据从哪里来、如何脱敏，以及如何填写证据索引和清单。

Change: Added `personal_guarded_live_evidence_collection_guide.md` and linked it from the governance profile and evidence index.

中文变更：新增个人小额受控试运行证据收集教程，并在治理说明和证据索引中引用。

## Review round 14 - Chinese-first documentation entry / 中文优先文档入口

Finding: The document set was too hard to enter for Chinese users because the README was English-first and the active docs were numerous.

中文发现：文档集合对中文用户不够友好，README 以英文为主，且主动文档数量较多，容易让用户误以为必须全部读完。

Change: Added `docs/00_Start_Here_CN.md`, rewrote `README.md` as a Chinese-first entry, and referenced the Chinese entry from the Codex runbook.

中文变更：新增中文用户入口，重写 README 为中文优先入口，并在 Codex 运行手册中引用中文入口。

## Review round 15 - Evidence guide Chinese localization / 证据教程中文化

Finding: `personal_guarded_live_evidence_collection_guide.md` still used too much English in headings, table columns and per-evidence instructions.

中文发现：个人小额受控试运行证据收集教程在标题、表头和逐项说明中仍有过多英文，中文用户需要额外推断。

Change: Rewrote the guide and its directly related personal-path documents as Chinese-first while keeping evidence IDs, commands and fixed status values stable.

中文变更：将证据收集教程及其直接相关的个人路径文档重写为中文优先版本，同时保留 evidence ID、命令和固定状态值的稳定性。

## Review round 16 - Personal path cross-document Chinese alignment / 个人路径关联文档中文对齐

Finding: The personal evidence collection guide is referenced from external-readiness review documents, but those documents still presented several entry sections, tables, and path-selection rules in English-first form.

中文发现：个人证据收集教程会被外部实盘准备评审文档引用，但这些文档的入口段落、表格和路径选择规则仍有多处英文优先，中文用户容易混淆“个人风险接受”和“外部审查通过”。

Change: Rewrote `controlled_live_readiness_checklist.md` and `controlled_live_readiness_review.md` as Chinese-first for the sections that connect external review, personal guarded-live governance, and evidence collection.

中文变更：将受控实盘准备清单和评审材料中连接外部审查、个人小额受控治理和证据收集的内容改为中文优先，同时保留 `Pass`、`Missing`、`Pending external review` 等固定状态值。
