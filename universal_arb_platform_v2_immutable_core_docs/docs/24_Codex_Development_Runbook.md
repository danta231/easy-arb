# Codex 开发运行手册 / Codex Development Runbook

本文档面向后续全程使用 Codex 开发的场景。它不是新的架构设计，而是把已有架构、模块边界和阶段计划转成 Codex 可直接执行的开发规程。

中文说明：后续每次让 Codex 开发时，应优先把本文档作为任务入口。Codex 需要先读取本文，再读取对应阶段和模块文档，然后再改代码。

## 1. 文档优先级

Codex 每次开始开发前，按以下顺序读取文档：

0. `universal_arb_platform_v2_immutable_core_docs/docs/00_Start_Here_CN.md`：中文用户入口。中文说明：人类用户可先读本文了解阅读路径；Codex 执行开发任务时仍以本 runbook 和任务包为准。
1. `universal_arb_platform_v2_immutable_core_docs/docs/24_Codex_Development_Runbook.md`：Codex 开发运行手册，即本文档。
2. `universal_arb_platform_v2_immutable_core_docs/docs/22_Development_Execution_Plan.md`：阶段目标、交付点、测试方案、验收方案。
3. `universal_arb_platform_v2_immutable_core_docs/docs/23_Module_Architecture_Map.md`：模块职责、输入输出、接口边界、测试责任。
4. `universal_arb_platform_v2_immutable_core_docs/docs/25_Core_Architecture_Reference.md`：核心架构、不可变原则、Rust 工程规则、安全治理和验收清单。
5. `universal_arb_platform_v2_immutable_core_docs/docs/14_Data_Schemas_and_Contracts.md`：数据合同和 schema 规则。
6. `universal_arb_platform_v2_immutable_core_docs/docs/21_State_Machines_and_Replay_Fixtures.md`：状态机和回放样例规则。

中文说明：不要只读阶段计划就开始写代码。阶段计划回答“做什么”，模块地图回答“边界在哪里”，核心架构参考回答“为什么这样设计”，schema 文档回答“数据合同是什么”。

路径约定：

- `REPO_ROOT`：当前仓库根目录。
- `DOC_ROOT`：`REPO_ROOT/universal_arb_platform_v2_immutable_core_docs`。
- `CODE_ROOT`：`REPO_ROOT`，Rust workspace、`crates/`、`fixtures/`、`xtask/` 默认都创建在这里。
- 文档中的 `docs/...` 短路径一律按 `DOC_ROOT/docs/...` 解析。
- 文档中的 `14`、`21`、`22`、`23`、`24`、`25` 短编号分别指向上方同编号文档。
- 文档中的 `schemas/...` 和 `templates/...` 短路径一律按 `DOC_ROOT/schemas/...`、`DOC_ROOT/templates/...` 解析，除非任务明确要求创建运行时代码侧副本。
- 若阶段提示词、任务提示词和本路径约定冲突，以本路径约定为准，先停止确认再改动。

## 2. Codex 总工作流

每个开发任务必须走同一套闭环：

```text
确认阶段和任务
  -> 读取相关文档
  -> 检查当前代码和文件状态
  -> 给出简短实施计划
  -> 小步修改
  -> 运行验证命令
  -> 修复验证失败
  -> 更新必要文档
  -> 输出交付说明
```

Codex 不应在未查看现有代码的情况下重写大块实现。若工作区已有用户改动，必须保留并兼容，不得擅自回退。

## 3. Codex 开发硬规则

1. 文档和代码注释不能只有英文；关键说明必须有中文。
2. 默认只开发只读、模拟和回放能力，不接真实资金。
3. 未完成阶段 9 前，不开发真实下单、撤单、转账、真实签名。
4. 策略模块不能依赖执行、签名、账本写入或运行时装配。
5. 只读场所模块不能依赖可变执行模块和签名模块。
6. 账本只能追加，修正必须用冲销或调整分录。
7. 外部未知状态必须按风险处理，不能当作成功。
8. 核心金额、价格、利率和收益禁止使用 `f64`。
9. 每个模块变更都必须有正向测试、反向测试或边界测试。
10. 若改动跨模块边界，必须同步检查 `universal_arb_platform_v2_immutable_core_docs/docs/23_Module_Architecture_Map.md` 是否仍准确。

安全门最低要求：

- `feature flag` 是 Cargo 功能开关；`crate` 是 Rust 包边界；`trait` 是接口边界。中文说明：危险能力必须同时受包边界、接口边界、编译开关和运行时配置控制。
- 默认 Cargo features 必须为空或只包含安全功能。真实执行必须放在 `live-exec` 之类的显式 feature 后面，真实签名必须放在 `real-signing` 之类的显式 feature 后面。
- 阶段 9 完成前，不能实现真实下单、撤单、转账、真实签名或可移动真实资金的路径。
- 阶段 11 只允许定义可变执行 trait、签名 trait、模拟实现和空签名器；不能接真实交易场所或真实密钥。
- 真实凭证缺失、权限未知、外部状态未知、feature 未开启、kill switch 打开时，都必须 fail closed，不能当作成功。
- 密钥、API secret、私钥、助记词、会话令牌和 webhook token 不能进入代码、日志、事件、fixture、报告或问题描述。

执行计划和人工审批语义：

- `RiskDecision::Rejected` 不能生成可执行计划。
- `RiskDecision::Approved` 可以生成执行计划，但仍受执行模式、kill switch、资本预留和运行时权限约束。
- `RiskDecision::NeedsManualApproval` 可以生成审批材料或带 `PendingManualApproval` 状态的计划预览，但不能分发执行。
- 人工审批只能批准同一个计划哈希，不能替代风控，不能修改计划后继续沿用旧审批。
- 审批通过后的下一步是解除该计划的人工审批阻塞，不是绕过账本、对账、kill switch 或执行权限。

## 4. 每次任务的输入模板

正式开发优先使用任务包编号入口，不需要同时复制通用提示词和专用提示词：

```text
请按 `universal_arb_platform_v2_immutable_core_docs/docs/24_Codex_Development_Runbook.md` 执行任务包 <任务包编号>。
只修改该任务包允许范围内的文件。
完成后按 runbook 的交付格式回复，并给出验证结果。
```

如果任务不属于已有任务包，再使用下面的自定义模板：

```text
请按 `universal_arb_platform_v2_immutable_core_docs/docs/24_Codex_Development_Runbook.md` 执行。

当前阶段：阶段 X：<阶段名称>
当前任务：<要完成的具体任务>
允许修改范围：<文件或模块范围>
禁止修改范围：<不允许触碰的模块、配置或文档>
验收要求：
- <验收项 1>
- <验收项 2>
- <验收项 3>

要求：
1. 先读取 Codex 运行手册、阶段计划和模块架构地图。
2. 开始改动前说明将修改哪些文件。
3. 完成后运行相关验证命令。
4. 最终说明改了什么、验证结果、剩余风险。
5. 文档说明不能为纯英文，需要有中文解释。
```

如果任务很小，也至少保留“当前阶段、当前任务、允许修改范围、验收要求”四项。

提示词优先级：

1. 本文档第 1 节路径约定、第 3 节硬规则和安全门。
2. 第 10 节任务包提示词。
3. 第 8 节阶段任务包摘要。
4. 第 9 节阶段级提示词。

中文说明：第 9 节和第 10 节保留为可复制材料，不作为两套事实源。若重复内容冲突，以更高优先级为准。

## 5. Codex 输出交付格式

每次任务完成后，Codex 最终回复应包含：

```text
已完成：
- <核心改动 1>
- <核心改动 2>

验证：
- <运行过的命令或检查>
- <结果>

交付文件：
- <文件路径 1>
- <文件路径 2>

剩余风险：
- <若无，写“未发现必须阻塞下一步的问题”>

下一步：
- <建议进入的下一个任务或阶段>
```

中文说明：最终回复要短而具体，不要粘贴大段日志。命令失败时必须说明失败原因和是否已经修复。

## 6. 通用验证命令

环境要求：

- 正式开发工具链以 Rust 为准：`cargo`、`rustfmt`、`clippy`、`cargo metadata`。
- Node.js 不是本项目的正式开发依赖。旧草案中出现的 Node one-liner 只能作为 workspace 创建前的临时人工检查，阶段 0 后应替换为 Rust `xtask`。
- 默认质量门不能依赖真实网络、真实凭证、真实交易权限或本机私有状态。
- 需要联网的只读适配器检查必须是显式 opt-in，不能阻塞默认离线质量门。

质量门定义：质量门是进入下一个任务包或阶段前必须通过的停止线。中文说明：单人维护也需要质量门，因为 Codex、未来上下文和 CI 都必须有同一个“能不能继续”的判定入口。

Rust workspace 创建后，所有阶段默认运行：

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

阶段 0 完成后还必须提供并运行 Rust-only 检查入口：

```bash
cargo xtask check-schema
cargo xtask check-crate-boundaries
cargo xtask check-docs
```

如果已实现聚合入口，优先运行：

```bash
cargo xtask quality-gate
```

遗留占位检查：

```bash
rg -n "TO""DO|TB""D|待""定|未""定义|arb-""adapters" universal_arb_platform_v2_immutable_core_docs
```

中文说明：如果 workspace 或 `xtask` 尚未创建，Rust 命令可以暂时跳过，但必须说明“尚未进入 Rust workspace/xtask 阶段”。阶段 0 之后不能继续依赖临时 Node 命令。

## 7. 任务拆分原则

Codex 每次领取任务时，优先按模块边界拆分：

| 任务类型 | 推荐修改范围 | 不应同时修改 |
|---|---|---|
| 领域类型 | `crates/arb-domain` | 执行、适配器、运行时 |
| 合同类型 | `crates/arb-contracts`, `fixtures/schema` | 策略逻辑、实盘适配 |
| 配置边界 | `crates/arb-config`, `templates/config.template.yaml` | 风控批准逻辑、签名实现 |
| 事件存储 | `crates/arb-eventstore`, `fixtures/replay` | 策略、执行、账本规则 |
| 账本 | `crates/arb-ledger` | 策略和可变适配器 |
| 策略接口 | `crates/arb-strategy-api` | 执行、签名、账本写入 |
| 样例策略 | `crates/arb-strategies` | 风控和执行内部规则 |
| 风控 | `crates/arb-risk` | 执行调度、签名、账本写入 |
| 执行模拟 | `crates/arb-execution` | 真实下单、真实签名 |
| 对账和运营 | `crates/arb-reconciliation`, `crates/arb-ops` | 可变执行和签名实现 |
| 只读适配器 | `crates/arb-venue-data` | 下单、撤单、转账 |
| 可变适配边界 | `crates/arb-venue-exec`, `crates/arb-signing` | 策略和风控批准逻辑 |
| 运行时装配 | `crates/arb-runtime` | 领域规则、风控规则、账本规则 |

中文说明：一次任务跨太多模块时，Codex 应先建议拆分。只有接口联调或端到端演练才允许集中修改多个模块。

第一个只读场所适配器默认策略：

- 选择标准：优先选择公开数据稳定、字段文档清楚、限频规则明确、无需交易权限、响应样例容易保存的场所或数据源。
- 凭证策略：默认使用公开数据；如必须使用账户数据，只允许只读凭证，禁止下单、撤单、转账、提现、签名权限。
- 联网策略：默认质量门必须离线通过；真实网络检查只能作为显式 opt-in 的集成检查，不能成为默认验收的唯一证据。
- fixture 策略：必须保存脱敏后的原始响应样例、标准化事件期望输出、断线/乱序/重复/缺字段/stale 数据样例。
- 离线测试要求：没有网络、没有 API key、没有本机私有配置时，schema、标准化、回放、风控新鲜度和边界检查仍必须通过。

中文说明：阶段 8 只证明“外部只读数据能稳定进入回放和风控”，不能引入任何账户变更能力。

## 8. 阶段任务包

### 8.1 阶段 0：工程骨架和边界

任务包 `S0-01`：创建 Rust workspace。

- 读取文档：`22`, `23`, `25`。
- 修改范围：根 `Cargo.toml`、`crates/*/Cargo.toml`、最小 `src/lib.rs` 或 `src/main.rs`。
- 完成内容：创建所有目标 crate，设置 workspace resolver，默认不开启实盘功能。
- 测试：`cargo test --workspace`。
- 验收：所有 crate 可编译；没有业务逻辑；没有真实执行路径。

任务包 `S0-02`：创建基础脚本和 fixture 目录。

- 修改范围：`xtask/`、必要的根 `Cargo.toml`、必要的 `.cargo/config.toml` Cargo alias、`fixtures/schema/valid`、`fixtures/schema/invalid`、`fixtures/replay`。
- 完成内容：Rust `xtask` 检查入口、schema 解析检查、依赖边界检查空壳、文档检查空壳、fixture 目录说明。
- 测试：`cargo xtask check-schema`。
- 验收：脚本能在空实现下运行并给出清晰结果。

任务包 `S0-03`：实现依赖边界检查。

- 修改范围：`xtask/`、必要的 `Cargo.toml`。
- 完成内容：解析 `cargo metadata`，拒绝禁止依赖表中的依赖。
- 测试：人为加入一个临时非法依赖确认失败，再移除。
- 验收：策略、风控、账本、只读适配器的禁止依赖均被检查。

### 8.2 阶段 1：领域类型、合同类型和配置边界

任务包 `S1-01`：实现 `arb-domain` 基础类型。

- 修改范围：`crates/arb-domain`。
- 完成内容：ID 包装、十进制封装、UTC 时间边界、核心状态枚举、错误类型。
- 测试：类型误用编译失败样例或单元测试；十进制字符串往返；禁止核心路径使用 `f64`。
- 验收：领域类型不依赖任何内部模块。

任务包 `S1-02`：实现 `arb-contracts` 合同类型。

- 修改范围：`crates/arb-contracts`、`fixtures/schema/valid`、`fixtures/schema/invalid`。
- 完成内容：所有 schema 对应结构体、严格反序列化、规范序列化、合同校验。
- 测试：每个 schema 正例通过，反例失败，未知字段失败，金额精度不丢失。
- 验收：核心合同都包含 `schema_version`。

任务包 `S1-03`：实现 `arb-config` 配置读取。

- 修改范围：`crates/arb-config`、`templates/config.template.yaml`。
- 完成内容：配置版本、配置哈希、执行模式、熔断开关、签名策略引用。
- 测试：默认实盘关闭；非法配置拒绝；熔断开关生效。
- 验收：配置读取不触发风控、执行或签名。

### 8.3 阶段 2：事件存储、规范哈希和回放基础

任务包 `S2-01`：实现追加式事件存储。

- 修改范围：`crates/arb-eventstore`。
- 完成内容：JSONL 追加、全局序号、事件哈希、按序读取、关联 ID 查询。
- 测试：只能追加；序号单调递增；哈希稳定；重复读取一致。
- 验收：不能修改历史事件。

任务包 `S2-02`：实现回放输入加载。

- 修改范围：`crates/arb-replay`、`fixtures/replay/minimal_smoke`。
- 完成内容：加载事件、配置、固定时间源、固定随机种子。
- 测试：同一 fixture 多次运行结果一致。
- 验收：回放期间不访问外部 API。

### 8.4 阶段 3：账本核心

任务包 `S3-01`：实现复式账本入账。

- 修改范围：`crates/arb-ledger`。
- 完成内容：账本科目、分录、借贷平衡、余额视图。
- 测试：借贷必平；重复事件幂等；金额精度不丢失。
- 验收：账本模块不依赖执行、风控、签名或运行时。

任务包 `S3-02`：实现冲销和调整。

- 修改范围：`crates/arb-ledger`。
- 完成内容：冲销分录、调整分录、原分录关联。
- 测试：冲销不删除原分录；调整有原因码和关联事件。
- 验收：历史账本不可原地改写。

### 8.5 阶段 4：策略只读接口和样例策略

任务包 `S4-01`：实现策略只读接口。

- 修改范围：`crates/arb-strategy-api`。
- 完成内容：只读快照、能力读取、固定时间源、策略 trait。
- 测试：接口无法访问执行、签名、账本写入。
- 验收：策略输出只能是候选组合转换或拒绝原因。

任务包 `S4-02`：实现第一个样例策略。

- 修改范围：`crates/arb-strategies`。
- 完成内容：固定输入下输出稳定候选转换；能力不足时拒绝。
- 测试：黄金输入输出测试；非法依赖检查。
- 验收：样例策略不依赖 `arb-execution`、`arb-signing`、`arb-ledger`。

### 8.6 阶段 5：风控引擎

任务包 `S5-01`：实现风控评估入口。

- 修改范围：`crates/arb-risk`。
- 完成内容：读取候选转换、组合状态、配置和能力，输出 `RiskDecision`。
- 测试：批准、拒绝、人工审批三类结果。
- 验收：风控模块不依赖执行和签名。

任务包 `S5-02`：实现核心风控检查。

- 修改范围：`crates/arb-risk`。
- 完成内容：新鲜度、流动性、手续费、滑点、保证金、资本预留、日亏损、场所健康、未知状态检查。
- 测试：每个检查有通过和失败样例。
- 验收：拒绝原因可被运营报告读取。

### 8.7 阶段 6：执行计划、资本预留和模拟执行

任务包 `S6-01`：实现执行计划构建。

- 修改范围：`crates/arb-execution`。
- 完成内容：从通过风控的决策生成 `ExecutionPlan`。
- 测试：拒绝决策不能生成计划；缺少必要字段失败。
- 验收：执行计划不绕过风控。

任务包 `S6-02`：实现资本预留状态机。

- 修改范围：`crates/arb-execution`。
- 完成内容：Requested、Reserved、ConvertedToExecution、Released、Expired、ReconciledMismatch 状态流。
- 测试：非法状态迁移失败；释放和过期幂等。
- 验收：预留资本有事件引用和账本引用。

任务包 `S6-03`：实现模拟执行。

- 修改范围：`crates/arb-execution`、`fixtures/replay`。
- 完成内容：模拟部分成功、失败、未知状态、补偿路径。
- 测试：状态机非法迁移；模拟报告 JSON 往返。
- 验收：阶段 6 完成前不出现真实下单实现。

### 8.8 阶段 7：对账、事故和运营报告

任务包 `S7-01`：实现对账核心。

- 修改范围：`crates/arb-reconciliation`。
- 完成内容：余额、仓位、成交、账本、资本预留对账。
- 测试：差异能发现；容忍范围不误报；严重差异生成事故。
- 验收：对账不改写账本历史。

任务包 `S7-02`：实现运营只读工具。

- 修改范围：`crates/arb-ops`、`templates/daily_operations_report.template.md`。
- 完成内容：日报、拒绝报告、事故报告、只读查询。
- 测试：报告字段完整；敏感信息脱敏；只读命令不触发可变动作。
- 验收：运营模块不依赖真实签名或实盘执行。

### 8.9 阶段 8：只读场所数据适配器

任务包 `S8-01`：实现只读适配器 trait。

- 修改范围：`crates/arb-venue-data`。
- 完成内容：行情、余额、仓位、工具信息、健康状态只读接口。
- 测试：只读模块不能依赖签名或可变执行。
- 验收：接口不能表达下单、撤单或转账。

任务包 `S8-02`：实现第一个只读场所适配器。

- 修改范围：`crates/arb-venue-data`、`fixtures/replay`。
- 完成内容：原始事件到 `NormalizedEvent` 的映射、错误分类、新鲜度判断。
- 测试：断线、乱序、重复消息、缺字段。
- 验收：能输出可回放标准化事件。

### 8.10 阶段 9：端到端模拟演练

任务包 `S9-01`：装配端到端模拟链路。

- 修改范围：`crates/arb-runtime`、必要的 fixture。
- 完成内容：只读数据、事件存储、状态构建、策略、风控、模拟执行、账本、对账、报告。
- 测试：固定 fixture 端到端运行；黄金结果比较。
- 验收：同一输入多次运行结果一致。

任务包 `S9-02`：完善运行时退出和健康检查。

- 修改范围：`crates/arb-runtime`。
- 完成内容：启动检查、熔断检查、任务生命周期、优雅退出、健康状态。
- 测试：配置错误拒绝启动；熔断打开时不启动可变执行。
- 验收：运行时不承载业务规则。

### 8.11 阶段 10：人工审批模式

任务包 `S10-01`：实现人工审批材料。

- 修改范围：`crates/arb-ops`, `crates/arb-execution`。
- 完成内容：审批摘要、风险检查、执行计划摘要、拒绝和批准记录。
- 测试：批准、拒绝、过期、重复审批。
- 验收：人工审批记录可回放、可审计。

### 8.12 阶段 11：可变执行适配器和签名边界预演

任务包 `S11-01`：实现可变执行 trait 和模拟实现。

- 修改范围：`crates/arb-venue-exec`。
- 完成内容：提交订单、撤单、查询状态、转账请求的边界 trait 和模拟实现。
- 测试：默认功能关闭时不编译真实实现；幂等键重复提交不重复执行。
- 验收：不接真实交易场所，不使用真实资金。

任务包 `S11-02`：实现签名边界和空签名器。

- 修改范围：`crates/arb-signing`。
- 完成内容：签名请求、签名策略、空签名器、审计引用、脱敏日志。
- 测试：真实签名默认不可用；签名失败不会被当作成功。
- 验收：策略和运营模块不能依赖签名实现。

### 8.13 阶段 12：受控实盘准备评审

任务包 `S12-01`：生成实盘准备评审材料。

- 修改范围：文档、运营报告、审查清单，不默认修改实盘代码。
- 完成内容：安全、风控、账本、执行、对账、回放、权限、密钥、事故流程审查材料。
- 测试：全量质量门、端到端回放、依赖边界、功能开关、脱敏检查。
- 验收：只有评审通过后，才允许创建真实执行实现任务。

## 9. 可直接复制的阶段提示词

以下提示词可以直接发给 Codex。默认含义是“执行整个阶段”。日常开发更推荐使用第 10 节的任务包级提示词；只有阶段范围很小或需要阶段收尾联调时，才建议使用本节提示词。

中文说明：阶段级提示词已经包含任务范围、禁止事项、测试要求、验收要求和最终回复格式。但阶段越往后越大，直接执行整阶段容易扩大改动面。因此正式开发时优先按任务包推进。

### 9.1 阶段 0 完整提示词

```text
请按 `universal_arb_platform_v2_immutable_core_docs/docs/24_Codex_Development_Runbook.md` 执行。

当前阶段：阶段 0：工程骨架和边界
当前任务包范围：S0-01、S0-02、S0-03

开始前必须读取：
- `docs/24_Codex_Development_Runbook.md`
- `docs/22_Development_Execution_Plan.md`
- `docs/23_Module_Architecture_Map.md`
- `docs/25_Core_Architecture_Reference.md`

允许修改范围：
- 根 `Cargo.toml`
- 必要的 `.cargo/config.toml`
- `crates/**/Cargo.toml`
- `crates/**/src/lib.rs`
- `crates/arb-runtime/src/main.rs`
- `xtask/**`
- `fixtures/schema/valid/**`
- `fixtures/schema/invalid/**`
- `fixtures/replay/**`

禁止修改范围：
- 不改 schema 合同字段
- 不实现业务逻辑
- 不实现真实下单、撤单、转账或真实签名
- 不连接真实交易 API

必须完成：
- 创建 Rust workspace
- 创建全部目标 crate 空壳
- 创建 fixture 目录
- 创建 Rust `xtask` 检查入口
- 创建 schema 检查、crate 依赖边界检查和文档检查命令
- 默认构建不包含实盘执行和真实签名路径

必须测试：
- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo test --workspace`
- `cargo xtask check-schema`
- `cargo xtask check-crate-boundaries`
- `cargo xtask check-docs`
- 人为加入一个临时非法依赖确认边界脚本失败，再移除该非法依赖

验收要求：
- 所有 crate 可编译
- 空 workspace 测试通过
- `xtask` schema 和文档检查可运行
- `xtask` 依赖边界检查能发现非法依赖
- 没有真实资金动作能力

完成后按“已完成 / 验证 / 交付文件 / 剩余风险 / 下一步”格式回复。
```

### 9.2 阶段 1 完整提示词

```text
请按 `universal_arb_platform_v2_immutable_core_docs/docs/24_Codex_Development_Runbook.md` 执行。

当前阶段：阶段 1：领域类型、合同类型和配置边界
当前任务包范围：S1-01、S1-02、S1-03

开始前必须读取：
- `docs/24_Codex_Development_Runbook.md`
- `docs/22_Development_Execution_Plan.md`
- `docs/23_Module_Architecture_Map.md`
- `docs/25_Core_Architecture_Reference.md`
- `docs/14_Data_Schemas_and_Contracts.md`

允许修改范围：
- `crates/arb-domain/**`
- `crates/arb-contracts/**`
- `crates/arb-config/**`
- `fixtures/schema/valid/**`
- `fixtures/schema/invalid/**`
- `templates/config.template.yaml`

禁止修改范围：
- 不修改执行、签名、可变适配器和运行时业务逻辑
- 不引入真实交易 API
- 不放宽 schema 严格性
- 不在核心金额路径使用 `f64`

必须完成：
- `arb-domain` 的 ID newtype、decimal 封装、UTC 时间边界、核心 enum 和错误类型
- `arb-contracts` 的 schema 对应类型、严格反序列化、规范序列化和合同校验
- `arb-config` 的配置版本、配置哈希、执行模式、熔断开关和签名策略引用
- 每个核心 schema 至少一个正例和一个反例 fixture

必须测试：
- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo test --workspace`
- `cargo xtask check-schema`
- `cargo xtask check-crate-boundaries`
- ID 类型不能互相误用
- decimal 字符串往返不丢精度
- 未知字段、错误枚举、错误 decimal 必须失败
- 默认配置不能开启实盘执行或真实签名

验收要求：
- `arb-domain` 无内部依赖
- 核心合同都有 `schema_version`
- 合同类型 JSON 往返不丢字段、不丢精度
- 配置读取不触发风控、执行或签名
- 文档和注释不能只有英文，关键说明必须有中文

完成后按“已完成 / 验证 / 交付文件 / 剩余风险 / 下一步”格式回复。
```

### 9.3 阶段 2 完整提示词

```text
请按 `universal_arb_platform_v2_immutable_core_docs/docs/24_Codex_Development_Runbook.md` 执行。

当前阶段：阶段 2：事件存储、规范哈希和回放基础
当前任务包范围：S2-01、S2-02

开始前必须读取：
- `docs/24_Codex_Development_Runbook.md`
- `docs/22_Development_Execution_Plan.md`
- `docs/23_Module_Architecture_Map.md`
- `docs/25_Core_Architecture_Reference.md`
- `docs/14_Data_Schemas_and_Contracts.md`
- `docs/21_State_Machines_and_Replay_Fixtures.md`

允许修改范围：
- `crates/arb-eventstore/**`
- `crates/arb-replay/**`
- `fixtures/replay/minimal_smoke/**`
- 必要的测试 fixture

禁止修改范围：
- 不访问外部 API
- 不引入策略逻辑
- 不引入执行逻辑
- 不修改账本规则
- 不实现真实签名或真实资金动作

必须完成：
- JSONL 追加式事件写入
- 全局递增序号
- 规范事件哈希
- 按序号读取事件
- 按关联 ID 查询事件链
- 回放输入加载、固定时间源和固定随机种子

必须测试：
- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo test --workspace`
- `cargo xtask check-schema`
- `cargo xtask check-crate-boundaries`
- 事件只能追加，不能覆盖
- 序号单调递增
- 同一事件哈希稳定
- 同一 fixture 多次回放结果一致

验收要求：
- 事件历史不可改写
- 回放不依赖系统当前时间
- 回放期间不访问外部 API
- 事件读取顺序确定

完成后按“已完成 / 验证 / 交付文件 / 剩余风险 / 下一步”格式回复。
```

### 9.4 阶段 3 完整提示词

```text
请按 `universal_arb_platform_v2_immutable_core_docs/docs/24_Codex_Development_Runbook.md` 执行。

当前阶段：阶段 3：账本核心
当前任务包范围：S3-01、S3-02

开始前必须读取：
- `docs/24_Codex_Development_Runbook.md`
- `docs/22_Development_Execution_Plan.md`
- `docs/23_Module_Architecture_Map.md`
- `docs/25_Core_Architecture_Reference.md`
- `docs/14_Data_Schemas_and_Contracts.md`

允许修改范围：
- `crates/arb-ledger/**`
- 必要的账本测试 fixture

禁止修改范围：
- 不依赖策略模块
- 不依赖风控模块
- 不依赖执行模块
- 不依赖签名或可变适配器
- 不原地修改历史账本分录

必须完成：
- 账本科目、账本分录、借贷平衡规则
- 余额视图
- 冲销分录
- 调整分录
- 原分录关联和原因码

必须测试：
- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo test --workspace`
- `cargo xtask check-crate-boundaries`
- 借贷必平
- 重复事件幂等
- 冲销不删除原分录
- 调整有原因码和关联事件
- 金额精度不丢失

验收要求：
- 账本只能追加
- 历史分录不可原地改写
- 账本模块不依赖风控、执行、签名或运行时
- 异常状态能被表达为后续事故或对账输入

完成后按“已完成 / 验证 / 交付文件 / 剩余风险 / 下一步”格式回复。
```

### 9.5 阶段 4 完整提示词

```text
请按 `universal_arb_platform_v2_immutable_core_docs/docs/24_Codex_Development_Runbook.md` 执行。

当前阶段：阶段 4：策略只读接口和样例策略
当前任务包范围：S4-01、S4-02

开始前必须读取：
- `docs/24_Codex_Development_Runbook.md`
- `docs/22_Development_Execution_Plan.md`
- `docs/23_Module_Architecture_Map.md`
- `docs/25_Core_Architecture_Reference.md`
- `docs/14_Data_Schemas_and_Contracts.md`

允许修改范围：
- `crates/arb-strategy-api/**`
- `crates/arb-strategies/**`
- 必要的策略测试 fixture

禁止修改范围：
- 策略不能依赖执行、签名、可变适配器、账本写入或运行时
- 不实现下单、撤单、转账、保证金修改
- 不在策略中写权威 PnL

必须完成：
- 只读快照接口
- 能力读取接口
- 固定时间源
- 策略 trait
- 第一个样例策略
- 能力不足时的拒绝路径

必须测试：
- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo test --workspace`
- `cargo xtask check-crate-boundaries`
- 策略接口无法访问执行、签名和账本写入
- 固定输入下样例策略输出稳定
- 能力不足时拒绝

验收要求：
- 策略输出只能是候选组合转换或拒绝原因
- 样例策略不依赖 `arb-execution`、`arb-signing`、`arb-ledger`、`arb-runtime`
- 策略时间源可固定，便于回放

完成后按“已完成 / 验证 / 交付文件 / 剩余风险 / 下一步”格式回复。
```

### 9.6 阶段 5 完整提示词

```text
请按 `universal_arb_platform_v2_immutable_core_docs/docs/24_Codex_Development_Runbook.md` 执行。

当前阶段：阶段 5：风控引擎
当前任务包范围：S5-01、S5-02

开始前必须读取：
- `docs/24_Codex_Development_Runbook.md`
- `docs/22_Development_Execution_Plan.md`
- `docs/23_Module_Architecture_Map.md`
- `docs/25_Core_Architecture_Reference.md`
- `docs/14_Data_Schemas_and_Contracts.md`

允许修改范围：
- `crates/arb-risk/**`
- 必要的风控测试 fixture

禁止修改范围：
- 风控不能调度执行
- 风控不能签名
- 风控不能写账本
- 风控不能依赖 `arb-execution`、`arb-venue-exec`、`arb-signing`、`arb-runtime`

必须完成：
- 风控评估入口
- `RiskDecision` 输出
- 批准、拒绝、需人工审批三类路径
- 数据新鲜度、流动性、手续费、滑点、保证金、资本预留、日亏损、场所健康、未知状态检查
- 原因码和检查明细

必须测试：
- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo test --workspace`
- `cargo xtask check-crate-boundaries`
- 每个核心检查至少有通过和失败样例
- 未知状态按风险处理
- 拒绝原因可序列化并可被运营报告读取

验收要求：
- 风控只输出 `RiskDecision`
- 风控模块不依赖执行和签名
- 每个决策包含策略版本、输入引用、检查结果和原因码

完成后按“已完成 / 验证 / 交付文件 / 剩余风险 / 下一步”格式回复。
```

### 9.7 阶段 6 完整提示词

```text
请按 `universal_arb_platform_v2_immutable_core_docs/docs/24_Codex_Development_Runbook.md` 执行。

当前阶段：阶段 6：执行计划、资本预留和模拟执行
当前任务包范围：S6-01、S6-02、S6-03

开始前必须读取：
- `docs/24_Codex_Development_Runbook.md`
- `docs/22_Development_Execution_Plan.md`
- `docs/23_Module_Architecture_Map.md`
- `docs/25_Core_Architecture_Reference.md`
- `docs/14_Data_Schemas_and_Contracts.md`
- `docs/21_State_Machines_and_Replay_Fixtures.md`

允许修改范围：
- `crates/arb-execution/**`
- `fixtures/replay/**`
- 必要的执行测试 fixture

禁止修改范围：
- 不实现真实下单、撤单、转账或真实签名
- 不依赖真实交易 API
- 不绕过风控
- 不直接写实盘账本

必须完成：
- 从 `Approved` 的 `RiskDecision` 构建可调度的 `ExecutionPlan`
- 从 `NeedsManualApproval` 的 `RiskDecision` 构建审批材料或 `PendingManualApproval` 计划预览，审批前不能分发执行
- 资本预留状态机：Requested、Reserved、ConvertedToExecution、Released、Expired、ReconciledMismatch
- 模拟执行报告
- 部分成功、失败、未知状态和补偿路径

必须测试：
- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo test --workspace`
- `cargo xtask check-crate-boundaries`
- 拒绝决策不能生成执行计划
- 人工审批路径审批前不能分发执行
- 缺少必要字段失败
- 非法状态迁移失败
- 释放和过期幂等
- 模拟报告 JSON 往返

验收要求：
- 执行计划不绕过风控
- 资本预留有事件引用和账本引用
- 阶段 6 完成前不出现真实下单实现
- 执行模块不直接持有密钥

完成后按“已完成 / 验证 / 交付文件 / 剩余风险 / 下一步”格式回复。
```

### 9.8 阶段 7 完整提示词

```text
请按 `universal_arb_platform_v2_immutable_core_docs/docs/24_Codex_Development_Runbook.md` 执行。

当前阶段：阶段 7：对账、事故和运营报告
当前任务包范围：S7-01、S7-02

开始前必须读取：
- `docs/24_Codex_Development_Runbook.md`
- `docs/22_Development_Execution_Plan.md`
- `docs/23_Module_Architecture_Map.md`
- `docs/25_Core_Architecture_Reference.md`
- `docs/14_Data_Schemas_and_Contracts.md`

允许修改范围：
- `crates/arb-reconciliation/**`
- `crates/arb-ops/**`
- `templates/daily_operations_report.template.md`
- 必要的对账和运营测试 fixture

禁止修改范围：
- 对账不能改写账本历史
- 运营工具不能下单、签名或直接改变账户
- 不依赖真实签名或实盘执行实现

必须完成：
- 余额、仓位、成交、账本、资本预留对账
- 差异分类和事故建议
- 日报、拒绝报告、事故报告、只读查询
- 敏感信息脱敏

必须测试：
- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo test --workspace`
- `cargo xtask check-crate-boundaries`
- 差异能发现
- 容忍范围不误报
- 严重差异生成事故
- 只读命令不触发可变动作
- 报告字段完整并脱敏

验收要求：
- 对账是独立核心模块，不是报告工具附属逻辑
- 对账不改写账本历史
- 运营模块默认只读
- 运营模块不依赖真实签名或实盘执行

完成后按“已完成 / 验证 / 交付文件 / 剩余风险 / 下一步”格式回复。
```

### 9.9 阶段 8 完整提示词

```text
请按 `universal_arb_platform_v2_immutable_core_docs/docs/24_Codex_Development_Runbook.md` 执行。

当前阶段：阶段 8：只读场所数据适配器
当前任务包范围：S8-01、S8-02

开始前必须读取：
- `docs/24_Codex_Development_Runbook.md`
- `docs/22_Development_Execution_Plan.md`
- `docs/23_Module_Architecture_Map.md`
- `docs/25_Core_Architecture_Reference.md`
- `docs/14_Data_Schemas_and_Contracts.md`

允许修改范围：
- `crates/arb-venue-data/**`
- `fixtures/replay/**`
- 必要的只读适配器测试 fixture

禁止修改范围：
- 不下单
- 不撤单
- 不转账
- 不签名
- 不依赖 `arb-venue-exec` 或 `arb-signing`

必须完成：
- 行情、余额、仓位、工具信息、健康状态只读 trait
- 第一个只读场所适配器
- 原始事件到 `NormalizedEvent` 的映射
- 外部错误分类和新鲜度判断
- 脱敏 fixture 和默认离线测试路径

必须测试：
- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo test --workspace`
- `cargo xtask check-crate-boundaries`
- 只读模块不能依赖签名或可变执行
- 断线、乱序、重复消息、缺字段场景
- 输出事件可回放
- 无网络、无 API key 时默认质量门通过

验收要求：
- 接口不能表达下单、撤单或转账
- 只读适配器能输出标准化事件
- `arb-venue-data` 不依赖 `arb-signing` 和 `arb-venue-exec`
- 任何真实网络检查都必须显式 opt-in，且不能替代离线 fixture 验收

完成后按“已完成 / 验证 / 交付文件 / 剩余风险 / 下一步”格式回复。
```

### 9.10 阶段 9 完整提示词

```text
请按 `universal_arb_platform_v2_immutable_core_docs/docs/24_Codex_Development_Runbook.md` 执行。

当前阶段：阶段 9：端到端模拟演练
当前任务包范围：S9-01、S9-02

开始前必须读取：
- `docs/24_Codex_Development_Runbook.md`
- `docs/22_Development_Execution_Plan.md`
- `docs/23_Module_Architecture_Map.md`
- `docs/25_Core_Architecture_Reference.md`
- `docs/14_Data_Schemas_and_Contracts.md`
- `docs/21_State_Machines_and_Replay_Fixtures.md`

允许修改范围：
- `crates/arb-runtime/**`
- 必要的端到端 fixture
- 必要的装配测试

禁止修改范围：
- 运行时不能写策略规则
- 运行时不能写风控规则
- 运行时不能写账本规则
- 运行时不能写执行状态机规则
- 不连接真实交易 API

必须完成：
- 装配只读数据、事件存储、状态构建、策略、风控、模拟执行、账本、对账和报告
- 固定 fixture 端到端模拟链路
- 黄金结果比较
- 启动检查、熔断检查、任务生命周期、优雅退出和健康状态

必须测试：
- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo test --workspace`
- `cargo xtask check-schema`
- `cargo xtask check-crate-boundaries`
- 固定 fixture 端到端运行
- 同一输入多次运行结果一致
- 配置错误拒绝启动
- 熔断打开时不启动可变执行

验收要求：
- 端到端模拟链路可回放
- 运行时只做装配，不承载业务规则
- 无真实资金动作
- 报告和对账结果可追溯

完成后按“已完成 / 验证 / 交付文件 / 剩余风险 / 下一步”格式回复。
```

### 9.11 阶段 10 完整提示词

```text
请按 `universal_arb_platform_v2_immutable_core_docs/docs/24_Codex_Development_Runbook.md` 执行。

当前阶段：阶段 10：人工审批模式
当前任务包范围：S10-01

开始前必须读取：
- `docs/24_Codex_Development_Runbook.md`
- `docs/22_Development_Execution_Plan.md`
- `docs/23_Module_Architecture_Map.md`
- `docs/25_Core_Architecture_Reference.md`
- `docs/14_Data_Schemas_and_Contracts.md`

允许修改范围：
- `crates/arb-ops/**`
- `crates/arb-execution/**`
- 必要的人工审批测试 fixture

禁止修改范围：
- 审批不能绕过风控
- 审批不能绕过账本
- 不提交真实资金动作
- 不接真实签名实现

必须完成：
- 审批摘要
- 风险检查摘要
- 执行计划摘要
- 拒绝和批准记录
- 审批过期和重复审批处理

必须测试：
- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo test --workspace`
- `cargo xtask check-crate-boundaries`
- 批准、拒绝、过期、重复审批场景
- 审批记录可回放
- 审批材料脱敏

验收要求：
- 人工审批记录可审计
- 审批通过也只能进入受控流程
- 审批不能绕过风控和账本

完成后按“已完成 / 验证 / 交付文件 / 剩余风险 / 下一步”格式回复。
```

### 9.12 阶段 11 完整提示词

```text
请按 `universal_arb_platform_v2_immutable_core_docs/docs/24_Codex_Development_Runbook.md` 执行。

当前阶段：阶段 11：可变执行适配器和签名边界预演
当前任务包范围：S11-01、S11-02

开始前必须读取：
- `docs/24_Codex_Development_Runbook.md`
- `docs/22_Development_Execution_Plan.md`
- `docs/23_Module_Architecture_Map.md`
- `docs/25_Core_Architecture_Reference.md`
- `docs/14_Data_Schemas_and_Contracts.md`

允许修改范围：
- `crates/arb-venue-exec/**`
- `crates/arb-signing/**`
- 必要的可变执行和签名边界测试 fixture

禁止修改范围：
- 不连接真实交易场所
- 不使用真实密钥
- 不提交真实资金动作
- 不让策略或运营模块依赖真实签名实现
- 不实现默认开启的实盘执行路径

必须完成：
- 可变执行 trait
- 可变执行模拟实现
- 幂等键处理
- 签名请求、签名策略、空签名器、审计引用
- 日志和报告脱敏

必须测试：
- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo test --workspace`
- `cargo xtask check-crate-boundaries`
- 默认功能关闭时不编译真实实现
- 幂等键重复提交不重复执行
- 真实签名默认不可用
- 签名失败不会被当作成功
- 策略和运营模块不能依赖签名实现

验收要求：
- 默认编译安全
- 真实签名实现不存在或不可用
- 可变执行路径需要显式功能开关
- 不接真实交易场所，不使用真实资金

完成后按“已完成 / 验证 / 交付文件 / 剩余风险 / 下一步”格式回复。
```

### 9.13 阶段 12 完整提示词

```text
请按 `universal_arb_platform_v2_immutable_core_docs/docs/24_Codex_Development_Runbook.md` 执行。

当前阶段：阶段 12：受控实盘准备评审
当前任务包范围：S12-01

开始前必须读取：
- `docs/24_Codex_Development_Runbook.md`
- `docs/22_Development_Execution_Plan.md`
- `docs/23_Module_Architecture_Map.md`
- `docs/25_Core_Architecture_Reference.md`
- `docs/14_Data_Schemas_and_Contracts.md`
- `docs/21_State_Machines_and_Replay_Fixtures.md`

允许修改范围：
- 受控实盘准备评审材料
- 运营报告模板或审查清单
- 必要的测试 fixture
- 必要的文档更新

禁止修改范围：
- 不默认实现真实执行
- 不接真实交易场所
- 不使用真实密钥
- 不提交真实资金动作
- 不删除或弱化安全检查

必须完成：
- 安全、风控、账本、执行、对账、回放、权限、密钥、事故流程审查材料
- kill switch 覆盖说明
- 权限配置检查说明
- 对账演练和事故响应演练材料
- 是否允许创建真实执行实现任务的结论

必须测试：
- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo test --workspace`
- `cargo xtask check-schema`
- `cargo xtask check-crate-boundaries`
- 端到端回放
- 依赖边界
- 功能开关
- 脱敏检查
- kill switch 阻断检查

验收要求：
- 外部审查完成前不能进入实盘
- 所有实盘能力默认关闭
- 所有实盘动作可追溯、可停止、可对账
- 事故响应流程已演练
- 只有评审通过后，才允许创建真实执行实现任务

个人自用治理补充：
- 如果系统只由所有者本人使用自己的小额资金，可以新增或更新 `review/personal_guarded_live_governance.md` 作为个人小额受控试运行治理材料。
- 个人路径不能声称外部独立审查通过，不能用于他人资金或商业服务，且不允许自动实盘。
- 个人小额受控试运行仍必须默认关闭实盘能力、禁止提现权限、限制资金和单笔额度、要求每笔人工确认、保留 kill switch、未知状态停机、执行后强制对账和事故记录。

完成后按“已完成 / 验证 / 交付文件 / 剩余风险 / 下一步”格式回复。
```

## 10. 可直接复制的任务包提示词

以下提示词按任务包拆分，可以逐条发送给 Codex。推荐顺序是从 `S0-01` 开始，完成、测试、验收通过后再发送下一个任务包。

中文说明：任务包提示词比阶段提示词更适合实际开发。每个任务包都限制修改范围、禁止越界行为，并要求输出测试和验收结果。

### 10.1 `S0-01` 创建 Rust workspace

```text
请按 `universal_arb_platform_v2_immutable_core_docs/docs/24_Codex_Development_Runbook.md` 执行。

当前阶段：阶段 0：工程骨架和边界
当前任务包：S0-01 创建 Rust workspace

开始前必须读取：
- `docs/24_Codex_Development_Runbook.md`
- `docs/22_Development_Execution_Plan.md`
- `docs/23_Module_Architecture_Map.md`
- `docs/25_Core_Architecture_Reference.md`

允许修改范围：
- 根 `Cargo.toml`
- `crates/*/Cargo.toml`
- `crates/*/src/lib.rs`
- `crates/arb-runtime/src/main.rs`

禁止修改范围：
- 不改 schema
- 不写业务逻辑
- 不实现真实下单、撤单、转账、签名

必须完成：
- 创建 Rust workspace
- 创建全部目标 crate 空壳
- 设置 workspace resolver
- 默认不开启实盘执行和真实签名能力

必须测试：
- `cargo fmt --all -- --check`
- `cargo test --workspace`

验收要求：
- 所有 crate 可编译
- 没有业务逻辑
- 没有真实资金动作路径

完成后按“已完成 / 验证 / 交付文件 / 剩余风险 / 下一步”格式回复。
```

### 10.2 `S0-02` 创建基础脚本和 fixture 目录

```text
请按 `universal_arb_platform_v2_immutable_core_docs/docs/24_Codex_Development_Runbook.md` 执行。

当前阶段：阶段 0：工程骨架和边界
当前任务包：S0-02 创建基础脚本和 fixture 目录

开始前必须读取：`24`、`22`、`23`、`25`。

允许修改范围：
- `xtask/**`
- 必要的根 `Cargo.toml`
- 必要的 `.cargo/config.toml`
- `fixtures/schema/valid/**`
- `fixtures/schema/invalid/**`
- `fixtures/replay/**`

禁止修改范围：
- 不改 Rust 业务模块
- 不改 schema 合同字段
- 不实现真实执行或签名

必须完成：
- 创建 Rust `xtask` 检查入口
- 创建 schema 解析检查
- 创建依赖边界检查空壳
- 创建文档检查空壳
- 创建 schema 正反 fixture 目录
- 创建 replay fixture 目录

必须测试：
- `cargo xtask check-schema`
- `cargo xtask check-docs`
- 若 workspace 已创建，运行 `cargo test --workspace`

验收要求：
- `xtask` 命令能在当前空实现下运行并给出清晰结果
- fixture 目录结构清楚
- 文档说明包含中文

完成后按“已完成 / 验证 / 交付文件 / 剩余风险 / 下一步”格式回复。
```

### 10.3 `S0-03` 实现依赖边界检查

```text
请按 `universal_arb_platform_v2_immutable_core_docs/docs/24_Codex_Development_Runbook.md` 执行。

当前阶段：阶段 0：工程骨架和边界
当前任务包：S0-03 实现依赖边界检查

开始前必须读取：`24`、`22`、`23`、`25`。

允许修改范围：
- `xtask/**`
- 必要的 `Cargo.toml`

禁止修改范围：
- 不改业务逻辑
- 不放宽模块边界
- 不引入真实执行或签名能力

必须完成：
- 解析 `cargo metadata`
- 检查 `23_Module_Architecture_Map.md` 中的禁止依赖
- 至少覆盖策略、风控、账本、只读适配器

必须测试：
- `cargo xtask check-crate-boundaries`
- 人为加入一个临时非法依赖确认脚本失败，再移除该非法依赖
- `cargo test --workspace`

验收要求：
- 非法依赖会导致检查失败
- 合法空 workspace 检查通过
- 策略、风控、账本、只读适配器边界被覆盖

完成后按“已完成 / 验证 / 交付文件 / 剩余风险 / 下一步”格式回复。
```

### 10.4 `S1-01` 实现 `arb-domain` 基础类型

```text
请按 `universal_arb_platform_v2_immutable_core_docs/docs/24_Codex_Development_Runbook.md` 执行。

当前阶段：阶段 1：领域类型、合同类型和配置边界
当前任务包：S1-01 实现 `arb-domain` 基础类型

开始前必须读取：`24`、`22`、`23`、`25`、`14`。

允许修改范围：
- `crates/arb-domain/**`

禁止修改范围：
- 不修改执行、签名、适配器、运行时
- 不使用 `f64` 表达核心金额、价格、利率、收益

必须完成：
- ID newtype
- decimal 或定点封装
- UTC 时间边界
- 核心状态 enum
- 领域错误类型

必须测试：
- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo test --workspace`
- `cargo xtask check-crate-boundaries`
- ID 类型不能互相误用
- decimal 字符串往返不丢精度

验收要求：
- `arb-domain` 无内部依赖
- 核心金额路径不使用 `f64`
- 领域类型有中文说明或清晰中文文档注释

完成后按“已完成 / 验证 / 交付文件 / 剩余风险 / 下一步”格式回复。
```

### 10.5 `S1-02` 实现 `arb-contracts` 合同类型

```text
请按 `universal_arb_platform_v2_immutable_core_docs/docs/24_Codex_Development_Runbook.md` 执行。

当前阶段：阶段 1：领域类型、合同类型和配置边界
当前任务包：S1-02 实现 `arb-contracts` 合同类型

开始前必须读取：`24`、`22`、`23`、`25`、`14`。

允许修改范围：
- `crates/arb-contracts/**`
- `fixtures/schema/valid/**`
- `fixtures/schema/invalid/**`

禁止修改范围：
- 不放宽 schema 严格性
- 不把合同类型写进风控、执行或账本模块
- 不访问外部网络

必须完成：
- 所有 schema 对应 Rust 结构体
- 严格反序列化
- 规范序列化辅助
- 合同校验
- 每个核心 schema 的正例和反例 fixture

必须测试：
- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo test --workspace`
- `cargo xtask check-schema`
- 正例通过，反例失败
- 未知字段、错误 enum、错误 decimal 必须失败

验收要求：
- 核心合同都有 `schema_version`
- JSON 往返不丢字段、不丢精度
- `arb-contracts` 不承担风控、执行或账本职责

完成后按“已完成 / 验证 / 交付文件 / 剩余风险 / 下一步”格式回复。
```

### 10.6 `S1-03` 实现 `arb-config` 配置读取

```text
请按 `universal_arb_platform_v2_immutable_core_docs/docs/24_Codex_Development_Runbook.md` 执行。

当前阶段：阶段 1：领域类型、合同类型和配置边界
当前任务包：S1-03 实现 `arb-config` 配置读取

开始前必须读取：`24`、`22`、`23`、`25`。

允许修改范围：
- `crates/arb-config/**`
- `templates/config.template.yaml`

禁止修改范围：
- 配置读取不能触发风控、执行或签名
- 不保存明文密钥
- 不默认开启实盘能力

必须完成：
- 配置结构
- 配置版本
- 配置哈希
- 执行模式读取
- 熔断开关读取
- 签名策略引用

必须测试：
- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo test --workspace`
- 默认实盘关闭
- 非法配置拒绝
- 熔断开关生效
- 配置哈希稳定

验收要求：
- 配置对象只读且已校验
- 配置模块不持有密钥明文
- 配置读取不触发账户变化

完成后按“已完成 / 验证 / 交付文件 / 剩余风险 / 下一步”格式回复。
```

### 10.7 `S2-01` 实现追加式事件存储

```text
请按 `universal_arb_platform_v2_immutable_core_docs/docs/24_Codex_Development_Runbook.md` 执行。

当前阶段：阶段 2：事件存储、规范哈希和回放基础
当前任务包：S2-01 实现追加式事件存储

开始前必须读取：`24`、`22`、`23`、`25`、`14`、`21`。

允许修改范围：
- `crates/arb-eventstore/**`
- 必要的事件测试 fixture

禁止修改范围：
- 不解释业务含义
- 不产生风控结论
- 不写账本
- 不改写历史事件

必须完成：
- JSONL 事件追加
- 全局递增序号
- 规范事件哈希
- 按序号读取
- 按关联 ID 查询事件链

必须测试：
- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo test --workspace`
- 事件只能追加
- 序号单调递增
- 哈希稳定
- 重复读取一致

验收要求：
- 事件历史不可覆盖或改写
- 事件顺序确定
- `arb-eventstore` 不依赖上层业务模块

完成后按“已完成 / 验证 / 交付文件 / 剩余风险 / 下一步”格式回复。
```

### 10.8 `S2-02` 实现回放输入加载

```text
请按 `universal_arb_platform_v2_immutable_core_docs/docs/24_Codex_Development_Runbook.md` 执行。

当前阶段：阶段 2：事件存储、规范哈希和回放基础
当前任务包：S2-02 实现回放输入加载

开始前必须读取：`24`、`22`、`23`、`25`、`21`。

允许修改范围：
- `crates/arb-replay/**`
- `fixtures/replay/minimal_smoke/**`

禁止修改范围：
- 回放期间不访问外部 API
- 不触发真实签名
- 不写真实账户

必须完成：
- 事件 fixture 加载
- 配置 fixture 加载
- 固定时间源
- 固定随机种子
- 最小 smoke 回放 fixture

必须测试：
- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo test --workspace`
- 同一 fixture 多次运行结果一致
- 回放不依赖系统当前时间

验收要求：
- 回放输入完全来自 fixture
- 回放结果确定
- 无外部 API 访问

完成后按“已完成 / 验证 / 交付文件 / 剩余风险 / 下一步”格式回复。
```

### 10.9 `S3-01` 实现复式账本入账

```text
请按 `universal_arb_platform_v2_immutable_core_docs/docs/24_Codex_Development_Runbook.md` 执行。

当前阶段：阶段 3：账本核心
当前任务包：S3-01 实现复式账本入账

开始前必须读取：`24`、`22`、`23`、`25`、`14`。

允许修改范围：
- `crates/arb-ledger/**`
- 必要的账本测试 fixture

禁止修改范围：
- 不调用策略
- 不调用执行适配器
- 不读取密钥
- 不依赖风控、执行、签名或运行时

必须完成：
- 账本科目
- 账本分录
- 借贷平衡规则
- 余额视图

必须测试：
- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo test --workspace`
- `cargo xtask check-crate-boundaries`
- 借贷必平
- 重复事件幂等
- 金额精度不丢失

验收要求：
- 账本只能追加
- `arb-ledger` 不依赖执行、风控、签名或运行时
- 余额视图可由账本分录推导

完成后按“已完成 / 验证 / 交付文件 / 剩余风险 / 下一步”格式回复。
```

### 10.10 `S3-02` 实现冲销和调整

```text
请按 `universal_arb_platform_v2_immutable_core_docs/docs/24_Codex_Development_Runbook.md` 执行。

当前阶段：阶段 3：账本核心
当前任务包：S3-02 实现冲销和调整

开始前必须读取：`24`、`22`、`23`、`25`。

允许修改范围：
- `crates/arb-ledger/**`
- 必要的账本测试 fixture

禁止修改范围：
- 不删除原分录
- 不原地改写历史分录
- 不通过执行模块修正账本

必须完成：
- 冲销分录
- 调整分录
- 原分录关联
- 调整原因码

必须测试：
- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo test --workspace`
- 冲销不删除原分录
- 调整保留原因码和关联事件
- 冲销和调整后账本仍平衡

验收要求：
- 历史账本不可原地改写
- 修正只能追加表达
- 调整链路可审计

完成后按“已完成 / 验证 / 交付文件 / 剩余风险 / 下一步”格式回复。
```

### 10.11 `S4-01` 实现策略只读接口

```text
请按 `universal_arb_platform_v2_immutable_core_docs/docs/24_Codex_Development_Runbook.md` 执行。

当前阶段：阶段 4：策略只读接口和样例策略
当前任务包：S4-01 实现策略只读接口

开始前必须读取：`24`、`22`、`23`、`25`、`14`。

允许修改范围：
- `crates/arb-strategy-api/**`
- 必要的策略接口测试 fixture

禁止修改范围：
- 不暴露执行、签名、转账、账本写入
- 不依赖运行时
- 不实现具体交易动作

必须完成：
- 只读快照接口
- 能力读取接口
- 固定时间源
- 策略 trait
- 候选转换输出接口

必须测试：
- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo test --workspace`
- `cargo xtask check-crate-boundaries`
- 策略接口无法访问执行、签名、账本写入

验收要求：
- 策略 API 只能读状态并输出候选转换或拒绝原因
- 依赖边界检查通过
- 时间源可固定，便于回放

完成后按“已完成 / 验证 / 交付文件 / 剩余风险 / 下一步”格式回复。
```

### 10.12 `S4-02` 实现第一个样例策略

```text
请按 `universal_arb_platform_v2_immutable_core_docs/docs/24_Codex_Development_Runbook.md` 执行。

当前阶段：阶段 4：策略只读接口和样例策略
当前任务包：S4-02 实现第一个样例策略

开始前必须读取：`24`、`22`、`23`、`25`、`14`。

允许修改范围：
- `crates/arb-strategies/**`
- 必要的策略测试 fixture

禁止修改范围：
- 不依赖 `arb-execution`
- 不依赖 `arb-signing`
- 不依赖 `arb-ledger`
- 不依赖 `arb-runtime`
- 不下单、不签名、不转账

必须完成：
- 第一个样例策略
- 固定输入下输出稳定候选转换
- 能力不足时拒绝
- 策略版本和配置引用

必须测试：
- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo test --workspace`
- `cargo xtask check-crate-boundaries`
- 黄金输入输出测试
- 能力不足拒绝测试

验收要求：
- 样例策略只依赖 `arb-strategy-api`
- 输出可序列化为候选组合转换
- 策略不拥有执行权限

完成后按“已完成 / 验证 / 交付文件 / 剩余风险 / 下一步”格式回复。
```

### 10.13 `S5-01` 实现风控评估入口

```text
请按 `universal_arb_platform_v2_immutable_core_docs/docs/24_Codex_Development_Runbook.md` 执行。

当前阶段：阶段 5：风控引擎
当前任务包：S5-01 实现风控评估入口

开始前必须读取：`24`、`22`、`23`、`25`、`14`。

允许修改范围：
- `crates/arb-risk/**`
- 必要的风控测试 fixture

禁止修改范围：
- 不调度执行
- 不签名
- 不写账本
- 不依赖执行、可变适配器、签名或运行时

必须完成：
- 风控评估入口
- 输入候选转换、组合状态、配置和能力
- 输出 `RiskDecision`
- 批准、拒绝、人工审批三类结果

必须测试：
- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo test --workspace`
- `cargo xtask check-crate-boundaries`
- 批准、拒绝、人工审批路径

验收要求：
- 风控只输出 `RiskDecision`
- 风控模块不依赖执行和签名
- 决策包含原因码和检查明细

完成后按“已完成 / 验证 / 交付文件 / 剩余风险 / 下一步”格式回复。
```

### 10.14 `S5-02` 实现核心风控检查

```text
请按 `universal_arb_platform_v2_immutable_core_docs/docs/24_Codex_Development_Runbook.md` 执行。

当前阶段：阶段 5：风控引擎
当前任务包：S5-02 实现核心风控检查

开始前必须读取：`24`、`22`、`23`、`25`、`14`。

允许修改范围：
- `crates/arb-risk/**`
- 必要的风控测试 fixture

禁止修改范围：
- 不调用执行
- 不写账本
- 不把未知状态当作成功

必须完成：
- 新鲜度检查
- 流动性检查
- 手续费和滑点检查
- 保证金检查
- 资本预留检查
- 日亏损检查
- 场所健康检查
- 未知状态检查

必须测试：
- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo test --workspace`
- 每个检查至少有通过和失败样例
- 未知状态必须拒绝或要求人工审批

验收要求：
- 拒绝原因可被运营报告读取
- 风控检查结果可序列化
- 风控仍不依赖执行、签名和账本写入

完成后按“已完成 / 验证 / 交付文件 / 剩余风险 / 下一步”格式回复。
```

### 10.15 `S6-01` 实现执行计划构建

```text
请按 `universal_arb_platform_v2_immutable_core_docs/docs/24_Codex_Development_Runbook.md` 执行。

当前阶段：阶段 6：执行计划、资本预留和模拟执行
当前任务包：S6-01 实现执行计划构建

开始前必须读取：`24`、`22`、`23`、`25`、`14`、`21`。

允许修改范围：
- `crates/arb-execution/**`
- 必要的执行计划测试 fixture

禁止修改范围：
- 不绕过风控
- 不真实下单
- 不真实签名
- 不直接写实盘账本

必须完成：
- 从 `Approved` 的 `RiskDecision` 构建可调度的 `ExecutionPlan`
- 从 `NeedsManualApproval` 的 `RiskDecision` 构建审批材料或 `PendingManualApproval` 计划预览，审批前不能分发执行
- 执行腿、依赖、超时、取消、对冲、补偿字段

必须测试：
- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo test --workspace`
- 拒绝决策不能生成执行计划
- 人工审批路径审批前不能分发执行
- 缺少必要字段失败

验收要求：
- 执行计划来自风控决策
- 执行计划不绕过风控
- 不出现真实下单实现

完成后按“已完成 / 验证 / 交付文件 / 剩余风险 / 下一步”格式回复。
```

### 10.16 `S6-02` 实现资本预留状态机

```text
请按 `universal_arb_platform_v2_immutable_core_docs/docs/24_Codex_Development_Runbook.md` 执行。

当前阶段：阶段 6：执行计划、资本预留和模拟执行
当前任务包：S6-02 实现资本预留状态机

开始前必须读取：`24`、`22`、`23`、`25`、`21`。

允许修改范围：
- `crates/arb-execution/**`
- 必要的资本预留测试 fixture

禁止修改范围：
- 不直接改账本历史
- 不绕过风控
- 不接真实资金

必须完成：
- Requested
- Reserved
- ConvertedToExecution
- Released
- Expired
- ReconciledMismatch
- 事件引用和账本引用

必须测试：
- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo test --workspace`
- 非法状态迁移失败
- 释放和过期幂等

验收要求：
- 资本预留状态可回放
- 预留资本有事件引用和账本引用
- 不出现真实资金动作

完成后按“已完成 / 验证 / 交付文件 / 剩余风险 / 下一步”格式回复。
```

### 10.17 `S6-03` 实现模拟执行

```text
请按 `universal_arb_platform_v2_immutable_core_docs/docs/24_Codex_Development_Runbook.md` 执行。

当前阶段：阶段 6：执行计划、资本预留和模拟执行
当前任务包：S6-03 实现模拟执行

开始前必须读取：`24`、`22`、`23`、`25`、`14`、`21`。

允许修改范围：
- `crates/arb-execution/**`
- `fixtures/replay/**`
- 必要的模拟执行测试 fixture

禁止修改范围：
- 不实现真实下单
- 不实现真实撤单
- 不实现真实转账
- 不实现真实签名

必须完成：
- 模拟部分成功
- 模拟失败
- 模拟未知状态
- 模拟补偿路径
- `ExecutionReport` 输出

必须测试：
- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo test --workspace`
- 状态机非法迁移失败
- 模拟报告 JSON 往返
- 固定输入下模拟结果稳定

验收要求：
- 阶段 6 完成前不出现真实下单实现
- 执行模块不直接持有密钥
- 未知状态可表达并进入后续对账或事故流程

完成后按“已完成 / 验证 / 交付文件 / 剩余风险 / 下一步”格式回复。
```

### 10.18 `S7-01` 实现对账核心

```text
请按 `universal_arb_platform_v2_immutable_core_docs/docs/24_Codex_Development_Runbook.md` 执行。

当前阶段：阶段 7：对账、事故和运营报告
当前任务包：S7-01 实现对账核心

开始前必须读取：`24`、`22`、`23`、`25`、`14`。

允许修改范围：
- `crates/arb-reconciliation/**`
- 必要的对账测试 fixture

禁止修改范围：
- 不改写账本历史
- 不直接下单补偿
- 不调用签名
- 不绕过事故流程

必须完成：
- 余额对账
- 仓位对账
- 成交对账
- 账本对账
- 资本预留对账
- 差异分类和事故建议

必须测试：
- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo test --workspace`
- 差异能发现
- 容忍范围不误报
- 严重差异生成事故

验收要求：
- 对账不改写账本历史
- 对账是独立核心模块
- 对账结果可供运营报告读取

完成后按“已完成 / 验证 / 交付文件 / 剩余风险 / 下一步”格式回复。
```

### 10.19 `S7-02` 实现运营只读工具

```text
请按 `universal_arb_platform_v2_immutable_core_docs/docs/24_Codex_Development_Runbook.md` 执行。

当前阶段：阶段 7：对账、事故和运营报告
当前任务包：S7-02 实现运营只读工具

开始前必须读取：`24`、`22`、`23`、`25`。

允许修改范围：
- `crates/arb-ops/**`
- `templates/daily_operations_report.template.md`
- 必要的运营报告测试 fixture

禁止修改范围：
- 不下单
- 不签名
- 不直接改变账户
- 不依赖真实签名或实盘执行

必须完成：
- 日报
- 拒绝报告
- 事故报告
- 只读查询
- 敏感信息脱敏

必须测试：
- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo test --workspace`
- 报告字段完整
- 敏感信息脱敏
- 只读命令不触发可变动作

验收要求：
- 运营模块默认只读
- 运营模块不依赖真实签名或实盘执行
- 报告从结构化事实生成

完成后按“已完成 / 验证 / 交付文件 / 剩余风险 / 下一步”格式回复。
```

### 10.20 `S8-01` 实现只读适配器 trait

```text
请按 `universal_arb_platform_v2_immutable_core_docs/docs/24_Codex_Development_Runbook.md` 执行。

当前阶段：阶段 8：只读场所数据适配器
当前任务包：S8-01 实现只读适配器 trait

开始前必须读取：`24`、`22`、`23`、`25`、`14`。

允许修改范围：
- `crates/arb-venue-data/**`
- 必要的只读适配器测试 fixture

禁止修改范围：
- 不表达下单
- 不表达撤单
- 不表达转账
- 不表达签名
- 不依赖 `arb-venue-exec` 或 `arb-signing`

必须完成：
- 行情只读接口
- 余额只读接口
- 仓位只读接口
- 工具信息只读接口
- 健康状态只读接口

必须测试：
- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo test --workspace`
- `cargo xtask check-crate-boundaries`
- 只读模块不能依赖签名或可变执行

验收要求：
- 接口不能表达账户变更动作
- `arb-venue-data` 不依赖 `arb-signing` 和 `arb-venue-exec`
- 只读边界清晰

完成后按“已完成 / 验证 / 交付文件 / 剩余风险 / 下一步”格式回复。
```

### 10.21 `S8-02` 实现第一个只读场所适配器

```text
请按 `universal_arb_platform_v2_immutable_core_docs/docs/24_Codex_Development_Runbook.md` 执行。

当前阶段：阶段 8：只读场所数据适配器
当前任务包：S8-02 实现第一个只读场所适配器

开始前必须读取：`24`、`22`、`23`、`25`、`14`、`21`。

允许修改范围：
- `crates/arb-venue-data/**`
- `fixtures/replay/**`
- 必要的只读适配器测试 fixture

禁止修改范围：
- 不连接真实账户写权限
- 不下单、不撤单、不转账、不签名
- 不依赖可变执行模块
- 不让默认测试依赖真实网络或真实 API key

必须完成：
- 原始事件到 `NormalizedEvent` 的映射
- 外部错误分类
- 新鲜度判断
- 断线、乱序、重复消息、缺字段处理
- 脱敏原始响应 fixture 和标准化事件期望输出

必须测试：
- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo test --workspace`
- 断线、乱序、重复消息、缺字段场景
- 标准化事件可回放
- 无网络、无 API key 时默认测试通过

验收要求：
- 能输出可回放标准化事件
- 只读适配器无账户变更能力
- 错误和新鲜度可被风控使用
- 真实网络检查必须显式 opt-in，不能替代离线 fixture 验收

完成后按“已完成 / 验证 / 交付文件 / 剩余风险 / 下一步”格式回复。
```

### 10.22 `S9-01` 装配端到端模拟链路

```text
请按 `universal_arb_platform_v2_immutable_core_docs/docs/24_Codex_Development_Runbook.md` 执行。

当前阶段：阶段 9：端到端模拟演练
当前任务包：S9-01 装配端到端模拟链路

开始前必须读取：`24`、`22`、`23`、`25`、`14`、`21`。

允许修改范围：
- `crates/arb-runtime/**`
- 必要的端到端 fixture
- 必要的装配测试

禁止修改范围：
- 运行时不写策略规则
- 运行时不写风控规则
- 运行时不写账本规则
- 运行时不写执行状态机规则
- 不连接真实交易 API

必须完成：
- 装配只读数据、事件存储、状态构建、策略、风控、模拟执行、账本、对账、报告
- 固定 fixture 端到端模拟
- 黄金结果比较

必须测试：
- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo test --workspace`
- `cargo xtask check-schema`
- `cargo xtask check-crate-boundaries`
- 固定 fixture 端到端运行
- 同一输入多次运行结果一致

验收要求：
- 端到端链路可回放
- 运行时只做装配
- 无真实资金动作

完成后按“已完成 / 验证 / 交付文件 / 剩余风险 / 下一步”格式回复。
```

### 10.23 `S9-02` 完善运行时退出和健康检查

```text
请按 `universal_arb_platform_v2_immutable_core_docs/docs/24_Codex_Development_Runbook.md` 执行。

当前阶段：阶段 9：端到端模拟演练
当前任务包：S9-02 完善运行时退出和健康检查

开始前必须读取：`24`、`22`、`23`、`25`。

允许修改范围：
- `crates/arb-runtime/**`
- 必要的运行时测试 fixture

禁止修改范围：
- 运行时不承载业务规则
- 不启动可变执行
- 不接真实交易 API

必须完成：
- 启动检查
- 熔断检查
- 任务生命周期
- 优雅退出
- 健康状态

必须测试：
- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo test --workspace`
- 配置错误拒绝启动
- 熔断打开时不启动可变执行
- 任务退出可观测

验收要求：
- 运行时只做依赖装配
- 启动失败原因明确
- 熔断能阻止可变执行

完成后按“已完成 / 验证 / 交付文件 / 剩余风险 / 下一步”格式回复。
```

### 10.24 `S10-01` 实现人工审批材料

```text
请按 `universal_arb_platform_v2_immutable_core_docs/docs/24_Codex_Development_Runbook.md` 执行。

当前阶段：阶段 10：人工审批模式
当前任务包：S10-01 实现人工审批材料

开始前必须读取：`24`、`22`、`23`、`25`、`14`。

允许修改范围：
- `crates/arb-ops/**`
- `crates/arb-execution/**`
- 必要的人工审批测试 fixture

禁止修改范围：
- 审批不能绕过风控
- 审批不能绕过账本
- 不提交真实资金动作
- 不接真实签名

必须完成：
- 审批摘要
- 风险检查摘要
- 执行计划摘要
- 批准记录
- 拒绝记录
- 过期和重复审批处理

必须测试：
- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo test --workspace`
- 批准、拒绝、过期、重复审批场景
- 审批记录可回放
- 审批材料脱敏

验收要求：
- 人工审批记录可审计
- 审批通过也只能进入受控流程
- 审批不能绕过风控和账本

完成后按“已完成 / 验证 / 交付文件 / 剩余风险 / 下一步”格式回复。
```

### 10.25 `S11-01` 实现可变执行 trait 和模拟实现

```text
请按 `universal_arb_platform_v2_immutable_core_docs/docs/24_Codex_Development_Runbook.md` 执行。

当前阶段：阶段 11：可变执行适配器和签名边界预演
当前任务包：S11-01 实现可变执行 trait 和模拟实现

开始前必须读取：`24`、`22`、`23`、`25`、`14`。

允许修改范围：
- `crates/arb-venue-exec/**`
- 必要的可变执行测试 fixture

禁止修改范围：
- 不连接真实交易场所
- 不提交真实资金动作
- 不默认开启实盘执行
- 不做策略判断或风控批准

必须完成：
- 提交订单 trait
- 撤单 trait
- 查询状态 trait
- 转账请求 trait
- 模拟实现
- 幂等键处理

必须测试：
- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo test --workspace`
- 默认功能关闭时不编译真实实现
- 幂等键重复提交不重复执行

验收要求：
- 不接真实交易场所
- 不使用真实资金
- 可变执行路径需要显式功能开关

完成后按“已完成 / 验证 / 交付文件 / 剩余风险 / 下一步”格式回复。
```

### 10.26 `S11-02` 实现签名边界和空签名器

```text
请按 `universal_arb_platform_v2_immutable_core_docs/docs/24_Codex_Development_Runbook.md` 执行。

当前阶段：阶段 11：可变执行适配器和签名边界预演
当前任务包：S11-02 实现签名边界和空签名器

开始前必须读取：`24`、`22`、`23`、`25`。

允许修改范围：
- `crates/arb-signing/**`
- 必要的签名边界测试 fixture

禁止修改范围：
- 不保存明文密钥
- 不输出明文密钥到日志、事件或报告
- 不让策略或运营模块依赖签名实现
- 不默认启用真实签名

必须完成：
- 签名请求
- 签名策略
- 空签名器
- 审计引用
- 脱敏日志

必须测试：
- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo test --workspace`
- 真实签名默认不可用
- 签名失败不会被当作成功
- 策略和运营模块不能依赖签名实现

验收要求：
- 真实签名实现不存在或不可用
- 日志和报告脱敏
- 签名边界不能被策略直接触达

完成后按“已完成 / 验证 / 交付文件 / 剩余风险 / 下一步”格式回复。
```

### 10.27 `S12-01` 生成实盘准备评审材料

```text
请按 `universal_arb_platform_v2_immutable_core_docs/docs/24_Codex_Development_Runbook.md` 执行。

当前阶段：阶段 12：受控实盘准备评审
当前任务包：S12-01 生成实盘准备评审材料

开始前必须读取：`24`、`22`、`23`、`25`、`14`、`21`。

允许修改范围：
- 受控实盘准备评审材料
- 运营报告模板或审查清单
- 必要的测试 fixture
- 必要的文档更新

禁止修改范围：
- 不默认实现真实执行
- 不接真实交易场所
- 不使用真实密钥
- 不提交真实资金动作
- 不删除或弱化安全检查

必须完成：
- 安全审查材料
- 风控审查材料
- 账本审查材料
- 执行审查材料
- 对账审查材料
- 回放审查材料
- 权限、密钥、事故流程审查材料
- 是否允许创建真实执行实现任务的结论

必须测试：
- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo test --workspace`
- `cargo xtask check-schema`
- `cargo xtask check-crate-boundaries`
- 端到端回放
- 依赖边界、功能开关、脱敏、kill switch 检查

验收要求：
- 外部审查完成前不能进入实盘
- 所有实盘能力默认关闭
- 所有实盘动作可追溯、可停止、可对账
- 事故响应流程已演练
- 只有评审通过后，才允许创建真实执行实现任务

个人自用治理补充：
- 如果系统只由所有者本人使用自己的小额资金，可以新增或更新 `review/personal_guarded_live_governance.md` 作为个人小额受控试运行治理材料。
- 个人路径不能声称外部独立审查通过，不能用于他人资金或商业服务，且不允许自动实盘。
- 个人小额受控试运行仍必须默认关闭实盘能力、禁止提现权限、限制资金和单笔额度、要求每笔人工确认、保留 kill switch、未知状态停机、执行后强制对账和事故记录。

完成后按“已完成 / 验证 / 交付文件 / 剩余风险 / 下一步”格式回复。
```

## 11. 可直接复制的验收提示词

任务包提示词用于开发，验收提示词用于检查。推荐节奏是：先发送一个任务包提示词，Codex 完成后发送任务包验收提示词；本阶段所有任务包都通过后，再发送阶段验收提示词。

中文说明：不要只看 Codex 的“已完成”。必须要求 Codex 给出证据文件、测试命令、模块边界检查结果和是否允许进入下一步的结论。

验收口径优先级：

1. 第 10 节任务包提示词中的 `允许修改范围`、`禁止修改范围`、`必须完成`、`必须测试`、`验收要求`。
2. 第 8 节任务包摘要中的对应字段。
3. 第 22、23、25 号文档中的阶段和模块要求。

字段映射：

- 第 8 节的 `修改范围` 等同于第 10 节的 `允许修改范围`。
- 第 8 节的 `完成内容` 等同于第 10 节的 `必须完成`。
- 第 8 节的 `测试` 等同于第 10 节的 `必须测试`。
- 第 8 节的 `验收` 等同于第 10 节的 `验收要求`。

中文说明：如果第 8 节摘要和第 10 节详细任务包冲突，以第 10 节为准；如果冲突涉及真实执行、签名、资金、凭证或模块越界，必须按更保守规则处理并停止进入下一步。

### 11.1 任务包验收提示词

每完成一个任务包后，复制下面提示词，把阶段名和任务包编号替换成当前任务。

```text
请执行任务包验收检查。

当前阶段：阶段 X：<阶段名称>
当前任务包：<任务包编号和名称，例如 S1-01 实现 arb-domain 基础类型>

请对照：
1. `universal_arb_platform_v2_immutable_core_docs/docs/24_Codex_Development_Runbook.md` 中该任务包的：
   - 允许修改范围
   - 禁止修改范围
   - 必须完成
   - 必须测试
   - 验收要求

2. `universal_arb_platform_v2_immutable_core_docs/docs/23_Module_Architecture_Map.md` 中对应模块的：
   - 定位
   - 输入
   - 输出
   - 数据归属
   - 公开接口
   - 禁止行为
   - 测试责任
   - 允许依赖和禁止依赖

3. 当前代码和测试结果。

请检查：
- 任务包要求是否完成
- 是否修改了不允许修改的文件
- 必须测试是否都运行
- 验收要求是否满足
- 模块职责是否越界
- 是否出现禁止依赖
- 是否存在未测试、临时实现、后续补充、阻塞问题
- 是否需要同步更新 `22`、`23`、`24`、`25` 或 `README`

请输出：
1. 任务包验收结论：通过 / 不通过
2. 已满足项
3. 未满足项
4. 模块边界检查结果
5. 已运行测试命令
6. 证据文件路径
7. 阻塞下一个任务包的问题
8. 是否允许进入下一个任务包

要求：
- 不要只回答“通过”
- 每个结论都要给出证据文件或测试命令
- 如果未运行某个测试，必须说明原因和是否阻塞
- 如果不允许进入下一个任务包，请列出需要先修复的问题
```

### 11.2 阶段验收提示词

本阶段所有任务包都通过后，复制下面提示词，把阶段名和已完成任务包列表替换成当前阶段。

```text
请执行阶段验收检查。

当前阶段：阶段 X：<阶段名称>
已完成任务包：
- <任务包编号 1>
- <任务包编号 2>
- <任务包编号 3>

请对照：
1. `universal_arb_platform_v2_immutable_core_docs/docs/22_Development_Execution_Plan.md` 中当前阶段的：
   - 目标
   - 需要完成的内容
   - 交付点
   - 测试方案
   - 验收方案

2. `universal_arb_platform_v2_immutable_core_docs/docs/23_Module_Architecture_Map.md` 中本阶段涉及模块的：
   - 模块职责
   - 输入输出
   - 数据归属
   - 公开接口
   - 禁止行为
   - 允许依赖
   - 禁止依赖
   - 模块完成定义

3. `universal_arb_platform_v2_immutable_core_docs/docs/24_Codex_Development_Runbook.md` 中当前阶段和已完成任务包的要求。

请检查：
- 阶段交付点是否全部完成
- 阶段测试方案是否全部执行
- 阶段验收方案是否全部满足
- 已完成任务包之间的接口是否一致
- 模块职责是否越界
- 是否出现禁止依赖
- 是否有未测试、临时实现、后续补充、阻塞问题
- 是否需要同步更新 `14`、`21`、`22`、`23`、`24`、`25` 或 `README`
- 是否允许进入下一阶段

请输出：
1. 阶段验收结论：通过 / 不通过
2. 阶段交付点完成情况
3. 阶段测试执行情况
4. 阶段验收方案逐项结果
5. 模块边界检查结果
6. 证据文件路径
7. 阻塞下一阶段的问题
8. 是否允许进入下一阶段

要求：
- 不要只回答“通过”
- 每个阶段验收项都要给出证据
- 如果某项未完成，说明是否阻塞下一阶段
- 只有无阻塞问题时，才能回答“允许进入下一阶段”
```

### 11.3 使用顺序

```text
发送任务包提示词
  -> Codex 开发并测试
  -> 发送任务包验收提示词
  -> 任务包验收通过
  -> 发送下一个任务包提示词
  -> 本阶段所有任务包通过
  -> 发送阶段验收提示词
  -> 阶段验收通过
  -> 进入下一阶段
```

中文说明：任务包验收回答“能不能进入下一个任务包”；阶段验收回答“能不能进入下一个阶段”。两者不能互相替代。

## 12. 失败处理规则

Codex 遇到失败时按以下顺序处理：

1. 先判断是实现错误、测试错误、环境缺失还是需求冲突。
2. 能在当前任务范围内修复的，直接修复并重新验证。
3. 需要跨模块修改的，先说明原因，再最小范围调整。
4. 需要真实凭证、真实网络、真实交易权限的，停止并说明该阶段不允许。
5. 如果验证命令因为工具未安装失败，记录命令、错误和替代检查。

中文说明：不能用删除测试、放宽 schema、移除边界检查来“修复”失败。

## 13. 文档同步规则

以下情况必须同步更新文档：

| 代码变化 | 必须检查的文档 |
|---|---|
| 新增或删除 crate | `22`, `23`, `24`, `README` |
| 修改模块依赖 | `22`, `23`, `24`, `25` |
| 修改 schema 字段 | `14`, 对应 schema 文件、fixture、合同测试 |
| 修改状态机 | `21`, `22`, `23`, `25`, 对应模块测试 |
| 修改风控检查 | `22`, `23`, `25`, 对应风控测试 |
| 修改执行流程 | `21`, `22`, `23`, `25`, 对应执行测试 |
| 修改账本规则 | `22`, `23`, `25`, 对应账本测试 |
| 修改实盘能力开关 | `22`, `23`, `24`, `25` |
| 修改 Codex 开发流程 | `24`, `README` |
| 修改核心架构原则 | `25`, `22`, `23`, `24`, `README` |

中文说明：文档不是事后装饰。对外合同、模块边界、状态机和安全开关发生变化时，文档必须与代码同轮更新。

## 14. Codex 交接摘要模板

长任务结束或上下文切换前，Codex 应留下交接摘要：

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

中文说明：交接摘要要能让下一次 Codex 继续开发，而不是重新分析整个项目。

## 15. 正式开发前检查清单

进入 Rust 代码开发前，确认以下事项：

- `22_Development_Execution_Plan.md` 已定义阶段和验收。
- `23_Module_Architecture_Map.md` 已定义模块边界。
- `24_Codex_Development_Runbook.md` 已定义 Codex 执行方式。
- `25_Core_Architecture_Reference.md` 已合并核心架构规则。
- `24_Codex_Development_Runbook.md` 已包含任务包提示词和验收提示词。
- 路径约定已确认：代码写入 `CODE_ROOT`，文档短路径按 `DOC_ROOT` 解析。
- 正式质量门使用 Rust `xtask`，不依赖 Node.js、真实网络或真实凭证。
- 真实执行和真实签名 feature 默认关闭，阶段 9 前不得出现真实资金动作路径。
- 人工审批语义已确认：审批只解除同一计划哈希的阻塞，不能绕过风控、账本、对账或 kill switch。
- 阶段 8 只读适配器默认离线可测，联网检查必须显式 opt-in。
- schema 文件可解析。
- 文档不是纯英文。
- 阶段 0 的第一批任务明确。
- 默认不开发真实执行和真实签名。

若以上全部满足，可以让 Codex 从阶段 0 开始创建工程骨架。
