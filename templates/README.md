# 运行时模板目录

本目录保存运行时代码直接读取的 `template`（模板）文件。模板必须保持离线可用、默认安全，并且不能包含密钥、账户完整标识、私钥、令牌或任何凭证明文。

## 当前文件

- `config.template.yaml`：运行时代码侧配置模板，由 `arb-config`（配置模块）读取。默认 `ReadOnly`（只读模式），不启用真实执行或真实签名。
- `daily_operations_report.template.md`：每日运营报告模板，由 `arb-ops`（运营只读模块）渲染。英文标签保留用于测试样例和下游脚本稳定匹配，中文解释跟随展示。
- `personal_guarded_live.preflight.yaml`：个人受控实盘预检配置模板，仅用于本地启动检查。该模板仍通过熔断配置阻断 `GuardedLive`（受控实盘）模式下的可变执行。

## 维护规则

- 修改模板后，运行与模板相关的测试或 `cargo xtask quality-gate`（项目质量门）。
- 修改 `daily_operations_report.template.md` 后，同步更新回放 `fixture`（测试样例）中的期望报告。
- 不新增对已清理 `docs/`（旧开发文档）或 `review/`（旧评审材料）入口的依赖，除非用户明确要求恢复。
