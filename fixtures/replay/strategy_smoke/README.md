# strategy_smoke

中文说明：该目录保存阶段 4 样例策略的固定输入输出黄金结果。策略只读地读取事件、组合状态、场所能力、配置和固定时间源，输出候选组合转换或明确拒绝，不包含下单、签名、转账或账本写入能力。

文件说明：

- `events.jsonl`：离线标准化事件输入，带规范 checksum。
- `config.yaml`：只读配置输入，默认不允许真实账户变化。
- `replay.yaml`：固定时间源和随机种子。
- `strategy_manifest.yaml`：样例策略版本和入口说明。
- `portfolio_state.json`：策略读取的组合状态快照。
- `venue_capabilities.jsonl`：策略读取的场所能力快照。
- `expected/candidate_transitions.jsonl`：回放期望候选转换输出。
