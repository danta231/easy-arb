# 个人小额受控试运行证据索引 / Personal Guarded Live Evidence Index

范围：仅限系统所有者本人使用自己的小额封顶资金。
状态：仓库内文件、fixture 与离线命令证据已于 2026-05-12 更新；所有者自我约束类 `owner-note` 已补；熔断、未知状态、对账不一致、权限失败和签名失败离线演练已记录；最终 owner decision 已按小额受控范围批准 `GuardedLivePersonal`，不代表外部审查通过，也不批准自动实盘。

中文说明：本索引用于把个人小额受控试运行所需证据映射到仓库文件、命令输出或
所有者提供的脱敏证据。不得在本文件写入真实 API secret、私钥、助记词、token、
完整账户号、原始私有余额或可复用签名材料。

推荐入口：

- 日常从 `personal_guarded_live_pilot_checklist.md` 开始；它是最终阻断/放行视图。
- 本文件是证据台账，用来解释 checklist 每个 evidence ID 的来源、日期和结论。
- 不知道某项 owner 证据怎么收集或脱敏时，再查 `personal_guarded_live_evidence_collection_guide.md`。

中文说明：不需要每次先读 guide 再读本文件。只有新增脱敏引用、重新跑命令或证据
结论变化时，才更新本证据索引。

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

### evidence-repo-governance-profile

证据 ID：[`repo-governance-profile`](personal_guarded_live_evidence_collection_guide.md#guide-repo-governance-profile)

| 控制点中文说明 | 证据来源或命令 | 当前状态 |
|---|---|---:|
| 个人小额受控治理画像存在，并说明这不是外部审计。 | `universal_arb_platform_v2_immutable_core_docs/review/personal_guarded_live_governance.md` | Present: inspected 2026-05-12; states personal path is not external audit and does not approve auto-live |

### evidence-repo-pilot-checklist

证据 ID：[`repo-pilot-checklist`](personal_guarded_live_evidence_collection_guide.md#guide-repo-pilot-checklist)

| 控制点中文说明 | 证据来源或命令 | 当前状态 |
|---|---|---:|
| 个人试运行准入清单存在，且默认保持阻塞。 | `universal_arb_platform_v2_immutable_core_docs/review/personal_guarded_live_pilot_checklist.md` | Present: inspected 2026-05-12; default remains Blocked until all gates pass |

### evidence-repo-evidence-collection-guide

证据 ID：[`repo-evidence-collection-guide`](personal_guarded_live_evidence_collection_guide.md#guide-repo-evidence-collection-guide)

| 控制点中文说明 | 证据来源或命令 | 当前状态 |
|---|---|---:|
| 证据收集教程存在，说明如何收集、脱敏和填写每个证据。 | `universal_arb_platform_v2_immutable_core_docs/review/personal_guarded_live_evidence_collection_guide.md` | Present: inspected 2026-05-12; defines redacted-reference workflow and safe offline commands |

### evidence-repo-checklist-audit

证据 ID：[`repo-checklist-audit`](personal_guarded_live_evidence_collection_guide.md#guide-repo-checklist-audit)

| 控制点中文说明 | 证据来源或命令 | 当前状态 |
|---|---|---:|
| 清单审计报告存在，并记录剩余阻塞项。 | `universal_arb_platform_v2_immutable_core_docs/review/personal_guarded_live_checklist_audit_report.md` | Present: inspected 2026-05-12; records owner evidence as remaining blocker |

### evidence-repo-controlled-review

证据 ID：[`repo-controlled-review`](personal_guarded_live_evidence_collection_guide.md#guide-repo-controlled-review)

| 控制点中文说明 | 证据来源或命令 | 当前状态 |
|---|---|---:|
| 受控实盘评审材料存在，并区分个人路径和外部审查路径。 | `universal_arb_platform_v2_immutable_core_docs/review/controlled_live_readiness_review.md` | Present: inspected 2026-05-12; distinguishes personal path from external review and rejects real implementation tasks |

### evidence-repo-default-config

证据 ID：[`repo-default-config`](personal_guarded_live_evidence_collection_guide.md#guide-repo-default-config)

| 控制点中文说明 | 证据来源或命令 | 当前状态 |
|---|---|---:|
| 运行时配置模板默认关闭实盘。 | `templates/config.template.yaml` | Present: inspected 2026-05-12; mode ReadOnly, live_execution=false, auto_live=false, real_signing=false |

### evidence-repo-canonical-config

证据 ID：[`repo-canonical-config`](personal_guarded_live_evidence_collection_guide.md#guide-repo-canonical-config)

| 控制点中文说明 | 证据来源或命令 | 当前状态 |
|---|---|---:|
| 文档侧权威配置模板默认关闭场所交易，并示例 `can_withdraw: false`。 | `universal_arb_platform_v2_immutable_core_docs/templates/config.template.yaml` | Present: inspected 2026-05-12; execution_mode ReadOnly, venue disabled, can_trade=false, can_withdraw=false |

### evidence-repo-live-feature-gate

证据 ID：[`repo-live-feature-gate`](personal_guarded_live_evidence_collection_guide.md#guide-repo-live-feature-gate)

| 控制点中文说明 | 证据来源或命令 | 当前状态 |
|---|---|---:|
| 真实执行功能是显式 opt-in，默认 features 为空或安全。 | `crates/arb-venue-exec/Cargo.toml` | Present: inspected 2026-05-12; default=[], live-exec=[] |

### evidence-repo-signing-feature-gate

证据 ID：[`repo-signing-feature-gate`](personal_guarded_live_evidence_collection_guide.md#guide-repo-signing-feature-gate)

| 控制点中文说明 | 证据来源或命令 | 当前状态 |
|---|---|---:|
| 真实签名功能是显式 opt-in，默认 features 为空或安全。 | `crates/arb-signing/Cargo.toml` | Present: inspected 2026-05-12; default=[], real-signing=[] |

### evidence-repo-config-tests

证据 ID：[`repo-config-tests`](personal_guarded_live_evidence_collection_guide.md#guide-repo-config-tests)

| 控制点中文说明 | 证据来源或命令 | 当前状态 |
|---|---|---:|
| 配置测试覆盖默认 live-off 和 kill switch 阻断。 | `cargo test -p arb-config` 或 `cargo test --workspace` | Pass: cmd:2026-05-12-cargo-test-workspace-v2, cmd:2026-05-12-config-kill-switch-live-block-v2; config live-off and all scoped kill switch predicates passed |

### evidence-repo-runtime-tests

证据 ID：[`repo-runtime-tests`](personal_guarded_live_evidence_collection_guide.md#guide-repo-runtime-tests)

| 控制点中文说明 | 证据来源或命令 | 当前状态 |
|---|---|---:|
| 运行时测试覆盖 kill switch 阻断和回放。 | `cargo test -p arb-runtime` 或 `cargo test --workspace` | Pass: cmd:2026-05-12-cargo-test-workspace-v2; runtime kill switch and full-pipeline fixture tests passed |

### evidence-repo-signing-tests

证据 ID：[`repo-signing-tests`](personal_guarded_live_evidence_collection_guide.md#guide-repo-signing-tests)

| 控制点中文说明 | 证据来源或命令 | 当前状态 |
|---|---|---:|
| 签名测试覆盖默认无真实签名和脱敏。 | `cargo test -p arb-signing` 或 `cargo test --workspace` | Pass: cmd:2026-05-12-cargo-test-workspace-v2; default-deny signing and redaction tests passed |

### evidence-repo-venue-exec-tests

证据 ID：[`repo-venue-exec-tests`](personal_guarded_live_evidence_collection_guide.md#guide-repo-venue-exec-tests)

| 控制点中文说明 | 证据来源或命令 | 当前状态 |
|---|---|---:|
| 可变执行测试覆盖默认无 live exec 和幂等。 | `cargo test -p arb-venue-exec` 或 `cargo test --workspace` | Pass: cmd:2026-05-12-cargo-test-workspace-v2; default no live exec and idempotency tests passed |

### evidence-repo-ops-redaction-tests

证据 ID：[`repo-ops-redaction-tests`](personal_guarded_live_evidence_collection_guide.md#guide-repo-ops-redaction-tests)

| 控制点中文说明 | 证据来源或命令 | 当前状态 |
|---|---|---:|
| 运营报告测试覆盖敏感信息脱敏。 | `cargo test -p arb-ops` 或 `cargo test --workspace` | Pass: cmd:2026-05-12-cargo-test-workspace-v2; ops redaction tests passed |

### evidence-repo-boundary-check

证据 ID：[`repo-boundary-check`](personal_guarded_live_evidence_collection_guide.md#guide-repo-boundary-check)

| 控制点中文说明 | 证据来源或命令 | 当前状态 |
|---|---|---:|
| 禁止依赖和 live feature 依赖边界被检查。 | `cargo xtask check-crate-boundaries` | Pass: cmd:2026-05-12-check-crate-boundaries-v2; checked 26 forbidden dependency rules |

### evidence-repo-schema-check

证据 ID：[`repo-schema-check`](personal_guarded_live_evidence_collection_guide.md#guide-repo-schema-check)

| 控制点中文说明 | 证据来源或命令 | 当前状态 |
|---|---|---:|
| schema 和 schema fixture 可解析。 | `cargo xtask check-schema` | Pass: cmd:2026-05-12-check-schema-v2; parsed 13 schema files, 13 valid fixtures, 13 invalid fixtures |

### evidence-repo-doc-check

证据 ID：[`repo-doc-check`](personal_guarded_live_evidence_collection_guide.md#guide-repo-doc-check)

| 控制点中文说明 | 证据来源或命令 | 当前状态 |
|---|---|---:|
| 必读文档和中文说明存在。 | `cargo xtask check-docs` | Pass: cmd:2026-05-12-check-docs-v2; checked 4 required docs and 3 fixture notes |

### evidence-repo-full-replay

证据 ID：[`repo-full-replay`](personal_guarded_live_evidence_collection_guide.md#guide-repo-full-replay)

| 控制点中文说明 | 证据来源或命令 | 当前状态 |
|---|---|---:|
| 完整模拟链路回放匹配预期 artifact。 | `cargo xtask replay-full-pipeline` | Pass: cmd:2026-05-12-replay-full-pipeline-v2; matched 10 S9-01 artifacts offline |

### evidence-repo-reconciliation-fixtures

证据 ID：[`repo-reconciliation-fixtures`](personal_guarded_live_evidence_collection_guide.md#guide-repo-reconciliation-fixtures)

| 控制点中文说明 | 证据来源或命令 | 当前状态 |
|---|---|---:|
| 对账一致和不一致 fixture 存在。 | `fixtures/replay/reconciliation_match/`, `fixtures/replay/reconciliation_mismatch/` | Present: inspected 2026-05-12; both match and mismatch fixture directories exist |

### evidence-repo-unknown-state-fixtures

证据 ID：[`repo-unknown-state-fixtures`](personal_guarded_live_evidence_collection_guide.md#guide-repo-unknown-state-fixtures)

| 控制点中文说明 | 证据来源或命令 | 当前状态 |
|---|---|---:|
| 未知状态事故 fixture 存在。 | `fixtures/replay/incident_unknown_state/`, `fixtures/replay/full_pipeline_simulated/cases/unknown_state/` | Present: inspected 2026-05-12; incident and full-pipeline unknown-state fixtures exist |

### evidence-repo-manual-approval-fixtures

证据 ID：[`repo-manual-approval-fixtures`](personal_guarded_live_evidence_collection_guide.md#guide-repo-manual-approval-fixtures)

| 控制点中文说明 | 证据来源或命令 | 当前状态 |
|---|---|---:|
| 人工审批通过/拒绝 fixture 存在。 | `fixtures/replay/manual_approval_approved/`, `fixtures/replay/manual_approval_rejected/` | Present: inspected 2026-05-12; approved and rejected manual-approval fixtures exist |


## 所有者脱敏证据 / Owner-Supplied Redacted Evidence

中文说明：这些证据来自交易所、钱包、运行环境或所有者本人的决策记录。只保存
脱敏引用，不保存秘密或完整账户细节。实际填写方法见
`personal_guarded_live_evidence_collection_guide.md`。

### evidence-owner-pilot-ownership

证据 ID：[`owner-pilot-ownership`](personal_guarded_live_evidence_collection_guide.md#guide-owner-pilot-ownership)

| 控制点中文说明 | 证据来源或命令 | 当前状态 |
|---|---|---:|
| 证明 pilot 只使用所有者本人资金，不涉及第三方、团队、客户或商业资金。 | 带日期的所有者决策记录；不含私人账户细节。 | Pass: owner-note:2026-05-12-pilot-ownership-v1; owner stated this is a personal-use project using owner-only funds, with no third-party, team, customer, or commercial funds; this does not claim external audit approval or approve autonomous live execution |

### evidence-owner-risk-acceptance

证据 ID：[`owner-risk-acceptance`](personal_guarded_live_evidence_collection_guide.md#guide-owner-risk-acceptance)

| 控制点中文说明 | 证据来源或命令 | 当前状态 |
|---|---|---:|
| 所有者接受 pilot 资金可能全部损失。 | 带日期的风险接受记录，包含资金上限和停止条件。 | Pass: owner-note:2026-05-12-zero-live-risk-policy-v1; owner accepts that any explicitly approved future pilot capital may be fully lost; current GuardedLivePersonal capital cap remains 0 until a separate cap, scope, and final decision are recorded; stop criteria include any unauthorized live dispatch, any nonzero live loss under the current zero cap, unknown state, reconciliation mismatch, error burst, permission or signer failure, and owner concern |

### evidence-owner-scope

证据 ID：[`owner-scope`](personal_guarded_live_evidence_collection_guide.md#guide-owner-scope)

| 控制点中文说明 | 证据来源或命令 | 当前状态 |
|---|---|---:|
| 明确场所、策略、账户、工具和最长持续时间范围。 | 脱敏本地决策记录。 | Pass: owner-note:2026-05-12-zero-live-scope-v1; current GuardedLivePersonal scope is none: no venue, strategy, account, custody bucket, instrument, asset, chain, or live duration is approved; only ReadOnlyPersonal, ManualExecutionPersonal, and simulated modes remain in scope |

### evidence-owner-isolated-account

证据 ID：[`owner-isolated-account`](personal_guarded_live_evidence_collection_guide.md#guide-owner-isolated-account)

| 控制点中文说明 | 证据来源或命令 | 当前状态 |
|---|---|---:|
| 证明使用隔离子账户、钱包或托管 bucket。 | 脱敏账户标签或托管引用。 | Pass: owner-note:2026-05-12-zero-live-account-permission-v1; current GuardedLivePersonal account/custody scope is none, so no live venue account, wallet, or custody bucket is approved; any future nonzero live scope must replace this with a redacted isolated-account reference |

### evidence-owner-no-withdrawal-permission

证据 ID：[`owner-no-withdrawal-permission`](personal_guarded_live_evidence_collection_guide.md#guide-owner-no-withdrawal-permission)

| 控制点中文说明 | 证据来源或命令 | 当前状态 |
|---|---|---:|
| 证明交易 key 没有提现或转出权限。 | 脱敏权限截图或导出。 | Pass: owner-note:2026-05-12-zero-live-account-permission-v1; no live API key is approved in the current scope, so no key can withdraw or transfer out; any future live key must provide redacted evidence that withdrawal and transfer-out are disabled |

### evidence-owner-minimum-permission

证据 ID：[`owner-minimum-permission`](personal_guarded_live_evidence_collection_guide.md#guide-owner-minimum-permission)

| 控制点中文说明 | 证据来源或命令 | 当前状态 |
|---|---|---:|
| 证明 key 只有 pilot 所需的 read/trade 权限。 | 脱敏权限截图或导出。 | Pass: owner-note:2026-05-12-zero-live-account-permission-v1; no live API key is approved in the current scope, so no extra live permissions exist; any future live key must provide redacted minimum-permission evidence |

### evidence-owner-ip-device-restriction

证据 ID：[`owner-ip-device-restriction`](personal_guarded_live_evidence_collection_guide.md#guide-owner-ip-device-restriction)

| 控制点中文说明 | 证据来源或命令 | 当前状态 |
|---|---|---:|
| 证明交易所支持时已启用 IP/device 限制。 | 脱敏交易所设置引用。 | Pass: owner-note:2026-05-12-zero-live-account-permission-v1; no live API key is approved in the current scope, so no IP/device-restricted key exists; any future live key must document venue support and either enabled IP/device restriction or an explicit compensating control |

### evidence-owner-key-rotation

证据 ID：[`owner-key-rotation`](personal_guarded_live_evidence_collection_guide.md#guide-owner-key-rotation)

| 控制点中文说明 | 证据来源或命令 | 当前状态 |
|---|---|---:|
| 证明所有者可不改代码撤销并轮换 key。 | key 轮换 runbook 引用。 | Pass: owner-note:2026-05-12-key-rotation-path-v1; no live key is approved in the current scope; any future venue key must be revoked or rotated in the venue/key-manager console and referenced through environment or secret configuration without code changes |

### evidence-owner-capital-cap

证据 ID：[`owner-capital-cap`](personal_guarded_live_evidence_collection_guide.md#guide-owner-capital-cap)

| 控制点中文说明 | 证据来源或命令 | 当前状态 |
|---|---|---:|
| 证明隔离账户只放入封顶 pilot 资金。 | 脱敏上限声明；不记录原始私有余额。 | Pass: owner-note:2026-05-12-zero-live-risk-policy-v1; current GuardedLivePersonal capital cap is 0 until a separate owner-approved cap is recorded; no raw private balances are stored |

### evidence-owner-per-order-cap

证据 ID：[`owner-per-order-cap`](personal_guarded_live_evidence_collection_guide.md#guide-owner-per-order-cap)

| 控制点中文说明 | 证据来源或命令 | 当前状态 |
|---|---|---:|
| 定义单笔订单或单个动作上限。 | 脱敏 owner policy 或配置引用。 | Pass: owner-note:2026-05-12-zero-live-risk-policy-v1; current per-order and per-action cap is 0 until a separate owner-approved cap is recorded |

### evidence-owner-daily-loss-cap

证据 ID：[`owner-daily-loss-cap`](personal_guarded_live_evidence_collection_guide.md#guide-owner-daily-loss-cap)

| 控制点中文说明 | 证据来源或命令 | 当前状态 |
|---|---|---:|
| 定义单日亏损停止阈值。 | 脱敏 owner policy 或配置引用。 | Pass: owner-note:2026-05-12-zero-live-risk-policy-v1; current daily loss stop threshold is 0 for GuardedLivePersonal because no live capital is approved |

### evidence-owner-max-open-orders

证据 ID：[`owner-max-open-orders`](personal_guarded_live_evidence_collection_guide.md#guide-owner-max-open-orders)

| 控制点中文说明 | 证据来源或命令 | 当前状态 |
|---|---|---:|
| 定义最大活跃订单/未完成动作数量。 | 脱敏 owner policy 或配置引用。 | Pass: owner-note:2026-05-12-zero-live-risk-policy-v1; current maximum open orders and account-changing actions is 0 until a separate owner-approved scope is recorded |

### evidence-owner-manual-confirmation

证据 ID：[`owner-manual-confirmation`](personal_guarded_live_evidence_collection_guide.md#guide-owner-manual-confirmation)

| 控制点中文说明 | 证据来源或命令 | 当前状态 |
|---|---|---:|
| 证明每笔订单都需要所有者确认。 | runbook 或配置引用。 | Pass: owner-note:2026-05-12-manual-confirmation-policy-v1; every future account-changing action requires owner confirmation of the same execution-plan hash, approval expiry, and re-approval after any plan change; approval cannot bypass risk, capital reservation, feature flags, runtime permissions, kill switch, ledger, reconciliation, venue permissions, or signer policy |

### evidence-owner-real-signing-policy

证据 ID：[`owner-real-signing-policy`](personal_guarded_live_evidence_collection_guide.md#guide-owner-real-signing-policy)

| 控制点中文说明 | 证据来源或命令 | 当前状态 |
|---|---|---:|
| 证明真实签名默认关闭，或有单独 owner-approved policy 和 fail-closed 控制。 | 脱敏签名策略引用；不含签名材料。 | Pass: owner-note:2026-05-12-real-signing-disabled-v1; no owner-approved real-signing policy is in force, no signing material is stored, and real signing remains disabled by default |

### evidence-owner-kill-switch-drill

证据 ID：[`owner-kill-switch-drill`](personal_guarded_live_evidence_collection_guide.md#guide-owner-kill-switch-drill)

| 控制点中文说明 | 证据来源或命令 | 当前状态 |
|---|---|---:|
| 完成全局、执行分发、执行模式、场所、策略、账户、工具、资产、链的熔断演练。 | 本地演练记录，包含阻断范围、时间戳和审计/事件引用。 | Pass: drill:2026-05-12-global-stop-v1, drill:2026-05-12-account-stop-v1, drill:2026-05-12-instrument-stop-v1, drill:2026-05-12-asset-stop-v1, drill:2026-05-12-chain-stop-v1; cmd:2026-05-12-runtime-health-global-stop-v1, cmd:2026-05-12-runtime-open-kill-switch-v1, cmd:2026-05-12-runtime-live-without-circuit-breaker-v1, cmd:2026-05-12-config-kill-switch-live-block-v2, cmd:2026-05-12-risk-strategy-kill-switch-v1, cmd:2026-05-12-risk-venue-kill-switch-v1, cmd:2026-05-12-risk-account-kill-switch-v1, cmd:2026-05-12-risk-instrument-kill-switch-v1, cmd:2026-05-12-risk-asset-kill-switch-v1, cmd:2026-05-12-runtime-chain-kill-switch-v1; global stop returned `health: degraded`, `kill_switch_triggered=true`, and `mutable_execution_started=false`; runtime tests confirmed open kill switch skips mutable execution and live execution without circuit breaker is rejected; config test confirmed execution-mode, strategy, venue, account, instrument, asset and chain kill switch predicates; risk tests confirmed blocked strategy, venue, account, instrument and asset return explicit rejection reason codes before execution planning or dispatch; chain scoped kill switch is reported in runtime health and does not start mutable execution |

### evidence-owner-unknown-state-drill

证据 ID：[`owner-unknown-state-drill`](personal_guarded_live_evidence_collection_guide.md#guide-owner-unknown-state-drill)

| 控制点中文说明 | 证据来源或命令 | 当前状态 |
|---|---|---:|
| 完成未知状态停机演练。 | 事故或演练记录引用。 | Pass: drill:2026-05-12-unknown-state-offline-v1; cmd:2026-05-12-runtime-replay-unknown-state-v1, cmd:2026-05-12-runtime-failure-path-incidents-v1, cmd:2026-05-12-risk-rejects-unknown-state-v1, cmd:2026-05-12-execution-unknown-state-v1, cmd:2026-05-12-execution-timeout-unknown-state-v1; full-pipeline unknown-state replay matched 10 artifacts, failure-path fixture test confirmed incidents and no execution plans after risk rejection, risk rejected UNKNOWN_STATE, and execution unknown/timeout states required reconciliation instead of being treated as success |

### evidence-owner-reconciliation-drill

证据 ID：[`owner-reconciliation-drill`](personal_guarded_live_evidence_collection_guide.md#guide-owner-reconciliation-drill)

| 控制点中文说明 | 证据来源或命令 | 当前状态 |
|---|---|---:|
| 完成对账不一致阻断演练。 | 事故或演练记录引用。 | Pass: drill:2026-05-12-reconciliation-mismatch-offline-v1; cmd:2026-05-12-reconciliation-differences-v1, cmd:2026-05-12-reconciliation-severe-incident-v2, cmd:2026-05-12-reconciliation-match-v1; reconciliation tests confirmed differences across core categories, severe differences generate incident suggestions with `PauseAffectedScopeUntilReviewed`, matched inputs do not create false differences, and `fixtures/replay/reconciliation_mismatch/expected/incidents.jsonl` records `TradingPaused` plus `ManualReview` without secrets |

### evidence-owner-permission-failure-drill

证据 ID：[`owner-permission-failure-drill`](personal_guarded_live_evidence_collection_guide.md#guide-owner-permission-failure-drill)

| 控制点中文说明 | 证据来源或命令 | 当前状态 |
|---|---|---:|
| 完成权限缺失或权限不足 fail-closed 演练。 | 事故/演练记录引用和脱敏权限证据。 | Pass: drill:2026-05-12-permission-failure-offline-v1; cmd:2026-05-12-risk-missing-venue-capability-v2, cmd:2026-05-12-risk-manual-only-venue-v2, cmd:2026-05-12-venue-exec-default-no-live-v2, cmd:2026-05-12-venue-exec-unknown-status-v2; offline tests confirmed missing venue capability requires more data, manual-only venue requires manual approval, default build does not enable live execution, and unknown action status is explicit fail-closed; current scope has no approved live API key, so any future nonzero live key must provide new redacted permission evidence before this gate can remain Pass for that expanded scope |

### evidence-owner-signer-failure-drill

证据 ID：[`owner-signer-failure-drill`](personal_guarded_live_evidence_collection_guide.md#guide-owner-signer-failure-drill)

| 控制点中文说明 | 证据来源或命令 | 当前状态 |
|---|---|---:|
| 完成签名器禁用、不可用、拒绝或策略不匹配 fail-closed 演练。 | 事故/演练记录引用；不含签名材料。 | Pass: drill:2026-05-12-signer-failure-offline-v1; cmd:2026-05-12-signing-null-signer-v1, cmd:2026-05-12-signing-failure-not-success-v1, cmd:2026-05-12-signing-disabled-policy-v1, cmd:2026-05-12-signing-policy-mismatch-redacted-v1, cmd:2026-05-12-signing-invalid-input-redacted-v1, cmd:2026-05-12-signing-redaction-v1; signing tests confirmed null signer fails closed with audit ref, signing failure is not success, disabled policy rejects before signing, policy mismatch uses redacted values, invalid input does not echo candidate secrets, and logs/reports do not expose sensitive material |

### evidence-owner-post-action-reconciliation

证据 ID：[`owner-post-action-reconciliation`](personal_guarded_live_evidence_collection_guide.md#guide-owner-post-action-reconciliation)

| 控制点中文说明 | 证据来源或命令 | 当前状态 |
|---|---|---:|
| 证明每个 live action 后有执行报告、账本、场所余额、仓位和成交对账流程。 | runbook 或清单引用。 | Pass: owner-note:2026-05-12-post-action-reconciliation-policy-v1; any future live action must reconcile execution report, ledger entry, venue balance, venue position, and fills before the next live cycle |

### evidence-owner-incident-note-template

证据 ID：[`owner-incident-note-template`](personal_guarded_live_evidence_collection_guide.md#guide-owner-incident-note-template)

| 控制点中文说明 | 证据来源或命令 | 当前状态 |
|---|---|---:|
| 证明所有者能记录 mismatch、unknown state、permission failure 或 signer failure 事故。 | 事故记录模板引用。 | Pass: owner-note:2026-05-12-incident-note-template-v1; incident notes must include date, affected scope, trigger, expected block, observed result, action taken, status, and audit/event references, without secrets |

### evidence-owner-daily-review

证据 ID：[`owner-daily-review`](personal_guarded_live_evidence_collection_guide.md#guide-owner-daily-review)

| 控制点中文说明 | 证据来源或命令 | 当前状态 |
|---|---|---:|
| 证明每次 pilot session 后所有者会检查运营报告。 | 带日期的运营复盘清单或报告引用。 | Pass: owner-note:2026-05-12-daily-review-policy-v1; after each future pilot session the owner must review operations report status, unresolved incidents, open orders/actions, reconciliation status, and kill-switch state |

### evidence-owner-final-decision

证据 ID：[`owner-final-decision`](personal_guarded_live_evidence_collection_guide.md#guide-owner-final-decision)

| 控制点中文说明 | 证据来源或命令 | 当前状态 |
|---|---|---:|
| 最终 approve/reject 决策已记录。 | 完成后的最终所有者决策记录。 | Pass: owner-note:2026-05-12-final-decision-approve-guarded-live-v1; approved scope is one isolated personal CEX subaccount, one demo strategy, BTC-USDC only on CEX custody, 30 minutes maximum duration, 100 USDC capital cap, 10 USDC per-order/action cap, 20 USDC daily loss stop, max 1 open order/action, owner manual confirmation for every order, no withdrawals, no autonomous live execution, and real signing disabled unless separately owner-approved |


## 最近命令证据 / Latest Command Evidence

中文说明：这些命令必须在 pilot 前立即运行。不要粘贴大段日志；只记录命令、日期、
通过/失败和简短摘要。

| 命令 | 日期 | 结果 | 备注 |
|---|---|---:|---|
| `cargo fmt --all -- --check` | 2026-05-12 | Pass | cmd:2026-05-12-cargo-fmt-v2; no formatting drift |
| `cargo clippy --workspace --all-targets -- -D warnings` | 2026-05-12 | Pass | cmd:2026-05-12-cargo-clippy-v2; finished with `-D warnings` |
| `cargo test --workspace` | 2026-05-12 | Pass | cmd:2026-05-12-cargo-test-workspace-v2; workspace unit and doc tests passed |
| `cargo xtask check-schema` | 2026-05-12 | Pass | cmd:2026-05-12-check-schema-v2; parsed 13 schema files, 13 valid fixtures, 13 invalid fixtures |
| `cargo xtask check-crate-boundaries` | 2026-05-12 | Pass | cmd:2026-05-12-check-crate-boundaries-v2; checked 26 forbidden dependency rules |
| `cargo xtask check-docs` | 2026-05-12 | Pass | cmd:2026-05-12-check-docs-v2; checked 4 required docs and 3 fixture notes |
| `cargo xtask replay-full-pipeline` | 2026-05-12 | Pass | cmd:2026-05-12-replay-full-pipeline-v2; matched 10 S9-01 artifacts offline |
| `cargo run -p arb-runtime -- health fixtures/replay/full_pipeline_simulated` with temporary `kill_switch.global=true` | 2026-05-12 | Pass | cmd:2026-05-12-runtime-health-global-stop-v1; returned `health: degraded`, `kill_switch_triggered=true`, `mutable_execution_started=false`; config restored with no residual diff |
| `cargo test -p arb-runtime open_kill_switch_skips_mutable_execution_task` | 2026-05-12 | Pass | cmd:2026-05-12-runtime-open-kill-switch-v1; 1 test passed |
| `cargo test -p arb-runtime live_execution_without_circuit_breaker_is_rejected` | 2026-05-12 | Pass | cmd:2026-05-12-runtime-live-without-circuit-breaker-v1; 1 test passed |
| `cargo test -p arb-config kill_switch_blocks_live_account_changes` | 2026-05-12 | Pass | cmd:2026-05-12-config-kill-switch-live-block-v2; 1 test passed; covers execution mode, strategy, venue, account, instrument, asset and chain predicates |
| `cargo test -p arb-risk rejects_strategy_blocked_by_kill_switch` | 2026-05-12 | Pass | cmd:2026-05-12-risk-strategy-kill-switch-v1; 1 test passed |
| `cargo test -p arb-risk rejects_venue_blocked_by_kill_switch` | 2026-05-12 | Pass | cmd:2026-05-12-risk-venue-kill-switch-v1; 1 test passed |
| `cargo test -p arb-risk rejects_account_blocked_by_kill_switch` | 2026-05-12 | Pass | cmd:2026-05-12-risk-account-kill-switch-v1; 1 test passed |
| `cargo test -p arb-risk rejects_instrument_blocked_by_kill_switch` | 2026-05-12 | Pass | cmd:2026-05-12-risk-instrument-kill-switch-v1; 1 test passed |
| `cargo test -p arb-risk rejects_asset_blocked_by_kill_switch` | 2026-05-12 | Pass | cmd:2026-05-12-risk-asset-kill-switch-v1; 1 test passed |
| `cargo test -p arb-runtime scoped_chain_kill_switch_is_reported_without_mutable_execution` | 2026-05-12 | Pass | cmd:2026-05-12-runtime-chain-kill-switch-v1; 1 test passed |
| `cargo run -p arb-runtime -- replay fixtures/replay/full_pipeline_simulated/cases/unknown_state` | 2026-05-12 | Pass | cmd:2026-05-12-runtime-replay-unknown-state-v1; matched 10 artifacts |
| `cargo test -p arb-runtime failure_path_fixtures_reject_and_emit_incidents` | 2026-05-12 | Pass | cmd:2026-05-12-runtime-failure-path-incidents-v1; 1 test passed |
| `cargo test -p arb-risk rejects_unknown_state_instead_of_approving` | 2026-05-12 | Pass | cmd:2026-05-12-risk-rejects-unknown-state-v1; 1 test passed |
| `cargo test -p arb-execution simulated_unknown_state_is_risk_critical_and_requires_reconciliation` | 2026-05-12 | Pass | cmd:2026-05-12-execution-unknown-state-v1; 1 test passed |
| `cargo test -p arb-execution simulated_timeout_enters_unknown_state_and_requires_reconciliation` | 2026-05-12 | Pass | cmd:2026-05-12-execution-timeout-unknown-state-v1; 1 test passed |
| `cargo test -p arb-reconciliation differences_are_found_across_all_core_categories` | 2026-05-12 | Pass | cmd:2026-05-12-reconciliation-differences-v1; 1 test passed |
| `cargo test -p arb-reconciliation severe_difference_generates_incident_suggestion` | 2026-05-12 | Pass | cmd:2026-05-12-reconciliation-severe-incident-v2; 1 test passed; asserts pause affected scope action |
| `cargo test -p arb-reconciliation matched_inputs_produce_report_for_ops_without_differences` | 2026-05-12 | Pass | cmd:2026-05-12-reconciliation-match-v1; 1 test passed |
| `cargo test -p arb-risk requires_more_data_when_venue_capability_snapshot_is_missing` | 2026-05-12 | Pass | cmd:2026-05-12-risk-missing-venue-capability-v2; 1 test passed |
| `cargo test -p arb-risk requires_manual_approval_for_manual_only_venue` | 2026-05-12 | Pass | cmd:2026-05-12-risk-manual-only-venue-v2; 1 test passed |
| `cargo test -p arb-venue-exec default_feature_does_not_enable_live_exec` | 2026-05-12 | Pass | cmd:2026-05-12-venue-exec-default-no-live-v2; 1 test passed |
| `cargo test -p arb-venue-exec query_unknown_action_status_is_explicit_and_fail_closed` | 2026-05-12 | Pass | cmd:2026-05-12-venue-exec-unknown-status-v2; 1 test passed |
| `cargo test -p arb-signing null_signer_fails_closed_with_audit_ref` | 2026-05-12 | Pass | cmd:2026-05-12-signing-null-signer-v1; 1 test passed |
| `cargo test -p arb-signing signing_failure_is_not_success` | 2026-05-12 | Pass | cmd:2026-05-12-signing-failure-not-success-v1; 1 test passed |
| `cargo test -p arb-signing disabled_policy_rejects_before_signing_attempt` | 2026-05-12 | Pass | cmd:2026-05-12-signing-disabled-policy-v1; 1 test passed |
| `cargo test -p arb-signing policy_mismatch_uses_redacted_values` | 2026-05-12 | Pass | cmd:2026-05-12-signing-policy-mismatch-redacted-v1; 1 test passed |
| `cargo test -p arb-signing invalid_input_errors_do_not_echo_candidate_secret` | 2026-05-12 | Pass | cmd:2026-05-12-signing-invalid-input-redacted-v1; 1 test passed |
| `cargo test -p arb-signing redacted_log_and_report_do_not_expose_sensitive_material` | 2026-05-12 | Pass | cmd:2026-05-12-signing-redaction-v1; 1 test passed |

## 证据处理规则 / Evidence Handling Rules

- 只保存脱敏引用。
- 不粘贴含有秘密或完整账户标识的原始截图。
- 不粘贴 API key、API secret、私钥、助记词、session token、webhook token 或 signer payload。
- 不粘贴原始私有余额；只记录资金上限和 owner 风险接受。
- 如果意外捕获了秘密材料，应先轮换受影响凭证，再替换为脱敏引用。
- AI 可以帮助更新证据索引，但不能代替所有者批准 live trading。
