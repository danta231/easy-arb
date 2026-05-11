# Invalid Schema Fixtures

中文说明：本目录放置应被 schema 校验拒绝的反向 JSON fixture。S0-02 只创建目录和说明，后续阶段会补充缺字段、未知字段、错误枚举和错误 decimal 等样例。

约束：

- 反例用于测试拒绝路径，不代表可接受输入。
- 不放密钥、API key、私钥、token 或任何真实凭证。
- 即使是反例，也应尽量保持 JSON 语法可解析，方便后续区分语法错误和合同错误。
