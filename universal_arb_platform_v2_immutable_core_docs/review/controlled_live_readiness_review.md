# 受控实盘准备评审材料 / Controlled Live Readiness Review

评审包：`S12-01`
当前阶段：阶段 12：受控实盘准备评审
生成日期：2026-05-11
架构版本：`v2-immutable-core`

中文说明：本文是受控实盘前的评审材料，不是实盘上线批准。本文不包含真实
API key、私钥、助记词、token、钱包签名、真实账户余额、真实订单或真实资金
动作。外部审查完成并签署前，本项目不能进入实盘，也不能创建真实执行实现任务。

## 结论 / Executive Conclusion

决定：**当前不要创建真实执行实现任务。**

中文结论：阶段 12 只生成准备评审材料；外部安全、风控、账本、执行和运营审查尚未
完成。真实权限、真实密钥托管、热钱包限额和事故演练不能用本仓库离线 fixture 代替。

个人路径说明：如果本系统只由所有者本人使用自己的小额封顶资金，可以选择
`personal_guarded_live_governance.md` 的个人小额受控试运行路径。该路径不是外部审查
通过，不允许自动实盘，仍必须满足无提现权限、隔离资金、人工确认、熔断、未知状态
停机、强制对账和事故记录。证据收集顺序以
`personal_guarded_live_evidence_collection_guide.md` 为准。

允许的下一步：

- 完成独立外部审查，并附脱敏签署引用。
- 如果仅为所有者本人小额自用，完成 `personal_guarded_live_governance.md`、证据教程、
  证据索引和个人清单；记录为个人风险接受，而不是外部审计。
- 执行 `controlled_live_readiness_checklist.md` 中的外部审查清单。
- 保持所有实盘执行和真实签名功能默认关闭。
- 只有全部必要审查门为 `Pass` 后，才可以提出真实执行实现设计任务。

不允许的事情：

- 不实现真实下单、撤单、转账、提现、保证金变更或真实签名。
- 不使用交易凭证连接真实场所。
- 不把 secret、token、私钥、webhook token、session token 或钱包助记词写进代码、
  日志、fixture、报告或 issue 文本。
- 不削弱或删除熔断、feature flag、权限、账本、对账或回放检查。

## 证据目录 / Evidence Inventory

| 领域 | 证据 | 当前状态 | 中文说明 |
|---|---|---:|---|
| 必读文档 | `docs/24`, `22`, `23`, `25`, `14`, `21` | Reviewed | 本评审按 Codex runbook 的必读顺序执行。 |
| 实盘默认关闭配置 | `templates/config.template.yaml` | Present | `mode: ReadOnly`, `live_execution_enabled: false`, `auto_live_enabled: false`, `real_signing_enabled: false`。 |
| 权威配置模板 | `universal_arb_platform_v2_immutable_core_docs/templates/config.template.yaml` | Present | `execution_mode: ReadOnly`, venue `enabled: false`, `can_trade: false`, `can_withdraw: false`。 |
| Feature gates | `crates/arb-venue-exec/Cargo.toml`, `crates/arb-signing/Cargo.toml` | Present | `default = []`, `live-exec = []`, `real-signing = []`。 |
| 运行时回放 | `fixtures/replay/full_pipeline_simulated/` | Present | 离线、模拟、脱敏、确定性回放 fixture。 |
| 未知状态 fixture | `fixtures/replay/incident_unknown_state/` and full-pipeline `cases/unknown_state/` | Present | 未知状态会生成事故证据，不能被当作成功。 |
| 对账演练 | `fixtures/replay/reconciliation_match/`, `fixtures/replay/reconciliation_mismatch/` | Present | 覆盖匹配和不匹配路径；不匹配会生成可追踪事故输出。 |
| 人工审批演练 | `fixtures/replay/manual_approval_approved/`, `fixtures/replay/manual_approval_rejected/` | Present | 人工批准只释放同一个 plan hash 的人工门。 |
| 脱敏测试 | `crates/arb-ops/src/lib.rs`, `crates/arb-signing/src/lib.rs` | Present | 报告前会脱敏自由文本和签名引用。 |
| 外部审计 | External reviewer sign-off | Missing | 缺少外部审查是进入实盘的硬阻塞。 |

## 安全审查 / Safety Review

必要不变量：危险能力必须同时被 crate 边界、trait 边界、Cargo feature、运行时配置、
熔断、权限模型、审计事件、账本、对账和事故流程阻断。

中文说明：危险能力不能只靠一个配置字段保护。真实执行和真实签名必须同时受包边界、
接口边界、编译功能开关、运行时配置、熔断、权限、审计事件、账本、对账和事故流程约束。

已审查控制：

- `arb-venue-exec` 和 `arb-signing` 的默认 Cargo features 为空。
- `LIVE_EXEC_FEATURE_ENABLED` 和 `REAL_SIGNING_FEATURE_ENABLED` 暴露显式 feature 状态，便于检查。
- `arb-config` 会拒绝敏感字段，以及与 `ReadOnly` 冲突的 live flag。
- `arb-ops` 是只读的，不暴露订单、撤单、转账、签名或账本变更命令。
- `arb-replay` 和全链路 fixture 是离线的，不能调用外部 API。

开放阻塞：

- 没有外部安全审查签署。
- 真实密钥托管、轮换、紧急撤销、IP allowlist 或权限导出尚未审查。
- 生产部署环境和监控证据尚未审查。

通过前需要的外部证据：

- 脱敏权限导出，证明 API key 不能提现。
- 脱敏签名策略，证明真实签名在批准前保持关闭或受独立门控。
- 脱敏部署配置，证明实盘执行和自动实盘默认关闭。
- 配置变更、策略启用、adapter 启用、key 轮换和人工覆盖的审计事件样例。

## 风控审查 / Risk Review

必要不变量：`RiskDecision::Rejected` 不能产生可执行计划；未知外部状态必须 fail closed；
审批仍受执行模式、资本预留、熔断和权限约束。

中文说明：风控批准不是执行豁免。即使风控批准，执行仍必须被执行模式、资本预留、
人工审批、熔断、权限、账本和对账继续拦截。

已审查控制：

- 数据新鲜度、场所健康、流动性、费用或滑点、保证金、资本预留、单日亏损、余额、未知状态和风控标记检查已体现在回放输出中。
- 陈旧数据和余额不足场景会在可执行分发前拒绝并停止。
- 未知状态场景会产生事故，并要求操作员流程介入。

开放阻塞：

- 真实场所限额表、live 名义金额上限、策略上限、账户上限和单日亏损阈值尚未独立审查。
- live 监控阈值和告警接收人尚未外部验证。
- 没有外部审查人签署紧急停止覆盖在运营上可达。

## 账本审查 / Ledger Review

必要不变量：账本事实是 append-only 的复式记录；更正必须通过冲销或调整完成，不能重写历史。

已审查控制：

- 账本 schema 和 fixture 要求分录平衡，并带调整原因码。
- 回放预期输出包含模拟命名空间账本分录。
- 对账不一致 fixture 会生成可追踪事故，而不是静默改写账本历史。

开放阻塞：

- 没有外部会计审查签署。
- live 账本命名空间尚未批准启用。
- 真实场所成交、手续费、资金费、转账和结算到会计科目的映射尚未审查。

## 执行审查 / Execution Review

必要不变量：执行计划只能来自允许的风控决策；可变 live execution 在明确审查和批准前不可用。

已审查控制：

- `arb-execution` 支持模拟执行报告，以及 unknown、partial、failure 状态语义。
- `arb-venue-exec` 只定义可变执行边界和模拟行为；默认构建不提供真实实盘执行。
- `arb-signing` 提供 null/default-deny 签名边界；默认构建不提供真实签名。
- 人工审批材料说明：审批不能绕过账本、对账、熔断或执行权限。

开放阻塞：

- 真实场所 adapter 没有外部审查。
- 真实订单幂等行为没有在场所 sandbox 或生产只读镜像中测试。
- 生产回滚或场所禁用演练没有外部签署。

## 对账审查 / Reconciliation Review

必要不变量：执行报告之后必须对账；未解决差异必须生成事故。

已审查控制：

- `reconciliation_match` 证明干净路径不会产生误报事故。
- `reconciliation_mismatch` 证明不一致会生成可追踪事故。
- 全链路模拟回放包含对账输出和日常运营报告。

开放阻塞：

- 真实场所余额、仓位、成交导出格式尚未审查。
- 生产对账计划、升级阈值和未解决差异负责人尚未外部批准。
- 没有证据证明真实不一致可以停止受影响的策略、场所、账户、工具、资产、链或执行模式。

## 回放审查 / Replay Review

必要不变量：回放使用固定事件、固定配置、固定策略和风控版本、固定排序，并且不调用外部 API。

已审查控制：

- 回放 fixture 包含事件、配置、策略 manifest、风控策略、预期决策、执行计划、报告、账本分录、对账报告、事故和运营报告。
- 事件排序遵循 `docs/21` 的 sequence-first replay 规则。
- 脱敏公开市场数据 fixture 由事件 payload 引用。

开放阻塞：

- live-readiness 回放不能证明真实场所行为；它只能证明确定性离线安全行为。
- 外部审查人仍需批准回放证据集，之后才可以创建任何真实实现任务。

## 权限、密钥与秘密审查 / Permissions, Keys, and Secrets Review

必要不变量：凭证和签名材料绝不能进入代码、日志、fixture、报告或 issue 文本。

已审查控制：

- 配置模板只保存 signing policy 引用。
- `arb-config` 会拒绝 API secret 等敏感配置字段。
- `arb-ops` 会脱敏 API key、secret、private key、token、bearer token 和相关自由文本标记。
- 场所 capability fixture 设置 `can_withdraw: false`；只读公开数据 fixture 设置 `can_trade: false`。

开放阻塞：

- 没有脱敏生产 API-key 权限导出。
- 热钱包最大余额、转账策略、gas 资金策略和紧急撤销流程尚未外部审查。
- 没有真实签名器证明。

## 事故流程审查 / Incident Workflow Review

必要不变量：未知状态、对账不一致、执行卡住、权限失败、签名失败和熔断激活必须产生可追踪事故或审计证据。

已审查控制：

- 事故 schema 要求源事件引用。
- 未知状态和对账不一致 fixture 包含预期事故。
- 运营报告会暴露开放事故和对账差异。

开放阻塞：

- 缺少外部事故 tabletop 演练。
- 值班负责人、沟通渠道、升级时间目标和复盘负责人没有外部签署。
- 没有生产告警投递证明。

## 熔断覆盖说明 / Kill Switch Coverage

必需覆盖层级：

- 全局。
- 执行分发。
- 策略。
- 场所。
- 账户。
- 工具或合约。
- 资产。
- 链。
- 执行模式。

当前证据：

- 运行时配置模板包含所有必需熔断维度。
- `arb-config` 测试包含阻断 `GuardedLive`、策略、场所和账户的场景。
- 文档要求未知状态和对账不一致暂停受影响执行。

外部通过标准：

- 审查人必须看到每个覆盖层级的脱敏配置证据。
- 操作员必须执行 tabletop drill，证明每个层级能阻断相关路径并生成审计证据。
- 任一层级失败或未测试，都阻塞真实执行实现任务。

## 演练记录 / Drill Records

| 演练 | 离线证据 | 必需外部证据 | 状态 |
|---|---|---|---:|
| 端到端回放 | `cargo xtask replay-full-pipeline` against `fixtures/replay/full_pipeline_simulated` | 审查人接受回放 artifact | Pending external review |
| 熔断 | `arb-config` tests and config template coverage | 全维度 tabletop 证明 | Pending external review |
| 权限检查 | Venue capability fixtures with `can_withdraw: false` | 脱敏场所权限导出 | Missing |
| 对账 | `reconciliation_match` and `reconciliation_mismatch` fixtures | 操作员演练证明 mismatch 会停止受影响范围 | Pending external review |
| 未知状态 | `incident_unknown_state` and full-pipeline unknown-state case | 操作员演练证明暂停和升级路径 | Pending external review |
| 事故响应 | Incident schema and operations report outputs | 带 owner、时间线、复盘的 tabletop 演练 | Missing |

## 本任务验证记录 / Current Task Verification Record

中文说明：下列命令是本任务完成前的验证门。本节记录本次 Codex 执行结果；
任何后续变更都必须重新运行。通过这些离线检查不等于外部审查通过。

| 检查 | 结果 |
|---|---:|
| `cargo fmt --all -- --check` | Pass |
| `cargo clippy --workspace --all-targets -- -D warnings` | Pass |
| `cargo test --workspace` | Pass |
| `cargo xtask check-schema` | Pass |
| `cargo xtask check-crate-boundaries` | Pass |
| `cargo xtask check-docs` | Pass |
| `cargo xtask replay-full-pipeline` | Pass |
| Feature gate inspection | Pass: `default = []` for `arb-venue-exec` and `arb-signing`; live/real signing features are explicit opt-in only. |
| Redaction inspection | Pass: report rendering and signing errors redact sensitive free text; raw market fixtures are marked redacted public data. |
| Kill switch inspection | Pass: config/runtime tests cover live blocking and template exposes required kill switch dimensions. |
| Dangerous default inspection | Pass with expected exceptions: `true` occurrences are limited to negative tests, guarded-live kill switch tests, or invalid fixtures. |
