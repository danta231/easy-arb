//! `arb-ledger` 复式账本核心模块。
//!
//! 中文说明：本 crate 只负责追加式账本事实、借贷平衡规则、幂等入账和
//! 可由分录推导的余额视图；不调用策略、风控、执行、签名或运行时装配。

#![forbid(unsafe_code)]

use arb_domain::{
    AccountId, Amount, AssetId, CandidateTransitionId, Decimal, DomainError, EventId,
    ExecutionPlanId, LedgerEntryId, StrategyId, UtcTimestamp,
};
use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt;

/// 账本层统一返回类型。
pub type LedgerResult<T> = Result<T, LedgerError>;

/// 账本核心错误。
///
/// 中文说明：账本输入失败必须显式返回，不能把未知状态或不平衡分录当成成功。
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LedgerError {
    Domain(DomainError),
    InvalidIdentifier {
        type_name: &'static str,
        value: String,
        reason: &'static str,
    },
    InvalidHash {
        value: String,
        reason: &'static str,
    },
    EntryHasTooFewLegs {
        ledger_entry_id: Option<LedgerEntryId>,
        leg_count: usize,
    },
    UnbalancedEntry {
        asset_id: AssetId,
        debit_total: Decimal,
        credit_total: Decimal,
    },
    BalanceAssertionNotTrue {
        ledger_entry_id: LedgerEntryId,
    },
    BalanceAssertionHashMismatch {
        ledger_entry_id: LedgerEntryId,
        expected: BalanceAssertionHash,
        actual: BalanceAssertionHash,
    },
    DuplicateAccount {
        account_id: AccountId,
    },
    DuplicateLedgerEntryId {
        ledger_entry_id: LedgerEntryId,
    },
    IdempotencyConflict {
        idempotency_key: IdempotencyKey,
        existing_entry_id: LedgerEntryId,
        attempted_entry_id: LedgerEntryId,
    },
    InvalidReasonCode {
        value: String,
        reason: &'static str,
    },
    ConflictingCorrectionLinks {
        ledger_entry_id: LedgerEntryId,
    },
    MissingAdjustmentReasonCode {
        ledger_entry_id: LedgerEntryId,
    },
    UnexpectedAdjustmentReasonCode {
        ledger_entry_id: LedgerEntryId,
    },
    AdjustmentRequiresAdjustmentType {
        ledger_entry_id: LedgerEntryId,
        entry_type: LedgerEntryType,
    },
    AdjustmentEntryRequiresTarget {
        ledger_entry_id: LedgerEntryId,
    },
    ReferencedLedgerEntryNotFound {
        ledger_entry_id: LedgerEntryId,
    },
    DuplicateReversal {
        original_entry_id: LedgerEntryId,
        existing_reversal_entry_id: LedgerEntryId,
        attempted_reversal_entry_id: LedgerEntryId,
    },
}

impl fmt::Display for LedgerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Domain(error) => write!(f, "{error}"),
            Self::InvalidIdentifier {
                type_name,
                value,
                reason,
            } => write!(f, "{type_name} `{value}` is invalid: {reason}"),
            Self::InvalidHash { value, reason } => {
                write!(f, "balance assertion hash `{value}` is invalid: {reason}")
            }
            Self::EntryHasTooFewLegs {
                ledger_entry_id,
                leg_count,
            } => {
                if let Some(ledger_entry_id) = ledger_entry_id {
                    write!(
                        f,
                        "ledger entry `{ledger_entry_id}` has {leg_count} legs, at least 2 are required"
                    )
                } else {
                    write!(
                        f,
                        "ledger entry draft has {leg_count} legs, at least 2 are required"
                    )
                }
            }
            Self::UnbalancedEntry {
                asset_id,
                debit_total,
                credit_total,
            } => write!(
                f,
                "ledger entry is not balanced for asset `{asset_id}`: debit {debit_total}, credit {credit_total}"
            ),
            Self::BalanceAssertionNotTrue { ledger_entry_id } => write!(
                f,
                "ledger entry `{ledger_entry_id}` does not carry a true balance assertion"
            ),
            Self::BalanceAssertionHashMismatch {
                ledger_entry_id,
                expected,
                actual,
            } => write!(
                f,
                "ledger entry `{ledger_entry_id}` balance assertion hash mismatch: expected {expected}, got {actual}"
            ),
            Self::DuplicateAccount { account_id } => {
                write!(f, "ledger account `{account_id}` already exists")
            }
            Self::DuplicateLedgerEntryId { ledger_entry_id } => {
                write!(f, "ledger entry `{ledger_entry_id}` already exists")
            }
            Self::IdempotencyConflict {
                idempotency_key,
                existing_entry_id,
                attempted_entry_id,
            } => write!(
                f,
                "idempotency key `{idempotency_key}` already belongs to `{existing_entry_id}`, not `{attempted_entry_id}`"
            ),
            Self::InvalidReasonCode { value, reason } => {
                write!(f, "adjustment reason code `{value}` is invalid: {reason}")
            }
            Self::ConflictingCorrectionLinks { ledger_entry_id } => write!(
                f,
                "ledger entry `{ledger_entry_id}` cannot be both a reversal and an adjustment"
            ),
            Self::MissingAdjustmentReasonCode { ledger_entry_id } => write!(
                f,
                "ledger adjustment `{ledger_entry_id}` must carry an adjustment reason code"
            ),
            Self::UnexpectedAdjustmentReasonCode { ledger_entry_id } => write!(
                f,
                "ledger entry `{ledger_entry_id}` carries an adjustment reason code without adjustment_of"
            ),
            Self::AdjustmentRequiresAdjustmentType {
                ledger_entry_id,
                entry_type,
            } => write!(
                f,
                "ledger adjustment `{ledger_entry_id}` uses non-adjustment entry type `{}`",
                entry_type.as_str()
            ),
            Self::AdjustmentEntryRequiresTarget { ledger_entry_id } => write!(
                f,
                "ledger adjustment entry `{ledger_entry_id}` must reference adjustment_of"
            ),
            Self::ReferencedLedgerEntryNotFound { ledger_entry_id } => {
                write!(f, "referenced ledger entry `{ledger_entry_id}` was not found")
            }
            Self::DuplicateReversal {
                original_entry_id,
                existing_reversal_entry_id,
                attempted_reversal_entry_id,
            } => write!(
                f,
                "ledger entry `{original_entry_id}` is already reversed by `{existing_reversal_entry_id}`, cannot append `{attempted_reversal_entry_id}`"
            ),
        }
    }
}

impl Error for LedgerError {}

impl From<DomainError> for LedgerError {
    fn from(error: DomainError) -> Self {
        Self::Domain(error)
    }
}

macro_rules! define_ledger_id {
    ($name:ident, $doc:literal) => {
        #[doc = $doc]
        ///
        /// 中文说明：账本本地 ID 与 schema Identifier 规则保持一致。
        #[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
        pub struct $name(String);

        impl $name {
            pub fn new(value: impl Into<String>) -> LedgerResult<Self> {
                let value = value.into();
                validate_identifier(stringify!($name), &value)?;
                Ok(Self(value))
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }
    };
}

define_ledger_id!(JournalEntryId, "账本 journal entry ID。");
define_ledger_id!(LedgerLegId, "账本分录腿 ID。");
define_ledger_id!(IdempotencyKey, "账本入账幂等键。");

/// 调整原因码。
///
/// 中文说明：调整分录必须携带机器可聚合的原因码，自由文本只能放在 memo。
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct AdjustmentReasonCode(String);

impl AdjustmentReasonCode {
    pub fn new(value: impl Into<String>) -> LedgerResult<Self> {
        let value = value.into();
        validate_reason_code(&value)?;
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for AdjustmentReasonCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// 平衡断言哈希。
///
/// 中文说明：当前实现使用稳定 FNV-1a 校验字符串，只作为回放一致性校验，
/// 不表示密码学安全哈希。
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct BalanceAssertionHash(String);

impl BalanceAssertionHash {
    pub fn new(value: impl Into<String>) -> LedgerResult<Self> {
        let value = value.into();
        validate_hash(&value)?;
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for BalanceAssertionHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// 账本命名空间。
///
/// 中文说明：模拟、回测、实盘和调整账本必须隔离。
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum LedgerNamespace {
    Live,
    Simulation,
    Backtest,
    Adjustment,
}

impl LedgerNamespace {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Live => "Live",
            Self::Simulation => "Simulation",
            Self::Backtest => "Backtest",
            Self::Adjustment => "Adjustment",
        }
    }
}

/// 账本科目类别。
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum AccountKind {
    Asset,
    Liability,
    Equity,
    Income,
    Expense,
    ContraAsset,
    ContraLiability,
    Suspense,
}

impl AccountKind {
    /// 返回该科目的正常余额方向。
    pub fn normal_balance(self) -> LedgerDirection {
        match self {
            Self::Asset | Self::Expense | Self::ContraLiability | Self::Suspense => {
                LedgerDirection::Debit
            }
            Self::Liability | Self::Equity | Self::Income | Self::ContraAsset => {
                LedgerDirection::Credit
            }
        }
    }
}

/// 账本科目。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LedgerAccount {
    pub account_id: AccountId,
    pub kind: AccountKind,
    pub name: String,
    pub custody_location_id: Option<AccountId>,
    pub asset_id: Option<AssetId>,
}

impl LedgerAccount {
    pub fn new(account_id: AccountId, kind: AccountKind, name: impl Into<String>) -> Self {
        Self {
            account_id,
            kind,
            name: name.into(),
            custody_location_id: None,
            asset_id: None,
        }
    }

    pub fn with_custody_location(mut self, custody_location_id: AccountId) -> Self {
        self.custody_location_id = Some(custody_location_id);
        self
    }

    pub fn with_asset(mut self, asset_id: AssetId) -> Self {
        self.asset_id = Some(asset_id);
        self
    }
}

/// 账本科目表。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ChartOfAccounts {
    accounts: BTreeMap<AccountId, LedgerAccount>,
}

impl ChartOfAccounts {
    pub fn insert(&mut self, account: LedgerAccount) -> LedgerResult<()> {
        if self.accounts.contains_key(&account.account_id) {
            return Err(LedgerError::DuplicateAccount {
                account_id: account.account_id,
            });
        }
        self.accounts.insert(account.account_id.clone(), account);
        Ok(())
    }

    pub fn get(&self, account_id: &AccountId) -> Option<&LedgerAccount> {
        self.accounts.get(account_id)
    }

    pub fn accounts(&self) -> impl Iterator<Item = &LedgerAccount> {
        self.accounts.values()
    }
}

/// 账本分录类型。
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum LedgerEntryType {
    TradeFill,
    Fee,
    Funding,
    Transfer,
    CapitalReservation,
    CapitalRelease,
    Borrow,
    Lend,
    Repay,
    RealizedPnl,
    UnrealizedPnlSnapshot,
    ReconciliationAdjustment,
    ManualAdjustment,
}

impl LedgerEntryType {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::TradeFill => "TradeFill",
            Self::Fee => "Fee",
            Self::Funding => "Funding",
            Self::Transfer => "Transfer",
            Self::CapitalReservation => "CapitalReservation",
            Self::CapitalRelease => "CapitalRelease",
            Self::Borrow => "Borrow",
            Self::Lend => "Lend",
            Self::Repay => "Repay",
            Self::RealizedPnl => "RealizedPnl",
            Self::UnrealizedPnlSnapshot => "UnrealizedPnlSnapshot",
            Self::ReconciliationAdjustment => "ReconciliationAdjustment",
            Self::ManualAdjustment => "ManualAdjustment",
        }
    }

    pub fn is_adjustment(self) -> bool {
        matches!(
            self,
            Self::ReconciliationAdjustment | Self::ManualAdjustment
        )
    }
}

/// 借贷方向。
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum LedgerDirection {
    Debit,
    Credit,
}

impl LedgerDirection {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Debit => "Debit",
            Self::Credit => "Credit",
        }
    }
}

/// 账本分录腿。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LedgerLeg {
    pub leg_id: LedgerLegId,
    pub account_id: AccountId,
    pub custody_location_id: Option<AccountId>,
    pub asset_id: AssetId,
    pub direction: LedgerDirection,
    pub amount: Amount,
    pub valuation_usd: Option<Decimal>,
    pub memo: Option<String>,
}

impl LedgerLeg {
    pub fn new(
        leg_id: LedgerLegId,
        account_id: AccountId,
        asset_id: AssetId,
        direction: LedgerDirection,
        amount: Amount,
    ) -> Self {
        Self {
            leg_id,
            account_id,
            custody_location_id: None,
            asset_id,
            direction,
            amount,
            valuation_usd: None,
            memo: None,
        }
    }

    pub fn with_custody_location(mut self, custody_location_id: AccountId) -> Self {
        self.custody_location_id = Some(custody_location_id);
        self
    }

    pub fn with_valuation_usd(mut self, valuation_usd: Decimal) -> Self {
        self.valuation_usd = Some(valuation_usd);
        self
    }

    pub fn with_memo(mut self, memo: impl Into<String>) -> Self {
        self.memo = Some(memo.into());
        self
    }
}

/// 平衡断言。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BalanceAssertion {
    pub balanced: bool,
    pub assertion_hash: BalanceAssertionHash,
    pub checked_by: Option<String>,
}

/// 账本分录头部必填字段。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LedgerEntryHeader {
    pub ledger_entry_id: LedgerEntryId,
    pub journal_entry_id: JournalEntryId,
    pub timestamp: UtcTimestamp,
    pub namespace: LedgerNamespace,
    pub entry_type: LedgerEntryType,
    pub source_event_id: EventId,
    pub idempotency_key: IdempotencyKey,
}

impl LedgerEntryHeader {
    pub fn new(
        ledger_entry_id: LedgerEntryId,
        journal_entry_id: JournalEntryId,
        timestamp: UtcTimestamp,
        namespace: LedgerNamespace,
        entry_type: LedgerEntryType,
        source_event_id: EventId,
        idempotency_key: IdempotencyKey,
    ) -> Self {
        Self {
            ledger_entry_id,
            journal_entry_id,
            timestamp,
            namespace,
            entry_type,
            source_event_id,
            idempotency_key,
        }
    }
}

/// 账本分录草稿。
///
/// 中文说明：草稿通过 `LedgerEntry::from_draft` 转成已校验分录；该转换会
/// 重新计算借贷平衡断言。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LedgerEntryDraft {
    pub ledger_entry_id: LedgerEntryId,
    pub journal_entry_id: JournalEntryId,
    pub schema_version: String,
    pub timestamp: UtcTimestamp,
    pub namespace: LedgerNamespace,
    pub entry_type: LedgerEntryType,
    pub source_event_id: EventId,
    pub idempotency_key: IdempotencyKey,
    pub strategy_id: Option<StrategyId>,
    pub opportunity_id: Option<CandidateTransitionId>,
    pub execution_plan_id: Option<ExecutionPlanId>,
    pub reversal_of: Option<LedgerEntryId>,
    pub adjustment_of: Option<LedgerEntryId>,
    pub adjustment_reason_code: Option<AdjustmentReasonCode>,
    pub legs: Vec<LedgerLeg>,
}

impl LedgerEntryDraft {
    pub fn new(header: LedgerEntryHeader, legs: Vec<LedgerLeg>) -> Self {
        Self {
            ledger_entry_id: header.ledger_entry_id,
            journal_entry_id: header.journal_entry_id,
            schema_version: "1.0.0".to_owned(),
            timestamp: header.timestamp,
            namespace: header.namespace,
            entry_type: header.entry_type,
            source_event_id: header.source_event_id,
            idempotency_key: header.idempotency_key,
            strategy_id: None,
            opportunity_id: None,
            execution_plan_id: None,
            reversal_of: None,
            adjustment_of: None,
            adjustment_reason_code: None,
            legs,
        }
    }

    pub fn with_strategy_id(mut self, strategy_id: StrategyId) -> Self {
        self.strategy_id = Some(strategy_id);
        self
    }

    pub fn with_opportunity_id(mut self, opportunity_id: CandidateTransitionId) -> Self {
        self.opportunity_id = Some(opportunity_id);
        self
    }

    pub fn with_execution_plan_id(mut self, execution_plan_id: ExecutionPlanId) -> Self {
        self.execution_plan_id = Some(execution_plan_id);
        self
    }

    pub fn with_reversal_of(mut self, reversal_of: LedgerEntryId) -> Self {
        self.reversal_of = Some(reversal_of);
        self
    }

    pub fn with_adjustment_of(mut self, adjustment_of: LedgerEntryId) -> Self {
        self.adjustment_of = Some(adjustment_of);
        self
    }

    pub fn with_adjustment_reason_code(mut self, reason_code: AdjustmentReasonCode) -> Self {
        self.adjustment_reason_code = Some(reason_code);
        self
    }
}

/// 已校验的账本分录。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LedgerEntry {
    pub ledger_entry_id: LedgerEntryId,
    pub journal_entry_id: JournalEntryId,
    pub schema_version: String,
    pub timestamp: UtcTimestamp,
    pub namespace: LedgerNamespace,
    pub entry_type: LedgerEntryType,
    pub source_event_id: EventId,
    pub idempotency_key: IdempotencyKey,
    pub strategy_id: Option<StrategyId>,
    pub opportunity_id: Option<CandidateTransitionId>,
    pub execution_plan_id: Option<ExecutionPlanId>,
    pub reversal_of: Option<LedgerEntryId>,
    pub adjustment_of: Option<LedgerEntryId>,
    pub adjustment_reason_code: Option<AdjustmentReasonCode>,
    pub legs: Vec<LedgerLeg>,
    pub balance_assertion: BalanceAssertion,
}

impl LedgerEntry {
    pub fn from_draft(draft: LedgerEntryDraft) -> LedgerResult<Self> {
        assert_legs_balanced(None, &draft.legs)?;
        validate_correction_metadata(
            &draft.ledger_entry_id,
            draft.entry_type,
            draft.reversal_of.as_ref(),
            draft.adjustment_of.as_ref(),
            draft.adjustment_reason_code.as_ref(),
        )?;
        let assertion_hash = compute_assertion_hash(&EntryHashView::from_draft(&draft))?;
        Ok(Self {
            ledger_entry_id: draft.ledger_entry_id,
            journal_entry_id: draft.journal_entry_id,
            schema_version: draft.schema_version,
            timestamp: draft.timestamp,
            namespace: draft.namespace,
            entry_type: draft.entry_type,
            source_event_id: draft.source_event_id,
            idempotency_key: draft.idempotency_key,
            strategy_id: draft.strategy_id,
            opportunity_id: draft.opportunity_id,
            execution_plan_id: draft.execution_plan_id,
            reversal_of: draft.reversal_of,
            adjustment_of: draft.adjustment_of,
            adjustment_reason_code: draft.adjustment_reason_code,
            legs: draft.legs,
            balance_assertion: BalanceAssertion {
                balanced: true,
                assertion_hash,
                checked_by: Some("arb-ledger".to_owned()),
            },
        })
    }

    fn validate_for_append(&self) -> LedgerResult<()> {
        if !self.balance_assertion.balanced {
            return Err(LedgerError::BalanceAssertionNotTrue {
                ledger_entry_id: self.ledger_entry_id.clone(),
            });
        }
        assert_legs_balanced(Some(&self.ledger_entry_id), &self.legs)?;
        validate_correction_metadata(
            &self.ledger_entry_id,
            self.entry_type,
            self.reversal_of.as_ref(),
            self.adjustment_of.as_ref(),
            self.adjustment_reason_code.as_ref(),
        )?;
        let expected = compute_assertion_hash(&EntryHashView::from_entry(self))?;
        if expected != self.balance_assertion.assertion_hash {
            return Err(LedgerError::BalanceAssertionHashMismatch {
                ledger_entry_id: self.ledger_entry_id.clone(),
                expected,
                actual: self.balance_assertion.assertion_hash.clone(),
            });
        }
        Ok(())
    }

    fn same_idempotent_effect(&self, other: &Self) -> bool {
        self.journal_entry_id == other.journal_entry_id
            && self.schema_version == other.schema_version
            && self.timestamp == other.timestamp
            && self.namespace == other.namespace
            && self.entry_type == other.entry_type
            && self.source_event_id == other.source_event_id
            && self.idempotency_key == other.idempotency_key
            && self.strategy_id == other.strategy_id
            && self.opportunity_id == other.opportunity_id
            && self.execution_plan_id == other.execution_plan_id
            && self.reversal_of == other.reversal_of
            && self.adjustment_of == other.adjustment_of
            && self.adjustment_reason_code == other.adjustment_reason_code
            && self.legs == other.legs
            && self.balance_assertion == other.balance_assertion
    }

    pub fn from_reversal_request(
        request: ReversalRequest,
        original: &LedgerEntry,
    ) -> LedgerResult<Self> {
        if request.original_entry_id != original.ledger_entry_id {
            return Err(LedgerError::ReferencedLedgerEntryNotFound {
                ledger_entry_id: request.original_entry_id,
            });
        }
        let legs = reversed_legs_for(&request.header.ledger_entry_id, &original.legs)?;
        let draft = LedgerEntryDraft::new(request.header, legs)
            .with_reversal_of(original.ledger_entry_id.clone());
        Self::from_draft(copy_original_attribution(draft, original))
    }

    pub fn from_adjustment_request(
        request: AdjustmentRequest,
        original: &LedgerEntry,
    ) -> LedgerResult<Self> {
        if request.original_entry_id != original.ledger_entry_id {
            return Err(LedgerError::ReferencedLedgerEntryNotFound {
                ledger_entry_id: request.original_entry_id,
            });
        }
        let mut draft = LedgerEntryDraft::new(request.header, request.legs)
            .with_adjustment_of(original.ledger_entry_id.clone())
            .with_adjustment_reason_code(request.reason_code);

        draft.strategy_id = request.strategy_id.or_else(|| original.strategy_id.clone());
        draft.opportunity_id = request
            .opportunity_id
            .or_else(|| original.opportunity_id.clone());
        draft.execution_plan_id = request
            .execution_plan_id
            .or_else(|| original.execution_plan_id.clone());

        Self::from_draft(draft)
    }
}

/// 冲销请求。
///
/// 中文说明：冲销由账本根据原分录反向生成腿，只追加新分录，不删除原分录。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReversalRequest {
    pub original_entry_id: LedgerEntryId,
    pub header: LedgerEntryHeader,
}

impl ReversalRequest {
    pub fn new(original_entry_id: LedgerEntryId, header: LedgerEntryHeader) -> Self {
        Self {
            original_entry_id,
            header,
        }
    }
}

/// 调整请求。
///
/// 中文说明：调整分录由调用方给出平衡腿，并必须关联原分录和原因码。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AdjustmentRequest {
    pub original_entry_id: LedgerEntryId,
    pub header: LedgerEntryHeader,
    pub reason_code: AdjustmentReasonCode,
    pub legs: Vec<LedgerLeg>,
    pub strategy_id: Option<StrategyId>,
    pub opportunity_id: Option<CandidateTransitionId>,
    pub execution_plan_id: Option<ExecutionPlanId>,
}

impl AdjustmentRequest {
    pub fn new(
        original_entry_id: LedgerEntryId,
        header: LedgerEntryHeader,
        reason_code: AdjustmentReasonCode,
        legs: Vec<LedgerLeg>,
    ) -> Self {
        Self {
            original_entry_id,
            header,
            reason_code,
            legs,
            strategy_id: None,
            opportunity_id: None,
            execution_plan_id: None,
        }
    }

    pub fn with_strategy_id(mut self, strategy_id: StrategyId) -> Self {
        self.strategy_id = Some(strategy_id);
        self
    }

    pub fn with_opportunity_id(mut self, opportunity_id: CandidateTransitionId) -> Self {
        self.opportunity_id = Some(opportunity_id);
        self
    }

    pub fn with_execution_plan_id(mut self, execution_plan_id: ExecutionPlanId) -> Self {
        self.execution_plan_id = Some(execution_plan_id);
        self
    }
}

/// 入账结果。
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AppendOutcome {
    Appended {
        ledger_entry_id: LedgerEntryId,
        sequence: usize,
    },
    Duplicate {
        ledger_entry_id: LedgerEntryId,
        sequence: usize,
    },
}

/// 追加式内存账本。
///
/// 中文说明：该类型没有更新或删除接口；余额视图每次都从分录重新推导。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct LedgerBook {
    entries: Vec<LedgerEntry>,
    entry_ids: BTreeSet<LedgerEntryId>,
    idempotency_index: BTreeMap<IdempotencyKey, usize>,
    reversal_index: BTreeMap<LedgerEntryId, usize>,
}

impl LedgerBook {
    pub fn append(&mut self, entry: LedgerEntry) -> LedgerResult<AppendOutcome> {
        entry.validate_for_append()?;

        if let Some(existing_index) = self.idempotency_index.get(&entry.idempotency_key) {
            let existing = &self.entries[*existing_index];
            if existing.same_idempotent_effect(&entry) {
                return Ok(AppendOutcome::Duplicate {
                    ledger_entry_id: existing.ledger_entry_id.clone(),
                    sequence: *existing_index,
                });
            }
            return Err(LedgerError::IdempotencyConflict {
                idempotency_key: entry.idempotency_key,
                existing_entry_id: existing.ledger_entry_id.clone(),
                attempted_entry_id: entry.ledger_entry_id,
            });
        }

        if self.entry_ids.contains(&entry.ledger_entry_id) {
            return Err(LedgerError::DuplicateLedgerEntryId {
                ledger_entry_id: entry.ledger_entry_id,
            });
        }

        self.validate_correction_references(&entry)?;

        let ledger_entry_id = entry.ledger_entry_id.clone();
        let idempotency_key = entry.idempotency_key.clone();
        let reversal_of = entry.reversal_of.clone();
        let sequence = self.entries.len();
        self.entries.push(entry);
        self.entry_ids.insert(ledger_entry_id.clone());
        self.idempotency_index.insert(idempotency_key, sequence);
        if let Some(reversal_of) = reversal_of {
            self.reversal_index.insert(reversal_of, sequence);
        }
        Ok(AppendOutcome::Appended {
            ledger_entry_id,
            sequence,
        })
    }

    pub fn append_reversal(&mut self, request: ReversalRequest) -> LedgerResult<AppendOutcome> {
        let original_entry_id = request.original_entry_id.clone();
        let entry = {
            let original = self.entry(&original_entry_id).ok_or_else(|| {
                LedgerError::ReferencedLedgerEntryNotFound {
                    ledger_entry_id: original_entry_id.clone(),
                }
            })?;
            LedgerEntry::from_reversal_request(request, original)?
        };
        self.append(entry)
    }

    pub fn append_adjustment(&mut self, request: AdjustmentRequest) -> LedgerResult<AppendOutcome> {
        let original_entry_id = request.original_entry_id.clone();
        let entry = {
            let original = self.entry(&original_entry_id).ok_or_else(|| {
                LedgerError::ReferencedLedgerEntryNotFound {
                    ledger_entry_id: original_entry_id.clone(),
                }
            })?;
            LedgerEntry::from_adjustment_request(request, original)?
        };
        self.append(entry)
    }

    pub fn entries(&self) -> &[LedgerEntry] {
        &self.entries
    }

    pub fn entry(&self, ledger_entry_id: &LedgerEntryId) -> Option<&LedgerEntry> {
        self.entries
            .iter()
            .find(|entry| &entry.ledger_entry_id == ledger_entry_id)
    }

    pub fn entry_count(&self) -> usize {
        self.entries.len()
    }

    pub fn balance_view(&self) -> LedgerResult<BalanceView> {
        BalanceView::from_entries(&self.entries)
    }

    pub fn entries_by_account(&self, account_id: &AccountId) -> Vec<&LedgerEntry> {
        self.entries
            .iter()
            .filter(|entry| entry.legs.iter().any(|leg| &leg.account_id == account_id))
            .collect()
    }

    pub fn entries_by_asset(&self, asset_id: &AssetId) -> Vec<&LedgerEntry> {
        self.entries
            .iter()
            .filter(|entry| entry.legs.iter().any(|leg| &leg.asset_id == asset_id))
            .collect()
    }

    pub fn entries_by_strategy(&self, strategy_id: &StrategyId) -> Vec<&LedgerEntry> {
        self.entries
            .iter()
            .filter(|entry| entry.strategy_id.as_ref() == Some(strategy_id))
            .collect()
    }

    pub fn entries_by_opportunity(
        &self,
        opportunity_id: &CandidateTransitionId,
    ) -> Vec<&LedgerEntry> {
        self.entries
            .iter()
            .filter(|entry| entry.opportunity_id.as_ref() == Some(opportunity_id))
            .collect()
    }

    pub fn entries_by_execution_plan(
        &self,
        execution_plan_id: &ExecutionPlanId,
    ) -> Vec<&LedgerEntry> {
        self.entries
            .iter()
            .filter(|entry| entry.execution_plan_id.as_ref() == Some(execution_plan_id))
            .collect()
    }

    pub fn correction_entries_for(&self, ledger_entry_id: &LedgerEntryId) -> Vec<&LedgerEntry> {
        self.entries
            .iter()
            .filter(|entry| {
                entry.reversal_of.as_ref() == Some(ledger_entry_id)
                    || entry.adjustment_of.as_ref() == Some(ledger_entry_id)
            })
            .collect()
    }

    pub fn correction_chain(
        &self,
        ledger_entry_id: &LedgerEntryId,
    ) -> LedgerResult<LedgerCorrectionChain> {
        if !self.entry_ids.contains(ledger_entry_id) {
            return Err(LedgerError::ReferencedLedgerEntryNotFound {
                ledger_entry_id: ledger_entry_id.clone(),
            });
        }

        let mut links = Vec::new();
        let mut seen = BTreeSet::new();
        let mut pending = vec![ledger_entry_id.clone()];
        while let Some(current_entry_id) = pending.pop() {
            if !seen.insert(current_entry_id.clone()) {
                continue;
            }

            for entry in &self.entries {
                let link = if entry.reversal_of.as_ref() == Some(&current_entry_id) {
                    Some(LedgerCorrectionLink {
                        kind: LedgerCorrectionKind::Reversal,
                        original_entry_id: current_entry_id.clone(),
                        correction_entry_id: entry.ledger_entry_id.clone(),
                        source_event_id: entry.source_event_id.clone(),
                        reason_code: None,
                    })
                } else if entry.adjustment_of.as_ref() == Some(&current_entry_id) {
                    Some(LedgerCorrectionLink {
                        kind: LedgerCorrectionKind::Adjustment,
                        original_entry_id: current_entry_id.clone(),
                        correction_entry_id: entry.ledger_entry_id.clone(),
                        source_event_id: entry.source_event_id.clone(),
                        reason_code: entry.adjustment_reason_code.clone(),
                    })
                } else {
                    None
                };

                if let Some(link) = link {
                    pending.push(entry.ledger_entry_id.clone());
                    links.push(link);
                }
            }
        }

        Ok(LedgerCorrectionChain {
            root_entry_id: ledger_entry_id.clone(),
            links,
        })
    }

    fn validate_correction_references(&self, entry: &LedgerEntry) -> LedgerResult<()> {
        if let Some(original_entry_id) = entry.reversal_of.as_ref() {
            if !self.entry_ids.contains(original_entry_id) {
                return Err(LedgerError::ReferencedLedgerEntryNotFound {
                    ledger_entry_id: original_entry_id.clone(),
                });
            }
            if let Some(existing_index) = self.reversal_index.get(original_entry_id) {
                let existing = &self.entries[*existing_index];
                return Err(LedgerError::DuplicateReversal {
                    original_entry_id: original_entry_id.clone(),
                    existing_reversal_entry_id: existing.ledger_entry_id.clone(),
                    attempted_reversal_entry_id: entry.ledger_entry_id.clone(),
                });
            }
        }

        if let Some(original_entry_id) = entry.adjustment_of.as_ref() {
            if !self.entry_ids.contains(original_entry_id) {
                return Err(LedgerError::ReferencedLedgerEntryNotFound {
                    ledger_entry_id: original_entry_id.clone(),
                });
            }
        }

        Ok(())
    }
}

/// 账本修正类型。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LedgerCorrectionKind {
    Reversal,
    Adjustment,
}

/// 账本修正审计链路节点。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LedgerCorrectionLink {
    pub kind: LedgerCorrectionKind,
    pub original_entry_id: LedgerEntryId,
    pub correction_entry_id: LedgerEntryId,
    pub source_event_id: EventId,
    pub reason_code: Option<AdjustmentReasonCode>,
}

/// 账本修正审计链。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LedgerCorrectionChain {
    pub root_entry_id: LedgerEntryId,
    pub links: Vec<LedgerCorrectionLink>,
}

/// 余额视图键。
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct BalanceKey {
    pub namespace: LedgerNamespace,
    pub account_id: AccountId,
    pub asset_id: AssetId,
}

/// 余额视图行。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BalanceRow {
    pub namespace: LedgerNamespace,
    pub account_id: AccountId,
    pub asset_id: AssetId,
    pub debit_total: Decimal,
    pub credit_total: Decimal,
    pub net_debit: Decimal,
}

/// 从账本分录推导出的余额视图。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BalanceView {
    rows: Vec<BalanceRow>,
}

impl BalanceView {
    pub fn from_entries(entries: &[LedgerEntry]) -> LedgerResult<Self> {
        let mut accumulators: BTreeMap<BalanceKey, BalanceAccumulator> = BTreeMap::new();
        for entry in entries {
            entry.validate_for_append()?;
            for leg in &entry.legs {
                let key = BalanceKey {
                    namespace: entry.namespace,
                    account_id: leg.account_id.clone(),
                    asset_id: leg.asset_id.clone(),
                };
                accumulators
                    .entry(key)
                    .or_insert_with(BalanceAccumulator::new)
                    .apply(leg.direction, leg.amount.as_decimal())?;
            }
        }

        let rows = accumulators
            .into_iter()
            .map(|(key, totals)| totals.into_row(key))
            .collect::<LedgerResult<Vec<_>>>()?;
        Ok(Self { rows })
    }

    pub fn rows(&self) -> &[BalanceRow] {
        &self.rows
    }

    pub fn find(
        &self,
        namespace: LedgerNamespace,
        account_id: &AccountId,
        asset_id: &AssetId,
    ) -> Option<&BalanceRow> {
        self.rows.iter().find(|row| {
            row.namespace == namespace && &row.account_id == account_id && &row.asset_id == asset_id
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct BalanceAccumulator {
    debit_total: Decimal,
    credit_total: Decimal,
}

impl BalanceAccumulator {
    fn new() -> Self {
        Self {
            debit_total: zero(),
            credit_total: zero(),
        }
    }

    fn apply(&mut self, direction: LedgerDirection, amount: Decimal) -> LedgerResult<()> {
        match direction {
            LedgerDirection::Debit => {
                self.debit_total = self.debit_total.checked_add(amount)?;
            }
            LedgerDirection::Credit => {
                self.credit_total = self.credit_total.checked_add(amount)?;
            }
        }
        Ok(())
    }

    fn into_row(self, key: BalanceKey) -> LedgerResult<BalanceRow> {
        let net_debit = self.debit_total.checked_sub(self.credit_total)?;
        Ok(BalanceRow {
            namespace: key.namespace,
            account_id: key.account_id,
            asset_id: key.asset_id,
            debit_total: self.debit_total,
            credit_total: self.credit_total,
            net_debit,
        })
    }
}

#[derive(Clone, Debug)]
struct EntryHashView<'a> {
    journal_entry_id: &'a JournalEntryId,
    schema_version: &'a str,
    timestamp: UtcTimestamp,
    namespace: LedgerNamespace,
    entry_type: LedgerEntryType,
    source_event_id: &'a EventId,
    idempotency_key: &'a IdempotencyKey,
    strategy_id: Option<&'a StrategyId>,
    opportunity_id: Option<&'a CandidateTransitionId>,
    execution_plan_id: Option<&'a ExecutionPlanId>,
    reversal_of: Option<&'a LedgerEntryId>,
    adjustment_of: Option<&'a LedgerEntryId>,
    adjustment_reason_code: Option<&'a AdjustmentReasonCode>,
    legs: &'a [LedgerLeg],
}

impl<'a> EntryHashView<'a> {
    fn from_draft(draft: &'a LedgerEntryDraft) -> Self {
        Self {
            journal_entry_id: &draft.journal_entry_id,
            schema_version: &draft.schema_version,
            timestamp: draft.timestamp,
            namespace: draft.namespace,
            entry_type: draft.entry_type,
            source_event_id: &draft.source_event_id,
            idempotency_key: &draft.idempotency_key,
            strategy_id: draft.strategy_id.as_ref(),
            opportunity_id: draft.opportunity_id.as_ref(),
            execution_plan_id: draft.execution_plan_id.as_ref(),
            reversal_of: draft.reversal_of.as_ref(),
            adjustment_of: draft.adjustment_of.as_ref(),
            adjustment_reason_code: draft.adjustment_reason_code.as_ref(),
            legs: &draft.legs,
        }
    }

    fn from_entry(entry: &'a LedgerEntry) -> Self {
        Self {
            journal_entry_id: &entry.journal_entry_id,
            schema_version: &entry.schema_version,
            timestamp: entry.timestamp,
            namespace: entry.namespace,
            entry_type: entry.entry_type,
            source_event_id: &entry.source_event_id,
            idempotency_key: &entry.idempotency_key,
            strategy_id: entry.strategy_id.as_ref(),
            opportunity_id: entry.opportunity_id.as_ref(),
            execution_plan_id: entry.execution_plan_id.as_ref(),
            reversal_of: entry.reversal_of.as_ref(),
            adjustment_of: entry.adjustment_of.as_ref(),
            adjustment_reason_code: entry.adjustment_reason_code.as_ref(),
            legs: &entry.legs,
        }
    }
}

fn copy_original_attribution(
    mut draft: LedgerEntryDraft,
    original: &LedgerEntry,
) -> LedgerEntryDraft {
    draft.strategy_id = original.strategy_id.clone();
    draft.opportunity_id = original.opportunity_id.clone();
    draft.execution_plan_id = original.execution_plan_id.clone();
    draft
}

fn reversed_legs_for(
    reversal_entry_id: &LedgerEntryId,
    original_legs: &[LedgerLeg],
) -> LedgerResult<Vec<LedgerLeg>> {
    original_legs
        .iter()
        .enumerate()
        .map(|(index, leg)| {
            let mut reversed = LedgerLeg::new(
                generated_reversal_leg_id(reversal_entry_id, index + 1)?,
                leg.account_id.clone(),
                leg.asset_id.clone(),
                opposite_direction(leg.direction),
                leg.amount,
            );
            reversed.custody_location_id = leg.custody_location_id.clone();
            reversed.valuation_usd = leg.valuation_usd;
            reversed.memo = Some(format!("reversal of {}", leg.leg_id));
            Ok(reversed)
        })
        .collect()
}

fn generated_reversal_leg_id(
    reversal_entry_id: &LedgerEntryId,
    index: usize,
) -> LedgerResult<LedgerLegId> {
    LedgerLegId::new(format!("{}:leg:{index}", reversal_entry_id.as_str()))
}

fn opposite_direction(direction: LedgerDirection) -> LedgerDirection {
    match direction {
        LedgerDirection::Debit => LedgerDirection::Credit,
        LedgerDirection::Credit => LedgerDirection::Debit,
    }
}

fn validate_correction_metadata(
    ledger_entry_id: &LedgerEntryId,
    entry_type: LedgerEntryType,
    reversal_of: Option<&LedgerEntryId>,
    adjustment_of: Option<&LedgerEntryId>,
    adjustment_reason_code: Option<&AdjustmentReasonCode>,
) -> LedgerResult<()> {
    if reversal_of.is_some() && adjustment_of.is_some() {
        return Err(LedgerError::ConflictingCorrectionLinks {
            ledger_entry_id: ledger_entry_id.clone(),
        });
    }

    if let Some(_adjustment_of) = adjustment_of {
        if adjustment_reason_code.is_none() {
            return Err(LedgerError::MissingAdjustmentReasonCode {
                ledger_entry_id: ledger_entry_id.clone(),
            });
        }
        if !entry_type.is_adjustment() {
            return Err(LedgerError::AdjustmentRequiresAdjustmentType {
                ledger_entry_id: ledger_entry_id.clone(),
                entry_type,
            });
        }
    } else if adjustment_reason_code.is_some() {
        return Err(LedgerError::UnexpectedAdjustmentReasonCode {
            ledger_entry_id: ledger_entry_id.clone(),
        });
    } else if entry_type.is_adjustment() && reversal_of.is_none() {
        return Err(LedgerError::AdjustmentEntryRequiresTarget {
            ledger_entry_id: ledger_entry_id.clone(),
        });
    }

    Ok(())
}

fn assert_legs_balanced(
    ledger_entry_id: Option<&LedgerEntryId>,
    legs: &[LedgerLeg],
) -> LedgerResult<()> {
    if legs.len() < 2 {
        return Err(LedgerError::EntryHasTooFewLegs {
            ledger_entry_id: ledger_entry_id.cloned(),
            leg_count: legs.len(),
        });
    }

    let mut totals: BTreeMap<AssetId, BalanceAccumulator> = BTreeMap::new();
    for leg in legs {
        totals
            .entry(leg.asset_id.clone())
            .or_insert_with(BalanceAccumulator::new)
            .apply(leg.direction, leg.amount.as_decimal())?;
    }

    for (asset_id, totals) in totals {
        if totals
            .debit_total
            .checked_sub(totals.credit_total)?
            .partial_cmp(&zero())
            != Some(Ordering::Equal)
        {
            return Err(LedgerError::UnbalancedEntry {
                asset_id,
                debit_total: totals.debit_total,
                credit_total: totals.credit_total,
            });
        }
    }

    Ok(())
}

fn compute_assertion_hash(view: &EntryHashView<'_>) -> LedgerResult<BalanceAssertionHash> {
    let mut hash = Fnv1a64::new();
    hash.feed(view.journal_entry_id.as_str());
    hash.feed(view.schema_version);
    hash.feed(&view.timestamp.to_string());
    hash.feed(view.namespace.as_str());
    hash.feed(view.entry_type.as_str());
    hash.feed(view.source_event_id.as_str());
    hash.feed(view.idempotency_key.as_str());
    feed_optional(&mut hash, view.strategy_id.map(StrategyId::as_str));
    feed_optional(
        &mut hash,
        view.opportunity_id.map(CandidateTransitionId::as_str),
    );
    feed_optional(
        &mut hash,
        view.execution_plan_id.map(ExecutionPlanId::as_str),
    );
    feed_optional(&mut hash, view.reversal_of.map(LedgerEntryId::as_str));
    feed_optional(&mut hash, view.adjustment_of.map(LedgerEntryId::as_str));
    feed_optional(
        &mut hash,
        view.adjustment_reason_code
            .map(AdjustmentReasonCode::as_str),
    );
    for leg in view.legs {
        hash.feed(leg.leg_id.as_str());
        hash.feed(leg.account_id.as_str());
        feed_optional(
            &mut hash,
            leg.custody_location_id.as_ref().map(AccountId::as_str),
        );
        hash.feed(leg.asset_id.as_str());
        hash.feed(leg.direction.as_str());
        hash.feed(&leg.amount.to_string());
        feed_optional(
            &mut hash,
            leg.valuation_usd
                .as_ref()
                .map(|valuation_usd| valuation_usd.to_string())
                .as_deref(),
        );
        feed_optional(&mut hash, leg.memo.as_deref());
    }
    BalanceAssertionHash::new(format!("ledger-balance:{:016x}", hash.finish()))
}

fn feed_optional(hash: &mut Fnv1a64, value: Option<&str>) {
    match value {
        Some(value) => {
            hash.feed("some");
            hash.feed(value);
        }
        None => hash.feed("none"),
    }
}

#[derive(Clone, Copy, Debug)]
struct Fnv1a64(u64);

impl Fnv1a64 {
    fn new() -> Self {
        Self(0xcbf2_9ce4_8422_2325)
    }

    fn feed(&mut self, value: &str) {
        for byte in value.bytes().chain([0xff]) {
            self.0 ^= u64::from(byte);
            self.0 = self.0.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }

    fn finish(self) -> u64 {
        self.0
    }
}

fn validate_identifier(type_name: &'static str, value: &str) -> LedgerResult<()> {
    let bytes = value.as_bytes();
    if !(2..=128).contains(&bytes.len()) {
        return Err(LedgerError::InvalidIdentifier {
            type_name,
            value: value.to_owned(),
            reason: "length must be in 2..=128 bytes",
        });
    }
    if !bytes[0].is_ascii_alphanumeric() {
        return Err(LedgerError::InvalidIdentifier {
            type_name,
            value: value.to_owned(),
            reason: "first byte must be an ASCII letter or digit",
        });
    }
    if bytes
        .iter()
        .skip(1)
        .any(|byte| !(byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.' | b':' | b'-')))
    {
        return Err(LedgerError::InvalidIdentifier {
            type_name,
            value: value.to_owned(),
            reason: "only ASCII letters, digits, underscore, dot, colon and dash are allowed",
        });
    }
    Ok(())
}

fn validate_hash(value: &str) -> LedgerResult<()> {
    let bytes = value.as_bytes();
    if !(16..=160).contains(&bytes.len()) {
        return Err(LedgerError::InvalidHash {
            value: value.to_owned(),
            reason: "length must be in 16..=160 bytes",
        });
    }
    if bytes.iter().any(|byte| {
        !(byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b':' | b'+' | b'.' | b'-'))
    }) {
        return Err(LedgerError::InvalidHash {
            value: value.to_owned(),
            reason: "contains a byte outside the schema HashString alphabet",
        });
    }
    Ok(())
}

fn validate_reason_code(value: &str) -> LedgerResult<()> {
    let bytes = value.as_bytes();
    if !(2..=64).contains(&bytes.len()) {
        return Err(LedgerError::InvalidReasonCode {
            value: value.to_owned(),
            reason: "length must be in 2..=64 bytes",
        });
    }
    if !bytes[0].is_ascii_uppercase() {
        return Err(LedgerError::InvalidReasonCode {
            value: value.to_owned(),
            reason: "first byte must be A-Z",
        });
    }
    if bytes
        .iter()
        .any(|byte| !(byte.is_ascii_uppercase() || byte.is_ascii_digit() || *byte == b'_'))
    {
        return Err(LedgerError::InvalidReasonCode {
            value: value.to_owned(),
            reason: "must match [A-Z][A-Z0-9_]+",
        });
    }
    Ok(())
}

fn zero() -> Decimal {
    Decimal::from_scaled_atoms(0, 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    #[test]
    fn balanced_entry_appends_and_balance_view_is_derived() {
        let mut book = LedgerBook::default();
        let entry = demo_entry("ledger:01", "idem:ledger:01", "100.00");

        let outcome = book.append(entry).expect("balanced entry appends");

        assert_eq!(
            outcome,
            AppendOutcome::Appended {
                ledger_entry_id: ledger_entry_id("ledger:01"),
                sequence: 0,
            }
        );
        assert_eq!(book.entry_count(), 1);

        let view = book.balance_view().expect("view derives from entries");
        let cash = view
            .find(
                LedgerNamespace::Simulation,
                &make_account_id("acct:cash"),
                &make_asset_id("asset:USDC"),
            )
            .expect("cash row exists");
        assert_eq!(cash.debit_total.to_string(), "100.00");
        assert_eq!(cash.credit_total.to_string(), "0");
        assert_eq!(cash.net_debit.to_string(), "100.00");

        let trade_clearing = view
            .find(
                LedgerNamespace::Simulation,
                &make_account_id("acct:trade-clearing"),
                &make_asset_id("asset:USDC"),
            )
            .expect("clearing row exists");
        assert_eq!(trade_clearing.debit_total.to_string(), "0");
        assert_eq!(trade_clearing.credit_total.to_string(), "100.00");
        assert_eq!(trade_clearing.net_debit.to_string(), "-100.00");
    }

    #[test]
    fn unbalanced_entry_is_rejected() {
        let draft = demo_draft("ledger:02", "idem:ledger:02", "100.00", "99.99");
        let error = LedgerEntry::from_draft(draft).expect_err("unbalanced draft fails");

        assert!(matches!(
            error,
            LedgerError::UnbalancedEntry {
                asset_id,
                debit_total,
                credit_total,
            } if asset_id == make_asset_id("asset:USDC")
                && debit_total.to_string() == "100.00"
                && credit_total.to_string() == "99.99"
        ));
    }

    #[test]
    fn duplicate_event_is_idempotent() {
        let mut book = LedgerBook::default();
        let first = demo_entry("ledger:03", "idem:duplicate-event", "10.0000");
        let duplicate_retry = demo_entry("ledger:retry-03", "idem:duplicate-event", "10.0000");

        assert!(matches!(
            book.append(first).expect("first append"),
            AppendOutcome::Appended { sequence: 0, .. }
        ));
        let duplicate = book
            .append(duplicate_retry)
            .expect("same logical event is idempotent");

        assert_eq!(
            duplicate,
            AppendOutcome::Duplicate {
                ledger_entry_id: ledger_entry_id("ledger:03"),
                sequence: 0,
            }
        );
        assert_eq!(book.entry_count(), 1);
    }

    #[test]
    fn idempotency_conflict_fails_closed() {
        let mut book = LedgerBook::default();
        let first = demo_entry("ledger:04", "idem:conflict", "10.00");
        let conflicting_retry = demo_entry("ledger:retry-04", "idem:conflict", "10.01");

        book.append(first).expect("first append");
        let error = book
            .append(conflicting_retry)
            .expect_err("same key with different economics fails");

        assert!(matches!(
            error,
            LedgerError::IdempotencyConflict {
                idempotency_key,
                existing_entry_id,
                attempted_entry_id,
            } if idempotency_key == make_idempotency_key("idem:conflict")
                && existing_entry_id == ledger_entry_id("ledger:04")
                && attempted_entry_id == ledger_entry_id("ledger:retry-04")
        ));
        assert_eq!(book.entry_count(), 1);
    }

    #[test]
    fn decimal_amount_precision_is_not_lost() {
        let mut book = LedgerBook::default();
        let precise = "0.000000000000000001";
        let entry = demo_entry("ledger:05", "idem:precision", precise);

        book.append(entry).expect("precise amount appends");

        let view = book.balance_view().expect("view derives");
        let cash = view
            .find(
                LedgerNamespace::Simulation,
                &make_account_id("acct:cash"),
                &make_asset_id("asset:USDC"),
            )
            .expect("cash row exists");
        assert_eq!(cash.debit_total.to_string(), precise);
        assert_eq!(cash.net_debit.to_string(), precise);
    }

    #[test]
    fn namespaces_are_isolated_in_balance_view() {
        let mut book = LedgerBook::default();
        book.append(demo_entry("ledger:06", "idem:sim", "1.00"))
            .expect("simulation append");
        let live = demo_draft("ledger:07", "idem:live", "2.00", "2.00");
        let live = LedgerEntry::from_draft(LedgerEntryDraft {
            namespace: LedgerNamespace::Live,
            ..live
        })
        .expect("live entry");
        book.append(live).expect("live append");

        let view = book.balance_view().expect("view derives");
        let simulation = view
            .find(
                LedgerNamespace::Simulation,
                &make_account_id("acct:cash"),
                &make_asset_id("asset:USDC"),
            )
            .expect("simulation row");
        let live = view
            .find(
                LedgerNamespace::Live,
                &make_account_id("acct:cash"),
                &make_asset_id("asset:USDC"),
            )
            .expect("live row");

        assert_eq!(simulation.net_debit.to_string(), "1.00");
        assert_eq!(live.net_debit.to_string(), "2.00");
    }

    #[test]
    fn query_interfaces_find_entries_by_attribution() {
        let mut book = LedgerBook::default();
        book.append(demo_entry("ledger:query", "idem:query", "3.50"))
            .expect("entry appends");

        assert_eq!(
            entry_ids(book.entries_by_account(&make_account_id("acct:cash"))),
            vec![ledger_entry_id("ledger:query")]
        );
        assert_eq!(
            entry_ids(book.entries_by_asset(&make_asset_id("asset:USDC"))),
            vec![ledger_entry_id("ledger:query")]
        );
        assert_eq!(
            entry_ids(
                book.entries_by_strategy(&StrategyId::new("strat:demo").expect("strategy id"))
            ),
            vec![ledger_entry_id("ledger:query")]
        );
        assert_eq!(
            entry_ids(book.entries_by_opportunity(
                &CandidateTransitionId::new("transition:demo").expect("transition id")
            )),
            vec![ledger_entry_id("ledger:query")]
        );
        assert_eq!(
            entry_ids(
                book.entries_by_execution_plan(
                    &ExecutionPlanId::new("plan:demo").expect("plan id")
                )
            ),
            vec![ledger_entry_id("ledger:query")]
        );
    }

    #[test]
    fn reversal_appends_without_deleting_original_and_offsets_balance() {
        let mut book = LedgerBook::default();
        let original = demo_entry("ledger:08", "idem:original-reversal", "42.00");
        let original_snapshot = original.clone();

        book.append(original).expect("original append");
        let outcome = book
            .append_reversal(ReversalRequest::new(
                ledger_entry_id("ledger:08"),
                demo_header(
                    "ledger:08-reversal",
                    "journal:reversal",
                    LedgerNamespace::Simulation,
                    LedgerEntryType::TradeFill,
                    "event:reversal:08",
                    "idem:reversal:08",
                ),
            ))
            .expect("reversal appends");

        assert_eq!(
            outcome,
            AppendOutcome::Appended {
                ledger_entry_id: ledger_entry_id("ledger:08-reversal"),
                sequence: 1,
            }
        );
        assert_eq!(book.entry_count(), 2);
        assert_eq!(
            book.entry(&ledger_entry_id("ledger:08")),
            Some(&original_snapshot)
        );

        let reversal = book
            .entry(&ledger_entry_id("ledger:08-reversal"))
            .expect("reversal is stored");
        assert_eq!(reversal.reversal_of, Some(ledger_entry_id("ledger:08")));
        assert_eq!(reversal.adjustment_of, None);
        assert_eq!(reversal.legs[0].direction, LedgerDirection::Credit);
        assert_eq!(reversal.legs[1].direction, LedgerDirection::Debit);
        assert!(reversal.balance_assertion.balanced);

        let chain = book
            .correction_chain(&ledger_entry_id("ledger:08"))
            .expect("audit chain exists");
        assert_eq!(
            chain.links,
            vec![LedgerCorrectionLink {
                kind: LedgerCorrectionKind::Reversal,
                original_entry_id: ledger_entry_id("ledger:08"),
                correction_entry_id: ledger_entry_id("ledger:08-reversal"),
                source_event_id: EventId::new("event:reversal:08").expect("event id"),
                reason_code: None,
            }]
        );

        let view = book.balance_view().expect("view remains valid");
        let cash = view
            .find(
                LedgerNamespace::Simulation,
                &make_account_id("acct:cash"),
                &make_asset_id("asset:USDC"),
            )
            .expect("cash row exists");
        let trade_clearing = view
            .find(
                LedgerNamespace::Simulation,
                &make_account_id("acct:trade-clearing"),
                &make_asset_id("asset:USDC"),
            )
            .expect("clearing row exists");
        assert_eq!(cash.net_debit.to_string(), "0.00");
        assert_eq!(trade_clearing.net_debit.to_string(), "0.00");
    }

    #[test]
    fn adjustment_preserves_reason_code_source_event_and_audit_link() {
        let mut book = LedgerBook::default();
        let original = demo_entry("ledger:09", "idem:original-adjustment", "10.00");
        let original_strategy = original.strategy_id.clone();
        let original_opportunity = original.opportunity_id.clone();
        let original_plan = original.execution_plan_id.clone();

        book.append(original).expect("original append");
        let reason_code = adjustment_reason_code("FEE_RECLASS");
        book.append_adjustment(AdjustmentRequest::new(
            ledger_entry_id("ledger:09"),
            demo_header(
                "ledger:09-adjustment",
                "journal:adjustment",
                LedgerNamespace::Adjustment,
                LedgerEntryType::ManualAdjustment,
                "event:adjustment:09",
                "idem:adjustment:09",
            ),
            reason_code.clone(),
            adjustment_legs("1.25"),
        ))
        .expect("adjustment appends");

        let adjustment = book
            .entry(&ledger_entry_id("ledger:09-adjustment"))
            .expect("adjustment is stored");
        assert_eq!(adjustment.adjustment_of, Some(ledger_entry_id("ledger:09")));
        assert_eq!(adjustment.reversal_of, None);
        assert_eq!(adjustment.adjustment_reason_code, Some(reason_code.clone()));
        assert_eq!(
            adjustment.source_event_id,
            EventId::new("event:adjustment:09").expect("event id")
        );
        assert_eq!(adjustment.strategy_id, original_strategy);
        assert_eq!(adjustment.opportunity_id, original_opportunity);
        assert_eq!(adjustment.execution_plan_id, original_plan);
        assert!(adjustment.balance_assertion.balanced);

        let chain = book
            .correction_chain(&ledger_entry_id("ledger:09"))
            .expect("audit chain exists");
        assert_eq!(
            chain.links,
            vec![LedgerCorrectionLink {
                kind: LedgerCorrectionKind::Adjustment,
                original_entry_id: ledger_entry_id("ledger:09"),
                correction_entry_id: ledger_entry_id("ledger:09-adjustment"),
                source_event_id: EventId::new("event:adjustment:09").expect("event id"),
                reason_code: Some(reason_code),
            }]
        );

        assert!(book.balance_view().is_ok());
    }

    #[test]
    fn adjustment_without_reason_code_is_rejected() {
        let draft = LedgerEntryDraft::new(
            demo_header(
                "ledger:10-adjustment",
                "journal:adjustment",
                LedgerNamespace::Adjustment,
                LedgerEntryType::ManualAdjustment,
                "event:adjustment:10",
                "idem:adjustment:10",
            ),
            adjustment_legs("0.01"),
        )
        .with_adjustment_of(ledger_entry_id("ledger:10"));

        let error = LedgerEntry::from_draft(draft).expect_err("reason code is required");

        assert!(matches!(
            error,
            LedgerError::MissingAdjustmentReasonCode {
                ledger_entry_id: missing_reason_entry_id
            } if missing_reason_entry_id == ledger_entry_id("ledger:10-adjustment")
        ));
    }

    #[test]
    fn correction_reference_must_exist() {
        let mut book = LedgerBook::default();
        let error = book
            .append_reversal(ReversalRequest::new(
                ledger_entry_id("ledger:missing"),
                demo_header(
                    "ledger:missing-reversal",
                    "journal:reversal",
                    LedgerNamespace::Simulation,
                    LedgerEntryType::TradeFill,
                    "event:reversal:missing",
                    "idem:reversal:missing",
                ),
            ))
            .expect_err("missing original fails closed");

        assert!(matches!(
            error,
            LedgerError::ReferencedLedgerEntryNotFound {
                ledger_entry_id: missing_entry_id
            } if missing_entry_id == ledger_entry_id("ledger:missing")
        ));
    }

    #[test]
    fn chart_of_accounts_rejects_duplicate_accounts() {
        let mut chart = ChartOfAccounts::default();
        let account = LedgerAccount::new(make_account_id("acct:cash"), AccountKind::Asset, "Cash")
            .with_asset(make_asset_id("asset:USDC"));

        chart.insert(account.clone()).expect("first insert");
        let error = chart.insert(account).expect_err("duplicate account fails");

        assert!(matches!(
            error,
            LedgerError::DuplicateAccount { account_id } if account_id == make_account_id("acct:cash")
        ));
    }

    fn demo_entry(entry_id: &str, idempotency: &str, amount: &str) -> LedgerEntry {
        LedgerEntry::from_draft(demo_draft(entry_id, idempotency, amount, amount))
            .expect("demo entry balances")
    }

    fn demo_draft(
        entry_id: &str,
        idempotency: &str,
        debit_amount: &str,
        credit_amount: &str,
    ) -> LedgerEntryDraft {
        let header = demo_header(
            entry_id,
            "journal:demo",
            LedgerNamespace::Simulation,
            LedgerEntryType::TradeFill,
            "event:fill:01",
            idempotency,
        );
        LedgerEntryDraft::new(
            header,
            vec![
                LedgerLeg::new(
                    ledger_leg_id("leg:debit"),
                    make_account_id("acct:cash"),
                    make_asset_id("asset:USDC"),
                    LedgerDirection::Debit,
                    amount(debit_amount),
                ),
                LedgerLeg::new(
                    ledger_leg_id("leg:credit"),
                    make_account_id("acct:trade-clearing"),
                    make_asset_id("asset:USDC"),
                    LedgerDirection::Credit,
                    amount(credit_amount),
                ),
            ],
        )
        .with_strategy_id(StrategyId::new("strat:demo").expect("strategy id"))
        .with_opportunity_id(CandidateTransitionId::new("transition:demo").expect("transition id"))
        .with_execution_plan_id(ExecutionPlanId::new("plan:demo").expect("plan id"))
    }

    fn demo_header(
        entry_id: &str,
        journal_id: &str,
        namespace: LedgerNamespace,
        entry_type: LedgerEntryType,
        source_event_id: &str,
        idempotency: &str,
    ) -> LedgerEntryHeader {
        LedgerEntryHeader::new(
            ledger_entry_id(entry_id),
            journal_entry_id(journal_id),
            UtcTimestamp::parse_rfc3339_z("2026-05-10T12:00:00Z").expect("timestamp"),
            namespace,
            entry_type,
            EventId::new(source_event_id).expect("event id"),
            make_idempotency_key(idempotency),
        )
    }

    fn adjustment_legs(value: &str) -> Vec<LedgerLeg> {
        vec![
            LedgerLeg::new(
                ledger_leg_id("leg:adjustment-debit"),
                make_account_id("acct:cash"),
                make_asset_id("asset:USDC"),
                LedgerDirection::Debit,
                amount(value),
            ),
            LedgerLeg::new(
                ledger_leg_id("leg:adjustment-credit"),
                make_account_id("acct:adjustment-clearing"),
                make_asset_id("asset:USDC"),
                LedgerDirection::Credit,
                amount(value),
            ),
        ]
    }

    fn ledger_entry_id(value: &str) -> LedgerEntryId {
        LedgerEntryId::new(value).expect("ledger entry id")
    }

    fn journal_entry_id(value: &str) -> JournalEntryId {
        JournalEntryId::new(value).expect("journal entry id")
    }

    fn ledger_leg_id(value: &str) -> LedgerLegId {
        LedgerLegId::new(value).expect("ledger leg id")
    }

    fn make_idempotency_key(value: &str) -> IdempotencyKey {
        IdempotencyKey::new(value).expect("idempotency key")
    }

    fn make_account_id(value: &str) -> AccountId {
        AccountId::new(value).expect("account id")
    }

    fn make_asset_id(value: &str) -> AssetId {
        AssetId::new(value).expect("asset id")
    }

    fn adjustment_reason_code(value: &str) -> AdjustmentReasonCode {
        AdjustmentReasonCode::new(value).expect("adjustment reason code")
    }

    fn amount(value: &str) -> Amount {
        Amount::from_str(value).expect("amount")
    }

    fn entry_ids(entries: Vec<&LedgerEntry>) -> Vec<LedgerEntryId> {
        entries
            .into_iter()
            .map(|entry| entry.ledger_entry_id.clone())
            .collect()
    }
}
