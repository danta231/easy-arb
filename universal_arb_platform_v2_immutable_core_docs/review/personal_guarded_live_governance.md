# 个人小额受控试运行治理 / Personal Guarded Live Governance

范围：仅限所有者本人操作、本人资金、小额封顶试运行。
状态：可选个人治理画像，不是外部审计。

中文说明：本文只适用于“系统所有者本人使用自己的小额资金”的场景。它不是外部
审计，也不是自动实盘批准。若系统管理他人资金、提供给他人使用、形成团队或商业
服务，必须回到 `controlled_live_readiness_checklist.md` 的外部审查路径。

## 定位 / Position

个人小额受控试运行是所有者本人承担风险的小额试运行。它允许在严格约束下准备或
验证受控实盘流程，但不能放宽默认安全门，也不能直接进入自动实盘。

允许的事情：

- 读取公开行情、个人只读余额、仓位和成交。
- 跑模拟执行和离线回放。
- 生成执行计划预览。
- 所有者对每一笔订单做人工确认。
- 在所有清单和证据门都通过后，讨论小额 `GuardedLivePersonal`。

不允许的事情：

- 自动实盘。
- 交易 API key 拥有提现或转出权限。
- 没有单独签名策略和所有者批准就启用真实签名。
- 隔离账户持有超过所有者愿意损失的资金。
- 把真实个人 secret 写进代码、日志、fixture、报告或提示词。
- 把 AI 辅助审查记录成外部独立审查。

## 最低准入门 / Required Minimum Gates

任何个人小额受控试运行前，下面全部门禁必须通过。

操作文档和证据索引：

- `personal_guarded_live_pilot_checklist.md`：记录每个准入门的 `Pass`、`Fail` 或 `Not reviewed`。
- `personal_guarded_live_evidence_index.md`：映射仓库证据和所有者脱敏证据。
- `personal_guarded_live_evidence_collection_guide.md`：说明每个证据怎么收集、脱敏和填写。
- `personal_guarded_live_checklist_audit_report.md`：记录 AI 辅助清单审计和剩余阻塞。

中文说明：实际执行个人小额受控试运行前，应先完成个人试运行清单和证据索引；缺失项
不能用口头确认、AI 判断或本审计报告替代。

| 门禁 | 最低要求 | 证据 |
|---|---|---|
| Owner-only scope / 所有者本人范围 | Pilot 排除第三方、团队、客户和商业资金。 | 带日期的所有者决策记录。 |
| Owner risk acceptance / 所有者风险接受 | 所有者记录全部 pilot 资金可能损失。 | 本地签署说明或带日期 owner decision；不含私钥或余额。 |
| Isolated account / 隔离账户 | Pilot 使用独立交易所子账户或独立钱包。 | 脱敏账户标签或托管引用。 |
| No withdrawal permission / 无提现权限 | API key 不能提现或转出。 | 脱敏权限截图或导出。 |
| Small capital cap / 小额资金上限 | 账户只持有 pilot 资金。 | 脱敏上限说明；不要求原始私有余额。 |
| Manual confirmation / 每笔人工确认 | 每笔订单分发前都需要所有者确认。 | runbook 步骤或配置证据。 |
| Live default-off / 实盘默认关闭 | `live_execution_enabled=false`、`auto_live_enabled=false`、真实签名默认关闭。 | `templates/config.template.yaml` 和运行时配置。 |
| Per-order limit / 单笔上限 | 配置单个动作名义金额上限。 | 脱敏配置或 owner policy。 |
| Daily loss limit / 单日亏损上限 | 配置单日停止阈值。 | 脱敏配置或 owner policy。 |
| Max open orders / 最大开放动作 | 配置最大活跃订单或未完成动作数量。 | 脱敏配置或 owner policy。 |
| Kill switch / 熔断 | 所有者可停止全局、执行分发、执行模式、场所、策略、账户、工具、资产和链范围。 | 本地 tabletop 演练记录。 |
| Unknown-state stop / 未知状态停机 | 未知执行或场所状态会停止受影响范围。 | 回放或演练证据。 |
| Post-trade reconciliation / 动作后对账 | 每个 live action 必须在下一轮 live cycle 前完成对账。 | 清单或报告证据。 |
| Permission and signer failure stop / 权限和签名失败停机 | 权限缺失或签名失败不能当作成功。 | 演练或事故证据。 |
| Incident note / 事故记录 | mismatch、unknown state、permission failure 或 signer failure 都有事故记录。 | 事故记录引用。 |

中文说明：以上只是最低门槛，不是完整安全审计。任何一项缺失，都应停留在只读、
模拟或人工手动交易模式。

## 个人试运行模式 / Personal Pilot Modes

| 模式 | 中文含义 | 是否有系统发起的账户变更 |
|---|---|---:|
| `ReadOnlyPersonal` | 只读个人账户和行情。 | No |
| `ManualExecutionPersonal` | 系统生成建议，所有者手动去交易所操作。 | No system action |
| `GuardedLivePersonal` | 小额、人工确认、无提现、强制对账的系统动作。 | Yes, tightly capped |
| `AutonomousLivePersonal` | 自动实盘。 | Not allowed by this profile |

中文说明：推荐长期停留在 `ManualExecutionPersonal`。只有当上方最低门全部完成时，
才可以讨论 `GuardedLivePersonal`，并且每个动作仍需人工确认。

## AI 辅助自审 / AI-Assisted Review

Codex 或其他 AI 工具可以帮助：

- 整理证据索引。
- 更新清单。
- 跑静态检查。
- 跑离线回放检查。
- 检查脱敏风险。
- 做 failure-mode review。
- 起草 owner runbook 和事故模板。

AI 工具不能：

- 接收真实 API secret、私钥、session token 或钱包助记词。
- 被记录为外部独立审查人。
- 自行批准 live trading。
- 覆盖 kill switch、对账或权限失败。

## 个人批准记录模板 / Personal Approval Record

```text
决定 (Decision): Personal guarded live pilot approved / rejected
所有者 (Owner): <local owner reference>
日期 (Date):
范围 (Scope):
资金上限 (Capital cap):
单笔上限 (Per-order cap):
单日亏损上限 (Daily loss cap):
API 权限证据 (API permission evidence): <redacted reference>
熔断演练证据 (Kill switch drill evidence): <reference>
对账演练证据 (Reconciliation drill evidence): <reference>
未知状态演练证据 (Unknown-state drill evidence): <reference>
限制 (Restrictions):
- No withdrawals / 无提现
- No autonomous live execution / 无自动实盘
- Manual confirmation required for every order / 每笔订单必须人工确认
- Stop on unknown state or reconciliation mismatch / 未知状态或对账不一致时停止
```

中文说明：批准记录不得包含真实密钥、完整账户号、私钥、助记词、token 或原始私有
余额。需要证明额度时，使用脱敏截图、hash 引用或文字上限说明。
