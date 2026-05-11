# 个人小额受控试运行证据索引 / Personal Guarded Live Evidence Index

范围：仅限系统所有者本人使用自己的小额封顶资金。
状态：证据地图草案，当前不代表准入通过。

中文说明：本索引用于把个人小额受控试运行所需证据映射到仓库文件、命令输出或
所有者提供的脱敏证据。不得在本文件写入真实 API secret、私钥、助记词、token、
完整账户号、原始私有余额或可复用签名材料。

填写前先读：

- `personal_guarded_live_evidence_collection_guide.md`：逐项说明证据怎么收集、怎么脱敏、怎么填写。
- `personal_guarded_live_pilot_checklist.md`：说明哪些证据门必须全部通过。

状态值说明：

- `Present`：仓库内文件或 fixture 存在。
- `Needs latest run`：需要在 pilot 前重新运行命令。
- `Missing`：证据缺失。
- `Pass`：已用脱敏引用证明通过。
- `Fail`：证据证明该项不满足要求。
- `Not run`：命令尚未运行。

## 仓库内证据 / Repository Evidence

中文说明：这些证据来自仓库文件、配置、fixture 或离线命令，不需要真实交易账户。
命令证据必须在 pilot 前重新运行，不能用旧结果替代。

| 证据 ID | 控制点中文说明 | 证据路径或命令 | 当前状态 |
|---|---|---|---:|
| `repo-governance-profile` | 个人小额受控治理画像存在，并说明这不是外部审计。 | `universal_arb_platform_v2_immutable_core_docs/review/personal_guarded_live_governance.md` | Present |
| `repo-pilot-checklist` | 个人试运行准入清单存在，且默认保持阻塞。 | `universal_arb_platform_v2_immutable_core_docs/review/personal_guarded_live_pilot_checklist.md` | Present |
| `repo-evidence-collection-guide` | 证据收集教程存在，说明如何收集、脱敏和填写每个证据。 | `universal_arb_platform_v2_immutable_core_docs/review/personal_guarded_live_evidence_collection_guide.md` | Present |
| `repo-checklist-audit` | 清单审计报告存在，并记录剩余阻塞项。 | `universal_arb_platform_v2_immutable_core_docs/review/personal_guarded_live_checklist_audit_report.md` | Present |
| `repo-controlled-review` | 受控实盘评审材料存在，并区分个人路径和外部审查路径。 | `universal_arb_platform_v2_immutable_core_docs/review/controlled_live_readiness_review.md` | Present |
| `repo-default-config` | 运行时配置模板默认关闭实盘。 | `templates/config.template.yaml` | Present |
| `repo-canonical-config` | 文档侧权威配置模板默认关闭场所交易，并示例 `can_withdraw: false`。 | `universal_arb_platform_v2_immutable_core_docs/templates/config.template.yaml` | Present |
| `repo-live-feature-gate` | 真实执行功能是显式 opt-in，默认 features 为空或安全。 | `crates/arb-venue-exec/Cargo.toml` | Present |
| `repo-signing-feature-gate` | 真实签名功能是显式 opt-in，默认 features 为空或安全。 | `crates/arb-signing/Cargo.toml` | Present |
| `repo-config-tests` | 配置测试覆盖默认 live-off 和 kill switch 阻断。 | `cargo test -p arb-config` 或 `cargo test --workspace` | Needs latest run |
| `repo-runtime-tests` | 运行时测试覆盖 kill switch 阻断和回放。 | `cargo test -p arb-runtime` 或 `cargo test --workspace` | Needs latest run |
| `repo-signing-tests` | 签名测试覆盖默认无真实签名和脱敏。 | `cargo test -p arb-signing` 或 `cargo test --workspace` | Needs latest run |
| `repo-venue-exec-tests` | 可变执行测试覆盖默认无 live exec 和幂等。 | `cargo test -p arb-venue-exec` 或 `cargo test --workspace` | Needs latest run |
| `repo-ops-redaction-tests` | 运营报告测试覆盖敏感信息脱敏。 | `cargo test -p arb-ops` 或 `cargo test --workspace` | Needs latest run |
| `repo-boundary-check` | 禁止依赖和 live feature 依赖边界被检查。 | `cargo xtask check-crate-boundaries` | Needs latest run |
| `repo-schema-check` | schema 和 schema fixture 可解析。 | `cargo xtask check-schema` | Needs latest run |
| `repo-doc-check` | 必读文档和中文说明存在。 | `cargo xtask check-docs` | Needs latest run |
| `repo-full-replay` | 完整模拟链路回放匹配预期 artifact。 | `cargo xtask replay-full-pipeline` | Needs latest run |
| `repo-reconciliation-fixtures` | 对账一致和不一致 fixture 存在。 | `fixtures/replay/reconciliation_match/`, `fixtures/replay/reconciliation_mismatch/` | Present |
| `repo-unknown-state-fixtures` | 未知状态事故 fixture 存在。 | `fixtures/replay/incident_unknown_state/`, `fixtures/replay/full_pipeline_simulated/cases/unknown_state/` | Present |
| `repo-manual-approval-fixtures` | 人工审批通过/拒绝 fixture 存在。 | `fixtures/replay/manual_approval_approved/`, `fixtures/replay/manual_approval_rejected/` | Present |

## 所有者脱敏证据 / Owner-Supplied Redacted Evidence

中文说明：这些证据来自交易所、钱包、运行环境或所有者本人的决策记录。只保存
脱敏引用，不保存秘密或完整账户细节。实际填写方法见
`personal_guarded_live_evidence_collection_guide.md`。

| 证据 ID | 控制点中文说明 | 可接受的脱敏证据 | 当前状态 |
|---|---|---|---:|
| `owner-pilot-ownership` | 证明 pilot 只使用所有者本人资金，不涉及第三方、团队、客户或商业资金。 | 带日期的所有者决策记录；不含私人账户细节。 | Missing |
| `owner-risk-acceptance` | 所有者接受 pilot 资金可能全部损失。 | 带日期的风险接受记录，包含资金上限和停止条件。 | Missing |
| `owner-scope` | 明确场所、策略、账户、工具和最长持续时间范围。 | 脱敏本地决策记录。 | Missing |
| `owner-isolated-account` | 证明使用隔离子账户、钱包或托管 bucket。 | 脱敏账户标签或托管引用。 | Missing |
| `owner-no-withdrawal-permission` | 证明交易 key 没有提现或转出权限。 | 脱敏权限截图或导出。 | Missing |
| `owner-minimum-permission` | 证明 key 只有 pilot 所需的 read/trade 权限。 | 脱敏权限截图或导出。 | Missing |
| `owner-ip-device-restriction` | 证明交易所支持时已启用 IP/device 限制。 | 脱敏交易所设置引用。 | Missing |
| `owner-key-rotation` | 证明所有者可不改代码撤销并轮换 key。 | key 轮换 runbook 引用。 | Missing |
| `owner-capital-cap` | 证明隔离账户只放入封顶 pilot 资金。 | 脱敏上限声明；不记录原始私有余额。 | Missing |
| `owner-per-order-cap` | 定义单笔订单或单个动作上限。 | 脱敏 owner policy 或配置引用。 | Missing |
| `owner-daily-loss-cap` | 定义单日亏损停止阈值。 | 脱敏 owner policy 或配置引用。 | Missing |
| `owner-max-open-orders` | 定义最大活跃订单/未完成动作数量。 | 脱敏 owner policy 或配置引用。 | Missing |
| `owner-manual-confirmation` | 证明每笔订单都需要所有者确认。 | runbook 或配置引用。 | Missing |
| `owner-real-signing-policy` | 证明真实签名默认关闭，或有单独 owner-approved policy 和 fail-closed 控制。 | 脱敏签名策略引用；不含签名材料。 | Missing |
| `owner-kill-switch-drill` | 完成全局、执行分发、执行模式、场所、策略、账户、工具、资产、链的熔断演练。 | 本地 tabletop 记录，包含阻断范围、时间戳和审计/事件引用。 | Missing |
| `owner-unknown-state-drill` | 完成未知状态停机演练。 | 事故或演练记录引用。 | Missing |
| `owner-reconciliation-drill` | 完成对账不一致阻断演练。 | 事故或演练记录引用。 | Missing |
| `owner-permission-failure-drill` | 完成权限缺失或权限不足 fail-closed 演练。 | 事故/演练记录引用和脱敏权限证据。 | Missing |
| `owner-signer-failure-drill` | 完成签名器禁用、不可用、拒绝或策略不匹配 fail-closed 演练。 | 事故/演练记录引用；不含签名材料。 | Missing |
| `owner-post-action-reconciliation` | 证明每个 live action 后有执行报告、账本、场所余额、仓位和成交对账流程。 | runbook 或清单引用。 | Missing |
| `owner-incident-note-template` | 证明所有者能记录 mismatch、unknown state、permission failure 或 signer failure 事故。 | 事故记录模板引用。 | Missing |
| `owner-daily-review` | 证明每次 pilot session 后所有者会检查运营报告。 | 带日期的运营复盘清单或报告引用。 | Missing |
| `owner-final-decision` | 最终 approve/reject 决策已记录。 | 完成后的最终所有者决策记录。 | Missing |

## 最近命令证据 / Latest Command Evidence

中文说明：这些命令必须在 pilot 前立即运行。不要粘贴大段日志；只记录命令、日期、
通过/失败和简短摘要。

| 命令 | 日期 | 结果 | 备注 |
|---|---|---:|---|
| `cargo fmt --all -- --check` |  | Not run |  |
| `cargo clippy --workspace --all-targets -- -D warnings` |  | Not run |  |
| `cargo test --workspace` |  | Not run |  |
| `cargo xtask check-schema` |  | Not run |  |
| `cargo xtask check-crate-boundaries` |  | Not run |  |
| `cargo xtask check-docs` |  | Not run |  |
| `cargo xtask replay-full-pipeline` |  | Not run |  |

## 证据处理规则 / Evidence Handling Rules

- 只保存脱敏引用。
- 不粘贴含有秘密或完整账户标识的原始截图。
- 不粘贴 API key、API secret、私钥、助记词、session token、webhook token 或 signer payload。
- 不粘贴原始私有余额；只记录资金上限和 owner 风险接受。
- 如果意外捕获了秘密材料，应先轮换受影响凭证，再替换为脱敏引用。
- AI 可以帮助更新证据索引，但不能代替所有者批准 live trading。
