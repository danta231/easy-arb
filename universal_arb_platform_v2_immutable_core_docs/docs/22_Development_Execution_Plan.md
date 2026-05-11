# 项目开发执行计划 / Development Execution Plan

本文档是正式开发的执行计划。它把模块拆分、阶段目标、交付点、测试方案和验收方案放在同一处，开发时应按阶段推进。除 crate 名称、命令和 schema 文件名外，本文尽量使用中文描述。

模块化细节请同时阅读 `23_Module_Architecture_Map.md`。执行计划回答“每个阶段做什么”，模块化架构地图回答“每个模块归谁、接收什么、输出什么、禁止什么、如何验收”。

如果后续全程使用 Codex 开发，请把 `24_Codex_Development_Runbook.md` 作为每次任务入口。该文档把本执行计划转换成 Codex 可直接领取的任务包、提示词、验证命令和交接格式。

架构背景和不可变规则已合并到 `25_Core_Architecture_Reference.md`。本执行计划只保留开发阶段、交付点、测试方案和验收方案。

## 1. 总体执行规则

1. 先合同，后流程：先实现 schema 对应类型、样例和校验，再实现风控、执行和账本流程。
2. 先只读和模拟，后实盘：只读数据和模拟执行未跑通前，不开发真实账户变更能力。
3. 先回放，后优化：所有核心流程必须能用固定事件和固定配置回放。
4. 策略永远只读：策略不能下单、签名、转账、改保证金、写账本。
5. 账本不能改历史：所有修正都必须追加冲销或调整分录。
6. 未知状态按风险处理：外部状态未知时，不能假设成功或继续交易。
7. 运行时只做装配：`arb-runtime` 连接模块，不承载核心业务规则。

## 2. 最终模块清单

```text
crates/
  arb-domain/
  arb-contracts/
  arb-config/
  arb-eventstore/
  arb-ledger/
  arb-reconciliation/
  arb-risk/
  arb-execution/
  arb-strategy-api/
  arb-strategies/
  arb-venue-data/
  arb-venue-exec/
  arb-signing/
  arb-replay/
  arb-ops/
  arb-runtime/
fixtures/
  schema/
    valid/
    invalid/
  replay/
xtask/
```

中文说明：`arb-venue-data` 和 `arb-venue-exec` 必须分开。前者只读，后者才可能在后续阶段包含账户变更能力。`arb-signing` 只做受控签名边界，阶段 6 之前不实现真实签名。

## 3. 模块边界和依赖规则

### 3.1 允许的主依赖方向

| 模块 | 允许依赖 |
|---|---|
| `arb-domain` | 无内部依赖 |
| `arb-contracts` | `arb-domain` |
| `arb-config` | `arb-domain`, `arb-contracts` |
| `arb-eventstore` | `arb-domain`, `arb-contracts` |
| `arb-ledger` | `arb-domain`, `arb-contracts` |
| `arb-reconciliation` | `arb-domain`, `arb-contracts`, `arb-ledger` |
| `arb-risk` | `arb-domain`, `arb-contracts`, `arb-config` |
| `arb-execution` | `arb-domain`, `arb-contracts`, `arb-config` |
| `arb-strategy-api` | `arb-domain`, `arb-contracts`, `arb-config` 的只读类型 |
| `arb-strategies` | `arb-strategy-api` |
| `arb-venue-data` | `arb-domain`, `arb-contracts`, `arb-config` |
| `arb-venue-exec` | `arb-domain`, `arb-contracts`, `arb-config`, `arb-signing` |
| `arb-signing` | `arb-domain`, `arb-contracts`, `arb-config` |
| `arb-replay` | `arb-domain`, `arb-contracts`, `arb-config`, `arb-eventstore`, `arb-risk`, `arb-execution`, `arb-strategy-api` |
| `arb-ops` | `arb-domain`, `arb-contracts`, `arb-eventstore`, `arb-ledger`, `arb-reconciliation` |
| `arb-runtime` | 负责装配，可依赖多个模块 |

### 3.2 明确禁止的依赖

| 模块 | 禁止依赖 |
|---|---|
| `arb-strategies` | `arb-execution`, `arb-venue-exec`, `arb-signing`, `arb-ledger`, `arb-runtime` |
| `arb-strategy-api` | `arb-execution`, `arb-venue-exec`, `arb-signing`, `arb-ledger`, `arb-runtime` |
| `arb-risk` | `arb-execution`, `arb-venue-exec`, `arb-signing`, `arb-runtime` |
| `arb-ledger` | `arb-risk`, `arb-execution`, `arb-venue-exec`, `arb-signing`, `arb-runtime` |
| `arb-venue-data` | `arb-venue-exec`, `arb-signing`, `arb-strategies` |
| `arb-ops` | `arb-venue-exec` 的实盘实现、`arb-signing` 的真实签名实现 |
| `arb-replay` | 外部 API、实盘签名、实盘账户写入 |

### 3.3 边界验收方式

- 使用 `cargo metadata` 生成依赖图。
- 写脚本检查禁止依赖表。
- 任何违反边界的依赖都必须让测试失败。
- 依赖检查应在阶段 0 完成，并在所有后续阶段保持开启。

## 4. 全局质量门

Rust workspace 创建后，每个阶段都必须保持以下命令通过：

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

文档和 schema 检查：

```bash
cargo xtask check-schema
cargo xtask check-crate-boundaries
cargo xtask check-docs
```

后续必须增加的检查：

- JSON Schema 正反样例校验。
- 合同类型 JSON 往返测试。
- crate 依赖边界测试。
- 回放黄金测试。
- 账本性质测试。
- 状态机非法迁移测试。

## 5. 阶段 0：工程骨架和边界

### 目标

创建 Rust workspace、空模块、基础命令和依赖边界检查。此阶段不写业务逻辑。

### 需要完成的内容

- 创建根 `Cargo.toml`。
- 创建所有目标 crate。
- 每个 crate 提供最小 `lib.rs` 或 `main.rs`。
- 新建 Rust `xtask/`，存放本地检查入口。
- 必要时新增 `.cargo/config.toml`，只用于提供 `cargo xtask ...` 本地 alias。
- 新建 `fixtures/schema/valid`、`fixtures/schema/invalid`、`fixtures/replay`。
- 增加依赖边界检查命令，至少覆盖策略、风控、账本、只读适配器。
- 设置默认功能开关：默认不能编译出实盘执行和真实签名路径。

### 交付点

- `Cargo.toml`
- `.cargo/config.toml`
- `crates/*/Cargo.toml`
- `xtask/`
- `fixtures/schema/valid/`
- `fixtures/schema/invalid/`
- `fixtures/replay/`

### 测试方案

- 运行格式化检查。
- 运行静态检查。
- 运行空 workspace 测试。
- 运行 `cargo xtask check-schema`。
- 运行 `cargo xtask check-docs`。
- 人为添加一个非法依赖，确认 `cargo xtask check-crate-boundaries` 能失败；再移除非法依赖。

### 验收方案

- `cargo test --workspace` 通过。
- `cargo xtask check-schema` 通过。
- `cargo xtask check-crate-boundaries` 能发现非法依赖。
- 默认构建不包含实盘执行功能。

## 6. 阶段 1：领域类型、合同类型和配置边界

### 目标

把文档中的 schema 和基础领域概念落成 Rust 类型，同时建立配置读取边界。

### 需要完成的内容

- 在 `arb-domain` 中实现 ID 包装类型。
- 在 `arb-domain` 中实现金额、价格、收益、利率、基点等十进制类型。
- 在 `arb-domain` 中实现执行状态、风控状态、事故状态、资本预留状态等枚举。
- 在 `arb-contracts` 中实现所有 schema 对应结构体。
- 在 `arb-contracts` 中实现严格反序列化，拒绝未知字段。
- 在 `arb-contracts` 中实现规范序列化辅助函数。
- 在 `arb-config` 中实现配置结构、配置版本、配置哈希、签名引用、kill switch 读取。
- 建立每个 schema 的正例和反例 fixture。

### 交付点

- `crates/arb-domain/src/lib.rs`
- `crates/arb-contracts/src/lib.rs`
- `crates/arb-config/src/lib.rs`
- `fixtures/schema/valid/*.json`
- `fixtures/schema/invalid/*.json`

### 测试方案

- 每个 ID 包装类型测试：不能误把 `VenueId` 当作 `AssetId` 使用。
- 每个十进制类型测试：字符串往返不丢精度。
- 每个枚举测试：未知枚举值被拒绝或进入明确错误。
- 每个 schema 正例测试：可反序列化、可序列化。
- 每个 schema 反例测试：缺字段、未知字段、错误 decimal、错误枚举都失败。
- 配置测试：配置版本、哈希、签名引用、kill switch 能读取。

### 验收方案

- 核心合同都包含必需的 `schema_version`。
- 核心金额路径没有使用 `f64`。
- 所有 schema 至少有一个正例和一个反例。
- 配置读取不会触发风控、执行或签名。

## 7. 阶段 2：事件存储、规范哈希和回放基础

### 目标

建立追加式事件存储和确定性回放输入能力。

### 需要完成的内容

- 在 `arb-eventstore` 中实现追加式 JSONL 事件写入。
- 为事件分配全局递增序号。
- 计算规范事件哈希。
- 支持按序号读取事件。
- 支持按关联 ID 查询事件链。
- 在 `arb-replay` 中实现回放输入加载。
- 建立 `fixtures/replay/minimal_smoke/`。

### 交付点

- `crates/arb-eventstore/src/lib.rs`
- `crates/arb-replay/src/lib.rs`
- `fixtures/replay/minimal_smoke/events.jsonl`
- `fixtures/replay/minimal_smoke/config.yaml`
- `fixtures/replay/minimal_smoke/expected/`

### 测试方案

- 追加写入测试：事件只能追加，不能覆盖。
- 序号测试：同一存储中序号严格递增。
- 哈希测试：同一事件多次计算哈希一致。
- 读取测试：按序号读取顺序稳定。
- 回放加载测试：同一 fixture 多次加载结果一致。
- 损坏事件测试：错误 JSON、错误 schema、错误哈希必须失败。

### 验收方案

- 事件存储可作为唯一事实来源。
- 回放不依赖系统当前时间。
- 回放不访问外部网络。
- 相同输入得到相同事件顺序和哈希。

## 8. 阶段 3：账本核心

### 目标

实现追加式复式账本，确保经济事实可审计、可对账、可调整。

### 需要完成的内容

- 在 `arb-ledger` 中实现账本入账接口。
- 实现借贷平衡检查。
- 实现实盘、模拟、回测、调整四类账本命名空间。
- 实现冲销分录。
- 实现调整分录。
- 实现按账户、资产、策略、机会、执行计划查询账本视图。
- 建立账本正反 fixture。

### 交付点

- `crates/arb-ledger/src/lib.rs`
- `fixtures/schema/valid/ledger_entry.*.json`
- `fixtures/schema/invalid/ledger_entry.*.json`

### 测试方案

- 平衡分录测试：借贷平衡时可入账。
- 不平衡分录测试：不平衡时拒绝。
- 冲销测试：冲销必须追加新分录，不能改原分录。
- 调整测试：调整必须记录被调整对象。
- 命名空间测试：模拟账本不能混入实盘命名空间。
- 查询测试：按策略、账户、资产查询结果正确。

### 验收方案

- 账本不提供改写历史的接口。
- 所有账本写入都能追溯 `source_event_id`。
- 不平衡分录无法通过测试。
- 模拟和实盘账本隔离。

## 9. 阶段 4：策略只读接口和样例策略

### 目标

证明策略只能读取标准化状态并输出候选组合转换。

### 需要完成的内容

- 在 `arb-strategy-api` 中定义策略接口。
- 定义只读输入快照：市场状态、组合状态、场所健康、配置快照、固定时间源。
- 定义策略输出：候选组合转换、策略诊断事件、策略拒绝事件。
- 在 `arb-strategies` 中实现一个最小样例策略。
- 加入策略依赖边界测试。

### 交付点

- `crates/arb-strategy-api/src/lib.rs`
- `crates/arb-strategies/src/lib.rs`
- `fixtures/replay/strategy_smoke/`

### 测试方案

- 只读输入测试：策略无法获得可变账户、执行和签名接口。
- 候选转换测试：策略输出能通过合同校验。
- 固定时间源测试：同一时间源下输出稳定。
- 依赖边界测试：策略 crate 引入执行或签名依赖时测试失败。
- 回放测试：相同事件和配置产生相同候选转换。

### 验收方案

- 策略不能编译出下单、签名、转账、账本写入路径。
- 策略输出包含输入事件引用、策略版本、配置版本和假设。
- 样例策略可在回放中稳定运行。

## 10. 阶段 5：风控引擎

### 目标

所有候选转换必须先经过风控，风控只输出决策，不执行。

### 需要完成的内容

- 在 `arb-risk` 中实现风控策略加载结果的评估逻辑。
- 检查策略版本、配置版本、输入状态引用。
- 实现数据新鲜度检查。
- 实现流动性检查。
- 实现余额和资本预留检查。
- 实现保证金和清算距离检查。
- 实现场所健康和限频检查。
- 实现未知状态处理。
- 输出 `RiskDecision`。

### 交付点

- `crates/arb-risk/src/lib.rs`
- `fixtures/replay/risk_accept/`
- `fixtures/replay/risk_reject/`
- `fixtures/replay/risk_requires_more_data/`

### 测试方案

- 通过用例：输入完整且风险在阈值内时通过。
- 拒绝用例：余额不足、数据陈旧、滑点过高、场所不健康时拒绝。
- 缺数据用例：缺关键输入时返回需更多数据或拒绝。
- 未知状态用例：未知状态不能返回批准。
- 约束用例：通过但需要限制时返回带约束的决策。
- 解释性测试：每个决策必须有检查项、阈值、观察值和原因码。

### 验收方案

- 风控模块不依赖执行模块。
- 风控模块不直接写账本。
- 每个候选转换都有明确风控结果。
- 任何未知关键状态都不能批准。

## 11. 阶段 6：执行计划、资本预留和模拟执行

### 目标

在不触碰真实账户的前提下跑通从风控决策到执行报告、模拟成交和模拟账本的闭环。

### 需要完成的内容

- 在 `arb-execution` 中实现执行计划构建器。
- 实现执行腿状态机。
- 实现资本预留状态机。
- 实现只读模式：只记录将会发生什么，不产生账户变更动作。
- 实现模拟模式：产生模拟成交和模拟执行报告。
- 将模拟结果转换为账本入账请求或模拟账本分录。
- 实现部分成交、超时、撤单、未知状态和补偿路径。

### 交付点

- `crates/arb-execution/src/lib.rs`
- `fixtures/replay/execution_read_only/`
- `fixtures/replay/execution_simulated_success/`
- `fixtures/replay/execution_partial_fill/`
- `fixtures/replay/execution_unknown_state/`

### 测试方案

- 只读模式测试：不能产生任何账户变更动作。
- 模拟成功测试：产生模拟成交、执行报告和模拟账本。
- 部分成交测试：按部分成交策略处理残余敞口。
- 超时测试：超时进入失败或未知状态。
- 未知状态测试：触发事故或人工处理路径。
- 非法状态迁移测试：状态机拒绝非法跳转。
- 资本预留测试：预留、转执行、释放、过期、对账差异都可表达。

### 验收方案

- 阶段 6 通过前，不允许开发实盘执行实现。
- 执行模块不直接持有密钥。
- 执行结果可生成账本输入。
- 所有异常路径都有事件、报告或事故记录。

## 12. 阶段 7：对账、事故和运营报告

### 目标

让系统即使没有前端，也能通过结构化记录、对账、事故和报告进行运营。

### 需要完成的内容

- 在 `arb-reconciliation` 中实现余额对账。
- 实现仓位对账。
- 实现成交和账本对账。
- 实现资本预留对账。
- 在 `arb-ops` 中实现日报生成。
- 在 `arb-ops` 中实现风控拒绝报告。
- 在 `arb-ops` 中实现事故报告。
- 生成事故记录，覆盖未知状态和对账差异。

### 交付点

- `crates/arb-reconciliation/src/lib.rs`
- `crates/arb-ops/src/lib.rs`
- `fixtures/replay/reconciliation_match/`
- `fixtures/replay/reconciliation_mismatch/`
- `fixtures/replay/incident_unknown_state/`

### 测试方案

- 对账一致测试：账本、余额、仓位一致时通过。
- 对账差异测试：差异被记录并产生事故。
- 事故测试：事故包含级别、影响范围、触发原因、自动动作和人工动作。
- 报告测试：日报由结构化记录生成，不手写事实。
- 查询测试：可按事件、策略、场所、账户追踪。

### 验收方案

- 对账不是运营工具的附属逻辑，而是独立核心模块。
- 每个事故都能追溯事件 ID。
- 报告和事故不包含密钥或敏感签名材料。
- 对账差异不能被静默忽略。

## 13. 阶段 8：只读场所数据适配器

### 目标

接入一个真实场所的公开数据或只读账户数据，但不启用任何可变账户能力。

### 需要完成的内容

- 在 `arb-venue-data` 中选择第一个只读场所。
- 实现公开行情采集。
- 实现断线和重连状态记录。
- 实现限频状态记录。
- 实现场所健康事件。
- 输出原始事件和标准化事件。
- 将场所能力写入配置。

### 交付点

- `crates/arb-venue-data/src/lib.rs`
- 第一个场所的只读适配器模块。
- `fixtures/replay/venue_data_smoke/`
- 场所能力配置样例。

### 测试方案

- 解析测试：样例行情响应可解析。
- 标准化测试：原始数据可转标准化事件。
- 断线测试：断线产生健康事件。
- 限频测试：限频信息被记录。
- 只读边界测试：`arb-venue-data` 不能依赖 `arb-signing` 和 `arb-venue-exec`。
- 离线测试：无网络、无 API key 时，fixture 解析、标准化、回放和边界检查仍通过。

### 验收方案

- 没有下单、撤单、转账、签名路径。
- 数据 stale 能触发风险标记。
- 适配器产生的事件可进入回放。
- 真实网络检查必须显式 opt-in，不能替代离线 fixture 验收。

## 14. 阶段 9：端到端模拟演练

### 目标

用真实或近真实的只读数据，跑通从事件、组合状态、策略、风控、执行计划、模拟执行、账本、对账、报告的完整闭环。

### 需要完成的内容

- 在 `arb-runtime` 中装配只读数据、事件存储、策略、风控、执行模拟、账本、对账和报告。
- 建立完整回放 fixture。
- 生成期望候选转换、风控决策、执行计划、执行报告、账本分录、事故和报告。
- 增加一键回放命令。

### 交付点

- `crates/arb-runtime/src/main.rs`
- `fixtures/replay/full_pipeline_simulated/`
- 一键回放脚本。

### 测试方案

- 完整闭环测试：输入事件到最终报告全链路通过。
- 重复回放测试：多次运行输出一致。
- 异常路径测试：插入陈旧数据、余额不足、未知状态，验证拒绝和事故。
- 命名空间测试：模拟账本不污染实盘命名空间。

### 验收方案

- 不访问实盘可变接口。
- 所有关键中间结果都有事件或 fixture 输出。
- 同一 fixture 输出完全一致。
- 失败路径可解释。

## 15. 阶段 10：人工审批模式

### 目标

允许系统准备执行计划，但必须有显式人工审批事件才可继续调度。

### 需要完成的内容

- 实现人工审批模式关卡。
- 实现审批事件合同使用。
- 实现执行计划预览报告。
- 实现审批通过、拒绝、过期路径。
- 审批事件进入事件存储和审计报告。

### 交付点

- 人工审批模式代码。
- `fixtures/replay/manual_approval_approved/`
- `fixtures/replay/manual_approval_rejected/`
- 执行计划预览报告样例。

### 测试方案

- 无审批测试：不能调度执行。
- 审批通过测试：可进入下一阶段流程。
- 审批拒绝测试：必须停止并记录原因。
- 审批过期测试：过期后不能继续执行。
- 审计测试：审批记录可追溯。

### 验收方案

- 没有审批事件就没有执行调度。
- 审批不能绕过风控。
- 审批记录进入审计事件。

## 16. 阶段 11：可变执行适配器和签名边界预演

### 目标

只实现边界和模拟实现，不接真实资金。为后续受控实盘做准备。

### 需要完成的内容

- 在 `arb-signing` 中定义签名 trait 和空实现。
- 在 `arb-venue-exec` 中定义下单、撤单、转账、交易提交 trait。
- 使用模拟签名器和模拟执行适配器跑测试。
- 增加功能开关，默认关闭真实执行和真实签名。
- 增加编译测试，确认不开启功能时不会编译真实路径。

### 交付点

- `crates/arb-signing/src/lib.rs`
- `crates/arb-venue-exec/src/lib.rs`
- 模拟执行适配器。
- 功能开关配置。

### 测试方案

- 默认构建测试：不包含真实签名和实盘执行。
- 模拟签名测试：只产生签名引用，不产生真实签名材料。
- 执行 trait 测试：执行模块只能通过 trait 调用可变适配器。
- 权限测试：策略和 ops 不能依赖签名实现。

### 验收方案

- 默认编译安全。
- 真实签名实现不存在或不可用。
- 可变执行路径需要显式功能开关。
- 策略仍无法触达执行和签名。

## 17. 阶段 12：受控实盘准备评审

### 目标

不是立即上线实盘，而是形成受控实盘前的检查包。

### 需要完成的内容

- 完成外部安全、风控、账本和执行审查材料。
- 证明 kill switch 覆盖全局、策略、场所、账户、工具、资产、链、执行模式。
- 证明 API key 无提现权限。
- 证明热钱包资金受限。
- 证明未知状态会暂停相关执行。
- 证明对账和事故响应已演练。

### 交付点

- 受控实盘准备报告。
- kill switch 演练记录。
- 权限配置检查记录。
- 对账演练记录。
- 事故响应演练记录。

### 测试方案

- kill switch 测试：每个层级都能阻断执行。
- 权限测试：无提现权限。
- 小额限制测试：超过限额被拒绝。
- 未知状态测试：立即暂停相关执行。
- 对账差异测试：差异产生事故并暂停相关策略或场所。

### 验收方案

- 外部审查完成前不能进入实盘。
- 所有实盘能力默认关闭。
- 所有实盘动作可追溯、可停止、可对账。
- 事故响应流程已演练。

个人自用例外说明：

- 若系统仅由所有者本人使用自己的小额资金，允许将“外部审查”替换为 `review/personal_guarded_live_governance.md` 中的个人小额受控试运行最低门槛；该路径只允许 `GuardedLivePersonal`，不允许自动实盘。
- 个人路径不是阶段 12 外部评审通过，也不能用于他人资金、团队资金、客户资金或商业服务。
- 个人路径仍要求所有实盘能力默认关闭、无提现权限、隔离账户、小额资金上限、每笔人工确认、kill switch、未知状态停机、执行后强制对账和事故记录。
- 任何真实执行实现仍必须作为新的显式任务包创建，且不得把 AI 辅助自审记为外部独立审查。

## 18. 阶段依赖关系

必须顺序完成：

```text
阶段 0 -> 阶段 1 -> 阶段 2 -> 阶段 3 -> 阶段 4 -> 阶段 5 -> 阶段 6
```

阶段 7 可以在阶段 6 后开始，也可以和阶段 8 部分并行，但对账核心必须先于实盘准备完成。

阶段 8 只能在阶段 2 完成后启动，因为只读适配器必须能写入事件并可回放。

阶段 9 必须在阶段 6 和阶段 7 完成后启动。

阶段 11 只能在阶段 9 完成后启动。

阶段 12 必须在阶段 11 完成后启动，并且需要独立审查。

## 19. 第一批立即执行任务

第一批任务只覆盖阶段 0 和阶段 1 的开头：

1. 创建 workspace。
2. 创建所有 crate 空壳。
3. 建立 schema 解析脚本。
4. 建立依赖边界脚本。
5. 实现 `arb-domain` 的 ID 包装和十进制类型。
6. 建立第一批 schema 正反 fixture。

禁止事项：

- 不接真实交易 API。
- 不实现真实签名。
- 不实现真实下单。
- 不把策略和执行写在同一个模块。
- 不把对账塞进报告工具里。
