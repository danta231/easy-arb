# 个人小额受控试运行清单 / Personal Guarded Live Pilot Checklist

范围：仅限系统所有者本人使用自己的小额封顶资金。
默认状态：**所有最低门槛全部 Pass 前保持 Blocked**。

中文说明：本清单只适用于系统所有者本人使用自己的小额封顶资金。它不是外部审计，
不允许自动实盘，不允许提现权限，不允许把真实密钥、完整账户号、私钥、助记词、
token、session 或 webhook 凭证写入代码、日志、fixture、报告或提示词。

审计说明：本清单通过只代表个人所有者接受小额受控试运行风险；不得记录为外部
独立审查通过，不得用于他人资金、团队资金或商业服务，也不得批准自动实盘。

## 使用规则 / Use Rules

- 每一项只能标记为 `Pass`、`Fail` 或 `Not reviewed`。
- 只能使用 `personal_guarded_live_evidence_index.md` 中的脱敏 evidence ID。
- 如果证据索引中缺少某个 evidence ID，必须先补证据索引，不能直接把清单改成 `Pass`。
- 任一项是 `Fail` 或 `Not reviewed` 时，只能保持 `ReadOnlyPersonal`、`ManualExecutionPersonal` 或模拟模式。
- `GuardedLivePersonal` 要求下面所有条目都是 `Pass`。
- `AutonomousLivePersonal` 不允许由本清单批准。
- 所有者确认只能解除同一 execution-plan hash 的人工门禁，不能绕过风控、资本预留、feature flag、运行时权限、kill switch、账本、对账或事故处理。

中文说明：人工确认不是“万能批准”。如果计划内容变化、plan hash 变化、权限未知、
熔断打开、对账不一致或签名失败，都必须继续阻断。

## 所有者决策门 / Owner Decision Gates

| 门禁 | 要求 | 证据引用 | 状态 |
|---|---|---|---:|
| Pilot ownership / 所有权范围 | Pilot 只使用所有者本人资金，不涉及第三方、团队、客户或商业资金。 | `owner-pilot-ownership`, `owner-final-decision` | Not reviewed |
| Risk acceptance / 风险接受 | 所有者记录 pilot 资金可能全部损失。 | `owner-risk-acceptance` | Not reviewed |
| Scope statement / 范围声明 | Pilot 范围列明场所、策略、账户、工具和最长持续时间。 | `owner-scope` | Not reviewed |
| Capital cap / 资金上限 | Pilot 只使用所有者愿意损失的封顶资金；证据记录上限，不记录原始私有余额。 | `owner-capital-cap` | Not reviewed |
| Stop criteria / 停止条件 | 所有者定义亏损、未知状态、对账不一致、错误爆发和人工担忧时的停止条件。 | `owner-risk-acceptance`, `owner-scope` | Not reviewed |
| No external-audit claim / 不声称外部审查 | 所有者记录这只是个人风险接受，不是独立外部审查。 | `owner-final-decision` | Not reviewed |

## 账户与权限门 / Account and Permission Gates

| 门禁 | 要求 | 证据引用 | 状态 |
|---|---|---|---:|
| Isolated account / 隔离账户 | Pilot 使用独立交易所子账户、钱包或托管 bucket。 | `owner-isolated-account` | Not reviewed |
| No withdrawal / 无提现权限 | 交易 API key 不能提现或转出。 | `owner-no-withdrawal-permission` | Not reviewed |
| Minimum permission / 最小权限 | API key 只有 pilot 必需的 read/trade 权限。 | `owner-minimum-permission` | Not reviewed |
| IP/device restriction / IP 或设备限制 | 交易所支持时，API key 受 IP 或设备限制。 | `owner-ip-device-restriction` | Not reviewed |
| Key rotation path / key 轮换路径 | 所有者可不改代码撤销和轮换 key。 | `owner-key-rotation` | Not reviewed |
| No secrets in repo / 仓库无秘密 | 仓库文件不保存 API secret、私钥、助记词、token 或 webhook secret。 | `repo-ops-redaction-tests`, `repo-signing-tests`, `repo-doc-check` | Not reviewed |

## 运行时与功能开关门 / Runtime and Feature Gates

| 门禁 | 要求 | 证据引用 | 状态 |
|---|---|---|---:|
| Default-off config / 默认关闭配置 | 默认配置保持 `ReadOnly`，live 和 auto-live 关闭。 | `repo-default-config`, `repo-canonical-config`, `repo-config-tests` | Not reviewed |
| Cargo feature gate / Cargo 功能门 | `live-exec` 和 `real-signing` 必须显式 opt-in，默认 features 为空或安全。 | `repo-live-feature-gate`, `repo-signing-feature-gate`, `repo-boundary-check` | Not reviewed |
| Manual confirmation / 人工确认 | 每个账户变更动作在分发前都需要所有者确认。 | `owner-manual-confirmation`, `repo-manual-approval-fixtures` | Not reviewed |
| Approval no-bypass / 审批不绕过门禁 | 人工审批不能绕过风控、资本预留、kill switch、权限、账本或对账。 | `owner-manual-confirmation`, `repo-manual-approval-fixtures` | Not reviewed |
| Per-order limit / 单笔上限 | 已配置单笔订单或单个动作名义金额上限。 | `owner-per-order-cap` | Not reviewed |
| Daily loss limit / 单日亏损上限 | 已配置单日已实现/未实现亏损停止阈值。 | `owner-daily-loss-cap` | Not reviewed |
| Max open orders / 最大开放动作 | 已限制最大活跃订单和未完成动作数量。 | `owner-max-open-orders` | Not reviewed |
| Real signing policy / 真实签名策略 | 真实签名默认关闭；如需真实签名，必须有单独 owner-approved policy、脱敏证据、kill switch 覆盖、失败停机和不暴露签名材料。 | `owner-real-signing-policy`, `repo-signing-feature-gate`, `repo-signing-tests` | Not reviewed |

## 熔断门 / Kill Switch Gates

| 门禁 | 要求 | 证据引用 | 状态 |
|---|---|---|---:|
| Global stop / 全局停止 | 所有者可以停止全部账户变更动作。 | `owner-kill-switch-drill`, `repo-config-tests`, `repo-runtime-tests` | Not reviewed |
| Execution dispatch stop / 执行分发停止 | 即使风控批准，所有者也可以停止可变执行分发。 | `owner-kill-switch-drill`, `repo-runtime-tests` | Not reviewed |
| Execution-mode stop / 执行模式停止 | 所有者可以阻断 `GuardedLivePersonal` 或更强模式。 | `owner-kill-switch-drill`, `repo-config-tests` | Not reviewed |
| Venue stop / 场所停止 | 所有者可以停止特定场所。 | `owner-kill-switch-drill`, `repo-config-tests` | Not reviewed |
| Strategy stop / 策略停止 | 所有者可以停止特定策略。 | `owner-kill-switch-drill`, `repo-config-tests` | Not reviewed |
| Account stop / 账户停止 | 所有者可以停止特定账户或托管 bucket。 | `owner-kill-switch-drill`, `repo-config-tests` | Not reviewed |
| Instrument stop / 工具停止 | 所有者可以停止特定交易工具。 | `owner-kill-switch-drill`, `repo-config-tests` | Not reviewed |
| Asset stop / 资产停止 | 所有者可以停止特定资产。 | `owner-kill-switch-drill`, `repo-config-tests` | Not reviewed |
| Chain stop / 链或结算网络停止 | 所有者可以停止特定链或结算网络。 | `owner-kill-switch-drill`, `repo-config-tests` | Not reviewed |
| Drill record / 演练记录 | 熔断演练记录预期阻断动作、受影响范围、时间戳和证据引用。 | `owner-kill-switch-drill` | Not reviewed |

## 回放、对账与事故门 / Replay, Reconciliation, and Incident Gates

| 门禁 | 要求 | 证据引用 | 状态 |
|---|---|---|---:|
| End-to-end replay / 端到端回放 | pilot 前完整 pipeline 回放通过。 | `repo-full-replay` | Not reviewed |
| Unknown-state drill / 未知状态演练 | 未知场所或执行状态会停止受影响范围并生成事故记录。 | `owner-unknown-state-drill`, `repo-unknown-state-fixtures` | Not reviewed |
| Reconciliation drill / 对账演练 | 模拟不一致会生成事故并阻断受影响范围。 | `owner-reconciliation-drill`, `repo-reconciliation-fixtures` | Not reviewed |
| Permission failure drill / 权限失败演练 | 权限缺失或不足必须 fail closed，不能分发执行。 | `owner-permission-failure-drill`, `owner-no-withdrawal-permission` | Not reviewed |
| Signer failure drill / 签名失败演练 | 签名不可用、禁用、拒绝或策略不匹配不能当作成功。 | `owner-signer-failure-drill`, `repo-signing-tests` | Not reviewed |
| Post-action reconciliation / 动作后对账 | 每个 live action 后，必须在下一轮 live cycle 前对执行报告、账本分录、场所余额、仓位和成交做对账。 | `owner-post-action-reconciliation` | Not reviewed |
| Incident note / 事故记录 | mismatch、unknown state、permission failure 或 signer failure 都有事故记录。 | `owner-incident-note-template` | Not reviewed |
| Daily review / 每日复盘 | 所有者在每次 pilot session 后检查运营报告。 | `owner-daily-review` | Not reviewed |

## 命令门 / Command Gate

pilot 前立即运行这些命令，并把日期和结果写入 `personal_guarded_live_evidence_index.md`。
中文说明：只记录通过/失败和摘要，不粘贴大段日志或私有输出。

| 命令 | 必须结果 | 状态 |
|---|---|---:|
| `cargo fmt --all -- --check` | Pass | Not reviewed |
| `cargo clippy --workspace --all-targets -- -D warnings` | Pass | Not reviewed |
| `cargo test --workspace` | Pass | Not reviewed |
| `cargo xtask check-schema` | Pass | Not reviewed |
| `cargo xtask check-crate-boundaries` | Pass | Not reviewed |
| `cargo xtask check-docs` | Pass | Not reviewed |
| `cargo xtask replay-full-pipeline` | Pass | Not reviewed |

## 最终所有者决策 / Final Owner Decision

```text
决定 (Decision): approve / reject personal guarded live pilot
所有者引用 (Owner reference):
日期 (Date):
试运行模式 (Pilot mode):
场所范围 (Venue scope):
策略范围 (Strategy scope):
账户/托管范围 (Account/custody scope):
工具/资产/链范围 (Instrument/asset/chain scope):
最长持续时间 (Maximum duration):
资金上限 (Capital cap):
单笔上限 (Per-order cap):
单日亏损上限 (Daily loss cap):
最大开放订单/动作 (Max open orders):
证据索引版本 (Evidence index version):
人工确认政策 (Manual confirmation policy):
真实签名政策 (Real signing policy):
熔断演练证据 (Kill switch drill evidence):
未知状态演练证据 (Unknown-state drill evidence):
对账演练证据 (Reconciliation drill evidence):
权限/签名失败演练证据 (Permission/signer failure drill evidence):
最新命令证据 (Latest command evidence):
限制 (Restrictions):
- Owner-only funds / 仅限所有者本人资金；无第三方、团队、客户或商业资金
- Personal risk acceptance only / 仅为个人风险接受，不是外部独立审查
- No withdrawals / 无提现
- No autonomous live execution / 无自动实盘
- No real signing unless the explicit owner-approved policy is in force / 没有显式 owner-approved policy 时不得真实签名
- Manual confirmation required for every order / 每笔订单必须人工确认
- Approval does not bypass risk, kill switch, ledger, reconciliation or execution permissions / 批准不绕过风控、熔断、账本、对账或执行权限
- No approval to create, weaken or treat as complete any real execution/signing implementation task / 不批准创建、削弱或视为完成任何真实执行/签名实现任务
- Stop on unknown state / 未知状态停机
- Stop on reconciliation mismatch / 对账不一致停机
- Stop on permission or signer failure / 权限或签名失败停机
- Reconcile after every live action / 每个 live action 后对账
```

中文说明：最终所有者决策不得包含真实密钥、完整账户号、私钥、助记词、token、
session、webhook secret 或原始私有余额。最终决策也不能把个人路径记录为外部审查
通过。
