# Venue Data Smoke Replay

中文说明：本 fixture 覆盖阶段 8 第一个只读场所数据适配器。原始响应来自公开行情形态并已脱敏，只保留市场数据字段，不包含账户、API key、签名、余额或仓位私有信息。

- `raw/`：脱敏原始响应和边界场景输入。
- `venue_capabilities.jsonl`：`venue:BINANCE_PUBLIC` 的只读场所能力配置样例。
- `expected/normalized_events.jsonl`：适配器应输出的可回放标准化事件。
- `expected/error_classifications.jsonl`：断线、重连、限频、乱序、重复和缺字段场景的错误分类期望。

默认测试只读取这些离线文件，不访问真实网络，也不需要真实 API key。
