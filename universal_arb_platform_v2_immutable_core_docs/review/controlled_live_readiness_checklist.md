# 受控实盘准备审查清单 / Controlled Live Readiness Checklist

范围：`S12-01`
默认状态：**外部审查完成前保持 Blocked**。

中文说明：本清单用于外部安全、风控、账本、执行、对账、回放、权限、密钥和
事故流程审查。任何条目未通过，都不能进入实盘，也不能创建真实执行实现任务。

## 个人自用替代路径 / Personal-Use Alternative

中文说明：如果系统只由所有者本人使用自己的小额封顶资金，请使用
`personal_guarded_live_governance.md`，不要把本外部审查清单标记为已通过。个人路径是
个人风险接受，不是外部独立批准，也不允许自动实盘。

需要收集个人路径证据时，先读：

- `personal_guarded_live_evidence_collection_guide.md`：逐项说明证据怎么来、如何脱敏、如何填写。
- `personal_guarded_live_evidence_index.md`：只记录脱敏证据引用，不记录秘密或原始账户资料。
- `personal_guarded_live_pilot_checklist.md`：只有全部必要项为 `Pass`，才可以讨论小额 `GuardedLivePersonal`。

## 使用方式 / How to Use

- 每一项只能标记为 `Pass`、`Fail` 或 `Not reviewed`。
- 只能附脱敏证据引用，不能写入 secret、私钥、token、助记词、webhook secret、完整账户号或原始私有余额。
- 外部审查签署必须包含审查人身份引用、日期、范围、拒绝项和仍开放项。
- 如果证据包含凭证、原始私有余额、私钥、API secret、session token、webhook token 或钱包助记词，必须拒收证据并轮换受影响凭证。

## 审查门 / Review Gates

| 门禁 | 中文要求 | 证据引用 | 状态 |
|---|---|---|---:|
| 外部安全审查 | 独立审查人确认代码、日志、fixture、报告和 issue 文本中没有真实秘密。 |  | Not reviewed |
| 外部风控审查 | 审查限额、陈旧数据、未知状态、场所健康、余额、保证金、流动性和单日亏损策略。 |  | Not reviewed |
| 外部账本审查 | 审查 live 账本命名空间、账户映射、手续费、成交、资金费、转账、冲销和调整流程。 |  | Not reviewed |
| 外部执行审查 | 审查幂等、部分成交、未知状态、取消、超时、补偿和禁用路径。 |  | Not reviewed |
| 外部对账审查 | 审查 live 余额、仓位、成交、手续费、资金费和转账对账流程。 |  | Not reviewed |
| 外部回放审查 | 外部审查人接受离线确定性回放作为实盘前安全证据之一。 |  | Not reviewed |
| 权限审查 | API key 没有提现权限，且只具备最小必要权限。 |  | Not reviewed |
| 密钥托管审查 | 真实签名保持关闭或受独立门控；托管、轮换和撤销流程已审查。 |  | Not reviewed |
| 事故流程审查 | 已完成未知状态、对账不一致、签名失败、权限失败和熔断演练。 |  | Not reviewed |

## 默认关闭控制 / Default-Off Controls

| 控制点 | 期望值 | 证据命令或文件 | 状态 |
|---|---|---|---:|
| 运行时执行模式 | `ReadOnly` | `templates/config.template.yaml` | Not reviewed |
| 实盘执行开关 | `false` | `templates/config.template.yaml` | Not reviewed |
| 自动实盘开关 | `false` | `templates/config.template.yaml` | Not reviewed |
| 真实签名开关 | `false` | `templates/config.template.yaml` | Not reviewed |
| 场所启用状态 | 批准前为 `false` | `universal_arb_platform_v2_immutable_core_docs/templates/config.template.yaml` | Not reviewed |
| 交易权限 | 批准前为 `false` | 脱敏场所权限导出 | Not reviewed |
| 提现权限 | 交易 key 永远为 `false` | 脱敏场所权限导出 | Not reviewed |
| Cargo 默认 features | 为空或仅安全 feature | `Cargo.toml`, crate `Cargo.toml` files | Not reviewed |
| 实盘执行 feature | 只能显式 opt-in | `crates/arb-venue-exec/Cargo.toml` | Not reviewed |
| 真实签名 feature | 只能显式 opt-in | `crates/arb-signing/Cargo.toml` | Not reviewed |

## 熔断演练 / Kill Switch Drill

| 维度 | 必要证明 | 状态 |
|---|---|---:|
| 全局 | 阻断全部账户变更执行，并产生审计证据。 | Not reviewed |
| 执行分发 | 即使风控决策批准，也能阻断分发。 | Not reviewed |
| 策略 | 只阻断受影响策略，并记录范围。 | Not reviewed |
| 场所 | 只阻断受影响场所，并记录范围。 | Not reviewed |
| 账户 | 只阻断受影响账户，并记录范围。 | Not reviewed |
| 工具或合约 | 只阻断受影响工具或合约，并记录范围。 | Not reviewed |
| 资产 | 只阻断受影响资产，并记录范围。 | Not reviewed |
| 链 | 只阻断受影响链，并记录范围。 | Not reviewed |
| 执行模式 | 配置后能阻断 `GuardedLive` 或更强模式。 | Not reviewed |

## 运营演练 / Operational Drill

| 演练 | 通过条件 | 状态 |
|---|---|---:|
| 端到端模拟回放 | 回放 artifact 与预期输出一致，且没有外部 API 调用。 | Not reviewed |
| 未知状态 | 未知场所或执行状态会生成事故，并暂停受影响范围。 | Not reviewed |
| 对账不一致 | 差异会生成可追踪事故，并阻断受影响范围。 | Not reviewed |
| 权限失败 | 权限缺失或不足时 fail closed，且不会分发执行。 | Not reviewed |
| 签名失败 | 签名不可用、被禁用、被拒绝或策略不匹配时，不能视为成功。 | Not reviewed |
| 事故响应 | 演练 owner、时间线、缓解动作、客户或内部沟通和复盘路径。 | Not reviewed |

## 最终批准要求 / Required Final Approval

创建任何真实执行实现任务前，下面所有条件必须同时满足：

- 外部安全、风控、账本、执行、对账、回放和事故流程审查全部完成。
- 所有实盘能力默认关闭。
- API key 没有提现权限。
- 热钱包或等价 live 资金严格封顶，并经过独立审查。
- 每个必需维度都有熔断覆盖证据。
- 每个 live action 都可追踪到事件、风控决策、执行计划、执行报告、账本分录、对账结果和事故路径。
- 事故响应已经用脱敏证据演练。
- 最终审查人显式批准创建真实执行实现任务。

中文结论：任一条件缺失时，结论必须保持 `Blocked`。
