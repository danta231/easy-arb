# State Machines and Replay Fixtures / 状态机与回放样例

The immutable core depends on explicit state machines and deterministic replay fixtures.

中文说明：不可变核心不是靠约定保证安全，而是通过状态机、事件序列和 fixture 测试保证行为可复现。

## Execution leg state machine / 执行腿状态机

Allowed states:

- `Prepared`
- `WaitingDependency`
- `Ready`
- `Dispatched`
- `Acknowledged`
- `PartiallyFilled`
- `Filled`
- `CancelRequested`
- `Cancelled`
- `Failed`
- `Unknown`
- `Compensating`
- `Compensated`

中文说明：`Unknown` 是风险关键状态，不能自动转为成功。进入 `Unknown` 后必须触发对账、告警或人工处理。

Allowed transition families:

- Prepared -> WaitingDependency -> Ready. 中文：依赖满足前不能执行。
- Ready -> Dispatched -> Acknowledged -> Filled. 中文：正常成交路径。
- Acknowledged -> PartiallyFilled -> Filled or CancelRequested. 中文：部分成交必须受 partial-fill policy 控制。
- Any live dispatch state -> Unknown when venue state cannot be proven. 中文：实盘调度后只要无法证明状态，就进入 Unknown。
- Unknown -> Compensating or ManualIntervention. 中文：未知状态只能进入补偿或人工处理。

## Capital reservation state machine / 资本预留状态机

Allowed states:

- `Requested`
- `Reserved`
- `ConvertedToExecution`
- `Released`
- `Expired`
- `ReconciledMismatch`

中文说明：资本预留必须有过期、释放或转为账本事件的路径。禁止永久悬挂预留。

## Ledger adjustment state machine / 账本调整状态机

Ledger entries are append-only.

中文说明：账本条目只能追加，不能原地修改。错误修正必须生成新的 reversal 或 adjustment 分录。

Allowed correction types:

- `Reversal`: fully offsets a previous journal entry. 中文：完全冲销原分录。
- `Adjustment`: posts the delta required by reconciliation and carries `adjustment_reason_code`. 中文：按对账差额补充分录，并必须携带 `adjustment_reason_code`。

## Replay fixture layout / 回放 fixture 目录

Each replay case should use this structure:

```text
fixtures/replay/<case_name>/
  events.jsonl
  config.yaml
  risk_policy.yaml
  strategy_manifest.yaml
  expected/
    candidate_transitions.jsonl
    risk_decisions.jsonl
    execution_plans.jsonl
    ledger_entries.jsonl
    incidents.jsonl
```

中文说明：fixture 必须同时包含输入事件、配置、风控策略、策略版本和期望输出。只保存事件而不保存版本信息，无法证明回放确定性。

阶段 7 对账和事故 fixture 至少包括：

- `fixtures/replay/reconciliation_match/`：对账一致，不产生事故。中文：证明一致路径不会误报。
- `fixtures/replay/reconciliation_mismatch/`：对账差异，产生可追溯事件 ID 的事故。中文：证明差异不能被静默忽略。
- `fixtures/replay/incident_unknown_state/`：未知状态，产生可追溯事件 ID 的事故。中文：证明未知状态不能按成功处理。

阶段 8 只读场所数据 fixture 至少包括：

- `fixtures/replay/venue_data_smoke/`：第一个只读场所适配器的公开行情、断线、重连、限频、乱序、重复、缺字段和 stale 数据样例。中文：证明只读适配器可在无网络、无 API key 的情况下输出可回放标准化事件和场所健康事件。
- `fixtures/replay/venue_data_smoke/venue_capabilities.jsonl`：只读场所能力配置样例。中文：证明场所按能力建模，且公开数据读取不暗含下单、撤单、转账或签名能力。

阶段 10 人工审批 fixture 至少包括：

- `fixtures/replay/manual_approval_approved/`：审批通过事件、审批记录、审批材料样例、`config.yaml` 和 `replay.yaml`。中文：证明审批通过只释放同一 `plan_hash` 的人工门禁，不能绕过风控、账本、对账、kill switch 或执行权限。
- `fixtures/replay/manual_approval_rejected/`：审批拒绝、过期和重复审批事件、审批记录、`config.yaml` 和 `replay.yaml`。中文：证明拒绝、过期和重复审批不能释放人工门禁，且这些审批事实可通过 replay fixture 离线加载。

## Replay ordering / 回放排序

Ordering rules:

1. Sort by event-store `sequence`.
2. If sequence is absent during import, sort by source stream and `source_sequence`.
3. Use `timestamp_event` only as a final tie-breaker.
4. Record any nondeterministic ordering as an incident or replay warning.

中文说明：不能依赖本地接收时间来决定业务顺序；接收时间只能用于延迟和新鲜度判断。

## Determinism rules / 确定性规则

- Strategies read from injected `TimeSource`, not system time. 中文：策略读取注入时间源，不能直接读系统时间。
- Randomness must use recorded seed. 中文：随机性必须记录种子。
- Decimal rounding mode must be fixed per schema version. 中文：decimal 舍入规则必须随 schema 版本固定。
- External API calls are forbidden during replay. 中文：回放期间禁止访问外部 API。
- Missing data must produce explicit `RequiresMoreData`, `Rejected`, or `UnknownState`. 中文：缺数据必须显式进入对应决策，不能静默跳过。
