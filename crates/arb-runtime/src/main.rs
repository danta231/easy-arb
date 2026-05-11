//! `arb-runtime` 进程入口。
//!
//! 中文说明：业务装配在库入口中完成，`main` 只做 CLI 转发，不承载策略、
//! 风控、账本或执行状态机规则。

#![forbid(unsafe_code)]

fn main() {
    std::process::exit(arb_runtime::main_cli());
}
