use std::error::Error;
use std::fmt;
use std::path::PathBuf;

/// 运行时统一返回类型。
pub type RuntimeResult<T> = Result<T, RuntimeError>;

/// 运行时装配错误。
#[derive(Debug)]
pub enum RuntimeError {
    Io {
        path: PathBuf,
        message: String,
    },
    Module {
        module: &'static str,
        message: String,
    },
    UnsafeConfig {
        message: String,
    },
    StartupRejected {
        reasons: Vec<String>,
    },
    MissingFixture {
        path: PathBuf,
    },
    LiveMarketData {
        message: String,
    },
    StrategyRejected {
        reason: String,
        detail: Option<String>,
    },
    GoldenMismatch {
        artifact: &'static str,
        path: PathBuf,
        expected_bytes: usize,
        actual_bytes: usize,
        first_difference: Option<usize>,
    },
}

impl fmt::Display for RuntimeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { path, message } => write!(f, "{}: {message}", path.display()),
            Self::Module { module, message } => write!(f, "{module}: {message}"),
            Self::UnsafeConfig { message } => write!(f, "unsafe runtime config: {message}"),
            Self::StartupRejected { reasons } => {
                write!(f, "runtime startup rejected: {}", reasons.join("; "))
            }
            Self::MissingFixture { path } => {
                write!(f, "{}: expected fixture file is missing", path.display())
            }
            Self::LiveMarketData { message } => write!(f, "live market data failed: {message}"),
            Self::StrategyRejected { reason, detail } => {
                write!(f, "strategy rejected candidate with reason `{reason}`")?;
                if let Some(detail) = detail {
                    write!(f, ": {detail}")?;
                }
                Ok(())
            }
            Self::GoldenMismatch {
                artifact,
                path,
                expected_bytes,
                actual_bytes,
                first_difference,
            } => write!(
                f,
                "{}: golden mismatch for {artifact}; expected {expected_bytes} bytes, got {actual_bytes} bytes, first difference at {:?}",
                path.display(),
                first_difference
            ),
        }
    }
}

impl Error for RuntimeError {}

macro_rules! module_error_from {
    ($source:ty, $module:literal) => {
        impl From<$source> for RuntimeError {
            fn from(error: $source) -> Self {
                Self::Module {
                    module: $module,
                    message: error.to_string(),
                }
            }
        }
    };
}

module_error_from!(arb_config::ConfigError, "arb-config");
module_error_from!(arb_contracts::ContractError, "arb-contracts");
module_error_from!(arb_domain::DomainError, "arb-domain");
module_error_from!(arb_eventstore::EventStoreError, "arb-eventstore");
module_error_from!(arb_execution::ExecutionError, "arb-execution");
module_error_from!(arb_ledger::LedgerError, "arb-ledger");
module_error_from!(arb_ops::OpsError, "arb-ops");
module_error_from!(
    arb_reconciliation::ReconciliationError,
    "arb-reconciliation"
);
module_error_from!(arb_replay::ReplayError, "arb-replay");
module_error_from!(arb_risk::RiskError, "arb-risk");
module_error_from!(arb_strategy_api::StrategyApiError, "arb-strategy-api");
module_error_from!(arb_venue_data::VenueDataError, "arb-venue-data");
#[cfg(feature = "live-exec")]
module_error_from!(arb_signing::SigningError, "arb-signing");
#[cfg(feature = "live-exec")]
module_error_from!(arb_venue_exec::VenueExecError, "arb-venue-exec");
