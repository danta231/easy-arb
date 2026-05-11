# AGENTS.md

本仓库按 `universal_arb_platform_v2_immutable_core_docs/docs/24_Codex_Development_Runbook.md` 执行开发。

## Required Reading

每次开发前先读取：

1. `universal_arb_platform_v2_immutable_core_docs/docs/24_Codex_Development_Runbook.md`
2. `universal_arb_platform_v2_immutable_core_docs/docs/22_Development_Execution_Plan.md`
3. `universal_arb_platform_v2_immutable_core_docs/docs/23_Module_Architecture_Map.md`
4. `universal_arb_platform_v2_immutable_core_docs/docs/25_Core_Architecture_Reference.md`

## Path Rules

- `DOC_ROOT` is `universal_arb_platform_v2_immutable_core_docs`.
- `CODE_ROOT` is the repository root.
- Rust workspace, `crates/`, `fixtures/`, and `xtask/` belong under `CODE_ROOT`.
- Documentation short paths such as `docs/...` resolve under `DOC_ROOT`.

## Hard Rules

- Do not implement real order placement, cancellation, transfer, or real signing before the allowed phase.
- Default development is read-only, simulated, replayable, and offline-testable.
- Use Rust tooling and `cargo xtask ...` for project checks after stage 0.
- Do not introduce Node.js as a formal project dependency.
- Do not put secrets, API keys, private keys, tokens, or credentials in code, logs, fixtures, docs, or reports.
- Unknown external state must fail closed and must not be treated as success.

## Workflow

Prefer task-package execution:

```text
请按 universal_arb_platform_v2_immutable_core_docs/docs/24_Codex_Development_Runbook.md 执行任务包 <ID>。
```

Do not duplicate the runbook here. If this file conflicts with the runbook, the runbook wins unless the conflict involves real execution, signing, funds, credentials, or unsafe external state; in that case use the stricter rule.
