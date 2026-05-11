# 个人小额受控试运行证据收集教程

状态：证据收集教程，不是实盘批准。
适用文档：`personal_guarded_live_evidence_index.md`、`personal_guarded_live_pilot_checklist.md`。

中文说明：本文面向中文用户，逐项说明证据从哪里来、如何安全启动离线检查、如何
脱敏、如何填写证据索引和清单。英文 evidence ID 和命令会保留，因为它们是文档
之间互相引用的稳定标识。

本文不是外部审计，不批准自动实盘，也不要求把真实密钥、完整账户号、原始私有
余额、API secret、私钥、助记词、session、token 或 webhook secret 放进仓库。

## 1. 总原则

每个证据都按同一个流程处理：

1. 在仓库外收集原始证据。
2. 对截图、导出、记录做脱敏。
3. 给脱敏证据分配一个稳定引用编号。
4. 先更新 `personal_guarded_live_evidence_index.md`。
5. 证据索引完整后，再更新 `personal_guarded_live_pilot_checklist.md` 的状态。

中文说明：仓库只保存“脱敏引用”和“结论”。原始截图、完整账号、真实余额、真实
API key、secret、签名材料和私有配置都应由所有者本地保存，不进入 repo、日志、
fixture、报告或提示词。

推荐证据引用格式：

```text
redacted-local:YYYY-MM-DD-short-name-v1
drill:YYYY-MM-DD-short-name-v1
cmd:YYYY-MM-DD-command-name-v1
owner-note:YYYY-MM-DD-short-name-v1
```

示例：

```text
redacted-local:2026-05-11-no-withdraw-permission-v1
drill:2026-05-11-kill-switch-tabletop-v1
cmd:2026-05-11-replay-full-pipeline-v1
owner-note:2026-05-11-pilot-scope-v1
```

## 2. 如何安全启动

收集证据时，只允许先跑离线模拟和回放：

```bash
cargo run -p arb-runtime -- health fixtures/replay/full_pipeline_simulated
cargo run -p arb-runtime -- replay fixtures/replay/full_pipeline_simulated
cargo xtask replay-full-pipeline
```

看到这些信号，说明仍在安全离线范围内：

- `execution_mode=Simulated`
- `mutable_execution_started=false`
- replay 输出匹配预期 artifacts

中文说明：这些命令只运行离线 fixture，不访问真实交易 API，不读取真实凭证，不启动
可变执行。不要为了收集证据打开真实交易权限。

## 3. 如何填写两个文档

### 3.1 填写证据索引

文件：`personal_guarded_live_evidence_index.md`

填写规则：

- `当前状态 / Current status` 可写 `Present`、`Needs latest run`、`Missing`、`Pass`、`Fail` 或 `Not run`。
- 仓库内证据通常来自文件存在性、配置默认值或命令结果。
- 所有者证据必须先有脱敏引用，才能从 `Missing` 改成 `Pass`。
- 命令证据要写日期、结果和一句摘要，不粘贴大段日志。

示例：

```text
| `owner-no-withdrawal-permission` | 交易 key 无提现或转出权限。 | 脱敏权限截图/导出。 | Pass: redacted-local:2026-05-11-no-withdraw-permission-v1 |
```

### 3.2 填写试运行清单

文件：`personal_guarded_live_pilot_checklist.md`

填写规则：

- 只要证据索引缺失，清单对应行就保持 `Not reviewed`。
- 如果证据证明不满足要求，写 `Fail`。
- 只有证据索引、命令结果和脱敏引用都支持该结论，才写 `Pass`。

示例：

```text
| No withdrawal / 无提现 | 交易 API key 不能提现或转出。 | `owner-no-withdrawal-permission` | Pass |
```

中文说明：不要先把清单改成 `Pass` 再补证据。顺序必须是先证据索引，后清单状态。

## 4. 仓库内证据怎么收集

这些证据来自仓库文件、配置、测试或 fixture，不需要真实交易账户。

| 证据 ID | 怎么启动或检查 | 证据从哪里来 | 怎么填写 |
|---|---|---|---|
| `repo-governance-profile` | 运行 `sed -n '1,140p' universal_arb_platform_v2_immutable_core_docs/review/personal_guarded_live_governance.md`。 | 个人治理文档存在，并说明个人路径不是外部审计、不允许自动实盘。 | 如果文件存在且限制仍然明确，状态保持 `Present`。 |
| `repo-pilot-checklist` | 运行 `sed -n '1,180p' universal_arb_platform_v2_immutable_core_docs/review/personal_guarded_live_pilot_checklist.md`。 | 个人试运行清单存在，并且每个门禁默认阻塞或未审查。 | 如果清单包含 evidence ID 且没有默认放行，状态保持 `Present`。 |
| `repo-evidence-collection-guide` | 打开本文。 | 本教程说明如何收集、脱敏和填写证据。 | 文件存在且中文说明完整时，状态保持 `Present`。 |
| `repo-checklist-audit` | 运行 `sed -n '1,180p' universal_arb_platform_v2_immutable_core_docs/review/personal_guarded_live_checklist_audit_report.md`。 | 清单审计报告存在，并记录剩余阻塞。 | 如果报告没有批准实盘，状态保持 `Present`。 |
| `repo-controlled-review` | 运行 `sed -n '1,80p' universal_arb_platform_v2_immutable_core_docs/review/controlled_live_readiness_review.md`。 | 受控实盘评审材料区分个人路径和外部审查。 | 如果明确“个人路径不是外部审查通过”，状态保持 `Present`。 |
| `repo-default-config` | 运行 `sed -n '1,80p' templates/config.template.yaml`。 | 运行时配置模板。 | 默认模式安全、live/auto-live/real-signing 都关闭时，状态为 `Present`。 |
| `repo-canonical-config` | 运行 `sed -n '1,90p' universal_arb_platform_v2_immutable_core_docs/templates/config.template.yaml`。 | 文档侧权威配置模板。 | 默认 venue disabled，且 `can_withdraw: false` 时，状态为 `Present`。 |
| `repo-live-feature-gate` | 运行 `sed -n '1,80p' crates/arb-venue-exec/Cargo.toml`。 | 可变执行 crate 的 Cargo features。 | 默认 features 为空或安全，live feature 必须显式 opt-in。 |
| `repo-signing-feature-gate` | 运行 `sed -n '1,80p' crates/arb-signing/Cargo.toml`。 | 签名 crate 的 Cargo features。 | 默认 features 为空或安全，real-signing 必须显式 opt-in。 |
| `repo-config-tests` | 运行 `cargo test -p arb-config`。 | 配置模块测试输出。 | 在 Latest Command Evidence 写日期和结果；本行可改为 `Pass` 或 `Needs latest run`。 |
| `repo-runtime-tests` | 运行 `cargo test -p arb-runtime`。 | 运行时测试输出。 | 记录日期和结果；备注 health/replay/kill switch 相关测试通过。 |
| `repo-signing-tests` | 运行 `cargo test -p arb-signing`。 | 签名模块测试输出。 | 记录默认无真实签名和脱敏测试通过。 |
| `repo-venue-exec-tests` | 运行 `cargo test -p arb-venue-exec`。 | 可变执行边界测试输出。 | 记录默认无 live exec、幂等和 fail-closed 测试通过。 |
| `repo-ops-redaction-tests` | 运行 `cargo test -p arb-ops`。 | 运营报告测试输出。 | 记录报告脱敏测试通过。 |
| `repo-boundary-check` | 运行 `cargo xtask check-crate-boundaries`。 | 依赖边界检查输出。 | 在 Latest Command Evidence 写日期、结果和 `cmd:*` 引用。 |
| `repo-schema-check` | 运行 `cargo xtask check-schema`。 | schema 和 fixture 检查输出。 | 记录日期和结果。 |
| `repo-doc-check` | 运行 `cargo xtask check-docs`。 | 文档检查输出。 | 记录日期和结果。 |
| `repo-full-replay` | 运行 `cargo xtask replay-full-pipeline`。 | 离线全链路回放输出。 | 记录日期、结果和匹配 artifact 数量。 |
| `repo-reconciliation-fixtures` | 运行 `ls fixtures/replay/reconciliation_match fixtures/replay/reconciliation_mismatch`。 | 对账一致和不一致 fixture 目录。 | 两个目录都存在时保持 `Present`；owner drill 仍要另做。 |
| `repo-unknown-state-fixtures` | 运行 `ls fixtures/replay/incident_unknown_state fixtures/replay/full_pipeline_simulated/cases/unknown_state`。 | 未知状态事故 fixture 目录。 | 目录存在时保持 `Present`；owner drill 仍要另做。 |
| `repo-manual-approval-fixtures` | 运行 `ls fixtures/replay/manual_approval_approved fixtures/replay/manual_approval_rejected`。 | 人工审批通过/拒绝 fixture 目录。 | fixture 存在且仍离线时保持 `Present`。 |

## 5. 所有者证据怎么收集

这些证据来自所有者本人、交易所、钱包、运行环境或本地演练。不要把原件放进仓库。

| 证据 ID | 怎么收集 | 填写前必须脱敏 | 怎么填写 |
|---|---|---|---|
| `owner-pilot-ownership` | 写一份所有者说明：本 pilot 只使用本人资金，不涉及第三方、团队、客户或商业资金。 | 法定姓名、地址、完整账号、身份证明。 | 在证据索引写 `Pass: owner-note:YYYY-MM-DD-pilot-ownership-v1`；清单 Pilot ownership 可改 `Pass`。 |
| `owner-risk-acceptance` | 写一份风险接受说明：pilot 资金可能全部损失，并列出停止条件。 | 原始余额、完整账号、私有资产明细。 | 用 `owner-note:*` 引用；说明资金上限和停止条件，不写原始余额。 |
| `owner-scope` | 写清楚试运行范围：场所、策略、账户/托管别名、工具/资产/链、最长持续时间。 | 完整账户号、完整钱包地址、UID、邮箱、手机号。 | 用 `owner-note:*` 或 `redacted-local:*` 引用；范围字段缺一项就不要改 `Pass`。 |
| `owner-isolated-account` | 截图或导出交易所子账户标签、隔离钱包标签、托管 bucket 标识。 | 完整账号、UID、钱包地址、邮箱、手机号。 | 只记录脱敏引用，例如 `redacted-local:*`；不要把截图贴进 markdown。 |
| `owner-no-withdrawal-permission` | 打开交易所 API key 权限页面或导出权限，确认提现和转出关闭。 | API key 值、secret、UID、完整 IP 白名单、完整账号。 | 证据摘要应写明 `can_withdraw=false`、transfer-out disabled。 |
| `owner-minimum-permission` | 用同一权限页面确认只开启 pilot 必需的 read/trade 权限。 | API key 值、secret、完整账号、私有标识。 | 记录允许权限列表，并明确没有多余权限。 |
| `owner-ip-device-restriction` | 如果交易所支持，截图或记录 IP/device 限制已启用。 | 完整 IP 地址、设备 ID、账号标识。 | 写明 enabled；如果交易所不支持，记录 `not supported by venue`，并保持清单 `Not reviewed` 或另写补偿控制。 |
| `owner-key-rotation` | 写一份撤销和轮换 key 的步骤：在哪里撤销、如何新建、如何更新配置引用且不改代码。 | API key 值、secret、完整截图标识。 | 用 `owner-note:*` 引用；写预计撤销耗时。 |
| `owner-capital-cap` | 写明隔离账户允许投入的最大 pilot 资金上限。 | 当前完整余额、完整账号、私有资产明细。 | 用 `owner-note:*` 引用；写上限或策略，不写当前完整余额。 |
| `owner-per-order-cap` | 写明单笔订单或单个账户变更动作的最大名义金额。 | 私有余额、完整账号。 | 用 `owner-note:*` 或 config 引用；数值必须具体到可执行。 |
| `owner-daily-loss-cap` | 写明单日已实现/未实现亏损停止阈值。 | 原始余额、账户细节。 | 用 `owner-note:*` 或 config 引用；必须写达到阈值后的停止动作。 |
| `owner-max-open-orders` | 写明最大活跃订单数或最大未完成动作数。 | 完整账号、真实订单 ID。 | 用 `owner-note:*` 或 config 引用；写达到上限后的处理方式。 |
| `owner-manual-confirmation` | 写确认流程：所有者看同一个 plan hash，批准/拒绝，审批会过期，改计划必须重新审批。 | 包含完整账号的计划详情、真实订单 ID。 | 用 runbook 或 `owner-note:*` 引用；必须写明审批不能绕过风控、熔断、账本、对账或权限。 |
| `owner-real-signing-policy` | 写明真实签名默认关闭；如果确实需要，另写独立 owner-approved signing policy。 | 私钥、助记词、签名 payload、签名结果、托管签名凭据。 | 没有真实签名政策时，记录 disabled-by-default；不要为了通过清单创建真实签名材料。 |
| `owner-kill-switch-drill` | 做 tabletop drill，覆盖 global、execution dispatch、execution mode、venue、strategy、account、instrument、asset、chain。 | 完整账号、真实订单 ID、私有余额。 | 使用本文第 6 节模板；每个维度都覆盖后，清单 kill switch 相关行才能 `Pass`。 |
| `owner-unknown-state-drill` | 通过 replay/tabletop 引入未知场所状态或未知执行状态，验证 affected scope 停机。 | 真实订单号、完整账号、私有交易细节。 | 写 drill 引用和 incident 引用；必须说明没有把 unknown 当 success。 |
| `owner-reconciliation-drill` | 通过 replay/tabletop 制造执行报告、账本、余额、仓位或成交不一致，验证下一轮被阻断。 | 原始私有余额、完整账号、真实成交 ID。 | 写 drill 引用、对账引用和 incident 引用。 |
| `owner-permission-failure-drill` | 模拟权限不足或权限缺失，确认 dispatch fail closed。 | API key、secret、完整权限截图。 | 写 drill 引用，并关联脱敏权限证据。 |
| `owner-signer-failure-drill` | 模拟 signer disabled、unavailable、rejected 或 policy mismatch，确认不能当成功。 | 私钥、签名、签名 payload、signer token。 | 写 drill 引用；绝不保存签名材料。 |
| `owner-post-action-reconciliation` | 写 live action 后对账流程：执行报告、账本分录、场所余额、场所仓位、成交都要核对。 | 原始私有余额、完整账号、真实成交 ID。 | 用 runbook 或 `owner-note:*` 引用；缺流程时清单保持阻塞。 |
| `owner-incident-note-template` | 建一个事故记录模板，覆盖 mismatch、unknown state、permission failure、signer failure。 | secret、原始余额、完整账号。 | 用模板引用；模板至少包含时间、范围、触发原因、动作、状态。 |
| `owner-daily-review` | 建一个 session 结束复盘清单，检查运营报告、未解决事故、开放订单和对账状态。 | 私有账号细节、原始余额。 | 用 `owner-note:*` 引用；每次 session 后记录日期和结果。 |
| `owner-final-decision` | 所有行都通过后，填写最终 owner decision。 | secret、完整账号、原始余额。 | 用 `owner-note:*` 引用；必须写明这是个人风险接受，不是外部审查。 |

## 6. 演练记录模板

每个 owner drill 都用同一模板。原始记录如果含有私有细节，保存在仓库外；仓库只写
脱敏引用。

```text
演练编号 (Drill ID):
日期 (Date):
证据引用 (Evidence reference):
范围 (Scope):
前提条件 (Precondition):
尝试动作 (Action attempted):
预期阻断 (Expected block):
实际结果 (Observed result):
审计/事件/事故引用 (Audit/event/incident reference):
是否通过 (Pass/Fail):
备注 (Notes):
```

中文字段解释：

- `Drill ID`：演练编号，例如 `drill:2026-05-11-kill-switch-tabletop-v1`。
- `Date`：演练日期。
- `Evidence reference`：脱敏证据引用。
- `Scope`：演练范围，例如场所、账户、策略、链、资产。
- `Precondition`：演练前提，例如打开 kill switch 或注入 unknown state。
- `Action attempted`：尝试的动作，例如尝试分发账户变更。
- `Expected block`：预期阻断结果。
- `Observed result`：实际观察结果。
- `Audit/event/incident reference`：审计、事件或事故引用。
- `Pass/Fail`：是否通过。
- `Notes`：补充说明，不写秘密。

### 6.1 Kill switch 演练最低要求

```text
范围 (Scope): global, execution dispatch, execution mode, venue, strategy, account, instrument, asset, chain
预期阻断 (Expected block): account-changing dispatch is blocked and audit/event evidence is produced
是否通过 (Pass/Fail):
```

中文说明：只证明 global stop 不够。必须覆盖执行分发、执行模式、场所、策略、账户、
工具、资产和链。

### 6.2 Unknown state 演练最低要求

```text
前提条件 (Precondition): unknown venue or execution state is introduced through replay/tabletop
预期阻断 (Expected block): affected scope pauses, no success is assumed, incident reference is created
是否通过 (Pass/Fail):
```

中文说明：未知状态不能当作成功，也不能继续下一轮账户变更。

### 6.3 Reconciliation mismatch 演练最低要求

```text
前提条件 (Precondition): simulated mismatch between execution report, ledger entry, venue balance/position/fill
预期阻断 (Expected block): next live cycle is blocked for affected scope and incident reference is created
是否通过 (Pass/Fail):
```

中文说明：对账不一致时，必须阻断 affected scope 的下一轮 live cycle。

### 6.4 Permission / signer failure 演练最低要求

```text
前提条件 (Precondition): permission or signer is disabled, missing, rejected or policy-mismatched
预期阻断 (Expected block): dispatch/signing fails closed and is not treated as success
是否通过 (Pass/Fail):
```

中文说明：权限失败和签名失败都必须 fail closed，不能当作已执行或已签名成功。

## 7. 填写示例

证据索引 owner 行示例：

```text
| `owner-no-withdrawal-permission` | 交易 key 无提现或转出权限。 | 脱敏权限截图/导出。 | Pass: redacted-local:2026-05-11-no-withdraw-permission-v1 |
```

命令证据示例：

```text
| `cargo xtask replay-full-pipeline` | 2026-05-11 | Pass | cmd:2026-05-11-replay-full-pipeline-v1; matched expected offline artifacts |
```

清单行示例：

```text
| No withdrawal / 无提现 | 交易 API key 不能提现或转出。 | `owner-no-withdrawal-permission` | Pass |
```

中文说明：如果证据只是一句“我确认了”，但没有脱敏引用、日期、范围和结论，
该项不能改成 `Pass`。

## 8. 禁止做法

- 不要粘贴 API key、API secret、private key、seed phrase、session token 或 webhook secret。
- 不要粘贴完整账户号、钱包私有材料或原始私有余额。
- 不要把 AI 输出当作真实交易所权限证据。
- 不要先把 checklist 改成 `Pass` 再补 evidence index。
- 任一证据仍是 `Missing`、`Not reviewed`、`Fail` 或已过期时，不要启动 `GuardedLivePersonal`。

中文说明：证据清单的目的不是增加形式成本，而是防止真实资金动作在权限、熔断、
对账、签名或未知状态没有证明前被放开。
