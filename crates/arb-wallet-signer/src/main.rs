//! `arb-wallet-signer` 进程入口。
//!
//! 中文说明：签名逻辑放在库中，main 只负责 CLI、stdin/stdout 和退出码。

#![forbid(unsafe_code)]

use std::io::{self, Read};

fn main() {
    let mut stdin = String::new();
    if let Err(error) = io::stdin().read_to_string(&mut stdin) {
        eprintln!("arb-wallet-signer failed to read stdin: {error}");
        std::process::exit(2);
    }

    match arb_wallet_signer::run_cli(std::env::args().skip(1), &stdin, |name| {
        std::env::var(name).ok()
    }) {
        Ok(output) => {
            println!("{output}");
        }
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(2);
        }
    }
}
