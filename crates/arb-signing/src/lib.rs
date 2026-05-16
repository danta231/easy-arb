//! `arb-signing` 签名边界和空签名器。
//!
//! 中文说明：本 crate 只定义受控签名请求、签名策略、审计引用和默认拒绝的
//! 空签名器。默认 feature 为空，不连接真实密钥、不保存明文密钥、不输出
//! 明文签名材料。`real-signing` feature 显式开启后才暴露真实 HMAC 签名边界；
//! 真实凭证仍只能从环境变量或调用方实现的外部 secret provider 读取。
//!
//! 中文说明：默认 feature 下 `arb_signing::real` 模块不存在；需要显式开启
//! `real-signing` feature 后才能使用真实签名 provider。

#![forbid(unsafe_code)]

use std::collections::BTreeSet;
use std::error::Error;
use std::fmt;

use arb_domain::{AccountId, CausationId, CorrelationId, EventId, VenueId};

/// 签名模块统一返回类型。
pub type SigningResult<T> = Result<T, SigningError>;

/// 真实签名 feature 是否已显式开启。
///
/// 中文说明：默认构建必须返回 `false`。阶段 11 没有真实签名实现；即使未来
/// feature 打开，也必须继续受运行时配置、审批、熔断和外部签名器治理约束。
pub const REAL_SIGNING_FEATURE_ENABLED: bool = cfg!(feature = "real-signing");

/// 签名边界错误。
///
/// 中文说明：错误对象不保存调用方传入的可疑原文，避免错误日志泄露密钥、
/// payload 或签名材料。签名失败必须以 `Err` 返回，不能被解释成成功。
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SigningError {
    InvalidToken {
        field: &'static str,
        reason: &'static str,
    },
    InvalidDigest {
        field: &'static str,
        reason: &'static str,
    },
    InvalidRequest {
        field: &'static str,
        reason: &'static str,
    },
    PolicyMismatch {
        audit_ref: SigningAuditRef,
        expected_policy: RedactedValue,
        actual_policy: RedactedValue,
    },
    PurposeNotAllowed {
        audit_ref: SigningAuditRef,
        purpose: SigningPurpose,
    },
    ApprovalRequired {
        audit_ref: SigningAuditRef,
    },
    PolicyDisabled {
        audit_ref: SigningAuditRef,
    },
    RealSigningUnavailable {
        audit_ref: SigningAuditRef,
    },
    RealSigningPolicyNotEnabled {
        audit_ref: SigningAuditRef,
    },
    SecretUnavailable {
        audit_ref: SigningAuditRef,
        reason: &'static str,
    },
    ClockUnavailable {
        audit_ref: SigningAuditRef,
    },
}

impl SigningError {
    /// 返回稳定失败码，供结构化日志和报告使用。
    pub fn code(&self) -> SigningFailureCode {
        match self {
            Self::InvalidToken { .. } => SigningFailureCode::InvalidToken,
            Self::InvalidDigest { .. } => SigningFailureCode::InvalidDigest,
            Self::InvalidRequest { .. } => SigningFailureCode::InvalidRequest,
            Self::PolicyMismatch { .. } => SigningFailureCode::PolicyMismatch,
            Self::PurposeNotAllowed { .. } => SigningFailureCode::PurposeNotAllowed,
            Self::ApprovalRequired { .. } => SigningFailureCode::ApprovalRequired,
            Self::PolicyDisabled { .. } => SigningFailureCode::PolicyDisabled,
            Self::RealSigningUnavailable { .. } => SigningFailureCode::RealSigningUnavailable,
            Self::RealSigningPolicyNotEnabled { .. } => {
                SigningFailureCode::RealSigningPolicyNotEnabled
            }
            Self::SecretUnavailable { .. } => SigningFailureCode::SecretUnavailable,
            Self::ClockUnavailable { .. } => SigningFailureCode::ClockUnavailable,
        }
    }

    /// 返回可审计引用；创建请求前的格式错误没有审计引用。
    pub fn audit_ref(&self) -> Option<&SigningAuditRef> {
        match self {
            Self::PolicyMismatch { audit_ref, .. }
            | Self::PurposeNotAllowed { audit_ref, .. }
            | Self::ApprovalRequired { audit_ref }
            | Self::PolicyDisabled { audit_ref }
            | Self::RealSigningUnavailable { audit_ref }
            | Self::RealSigningPolicyNotEnabled { audit_ref }
            | Self::SecretUnavailable { audit_ref, .. }
            | Self::ClockUnavailable { audit_ref } => Some(audit_ref),
            Self::InvalidToken { .. }
            | Self::InvalidDigest { .. }
            | Self::InvalidRequest { .. } => None,
        }
    }

    fn attempt_status(&self) -> SigningAttemptStatus {
        match self {
            Self::RealSigningUnavailable { .. }
            | Self::SecretUnavailable { .. }
            | Self::ClockUnavailable { .. } => SigningAttemptStatus::Unavailable,
            _ => SigningAttemptStatus::Rejected,
        }
    }
}

impl fmt::Display for SigningError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidToken { field, reason } => {
                write!(f, "{field}: invalid signing token: {reason}")
            }
            Self::InvalidDigest { field, reason } => {
                write!(f, "{field}: invalid signing payload digest: {reason}")
            }
            Self::InvalidRequest { field, reason } => {
                write!(f, "{field}: invalid signing request: {reason}")
            }
            Self::PolicyMismatch {
                audit_ref,
                expected_policy,
                actual_policy,
            } => write!(
                f,
                "signing policy mismatch for audit_ref `{audit_ref}`: expected {expected_policy}, got {actual_policy}"
            ),
            Self::PurposeNotAllowed { audit_ref, purpose } => write!(
                f,
                "signing purpose `{purpose}` is not allowed for audit_ref `{audit_ref}`"
            ),
            Self::ApprovalRequired { audit_ref } => {
                write!(f, "manual approval reference is required for audit_ref `{audit_ref}`")
            }
            Self::PolicyDisabled { audit_ref } => {
                write!(f, "signing policy is disabled for audit_ref `{audit_ref}`")
            }
            Self::RealSigningUnavailable { audit_ref } => write!(
                f,
                "real signing is unavailable for audit_ref `{audit_ref}` in this build"
            ),
            Self::RealSigningPolicyNotEnabled { audit_ref } => write!(
                f,
                "real signing policy is not enabled for audit_ref `{audit_ref}`"
            ),
            Self::SecretUnavailable { audit_ref, reason } => write!(
                f,
                "signing secret source is unavailable for audit_ref `{audit_ref}`: {reason}"
            ),
            Self::ClockUnavailable { audit_ref } => write!(
                f,
                "signing timestamp source is unavailable for audit_ref `{audit_ref}`"
            ),
        }
    }
}

impl Error for SigningError {}

macro_rules! token_type {
    ($name:ident, $field:literal, $doc:literal, $prefixes:expr) => {
        #[doc = $doc]
        ///
        /// 中文说明：签名边界标识只保存稳定 ASCII 引用，不保存密钥、payload
        /// 或签名原文。
        #[derive(Clone, Eq, Hash, Ord, PartialEq, PartialOrd)]
        pub struct $name(String);

        impl $name {
            pub fn new(value: impl Into<String>) -> SigningResult<Self> {
                let value = value.into();
                validate_token($field, &value, $prefixes)?;
                Ok(Self(value))
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }

            pub fn redacted(&self) -> RedactedValue {
                RedactedValue::from_reference(&self.0)
            }
        }

        impl fmt::Debug for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.debug_tuple(stringify!($name))
                    .field(&self.redacted())
                    .finish()
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }
    };
}

token_type!(
    SigningRequestId,
    "signing_request_id",
    "签名请求 ID。",
    &["signing-request/"]
);
token_type!(
    SigningPolicyRef,
    "signing_policy_ref",
    "签名策略引用。",
    &[
        "signing-policy/",
        "mock-policy/",
        "hardware-policy/",
        "kms-policy/"
    ]
);
token_type!(
    SigningApprovalRef,
    "signing_approval_ref",
    "人工审批引用。",
    &["approval/", "manual-approval/"]
);
token_type!(
    SigningAuditRef,
    "signing_audit_ref",
    "签名审计引用。",
    &["signing-audit/"]
);
token_type!(
    SignatureRef,
    "signature_ref",
    "外部签名结果引用。",
    &["signature-ref/"]
);

fn validate_token(
    field: &'static str,
    value: &str,
    allowed_prefixes: &[&str],
) -> SigningResult<()> {
    if value.is_empty() {
        return Err(SigningError::InvalidToken {
            field,
            reason: "value cannot be empty",
        });
    }

    if value.len() > 160 {
        return Err(SigningError::InvalidToken {
            field,
            reason: "value is too long for a boundary reference",
        });
    }

    if value.bytes().any(|byte| {
        !(byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.' | b':' | b'/'))
    }) {
        return Err(SigningError::InvalidToken {
            field,
            reason:
                "only ASCII letters, digits, underscore, dash, dot, colon and slash are allowed",
        });
    }

    if !allowed_prefixes.is_empty()
        && !allowed_prefixes
            .iter()
            .any(|prefix| value.starts_with(prefix))
    {
        return Err(SigningError::InvalidToken {
            field,
            reason: "unexpected reference prefix",
        });
    }

    if looks_like_secret_label(value) {
        return Err(SigningError::InvalidToken {
            field,
            reason: "reference must not look like key material",
        });
    }

    Ok(())
}

fn looks_like_secret_label(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    [
        "api_key",
        "apikey",
        "mnemonic",
        "password",
        "private",
        "secret",
        "sensitive",
    ]
    .iter()
    .any(|marker| lower.contains(marker))
}

/// 签名 payload 摘要。
///
/// 中文说明：签名请求只接收规范化 payload 的 `sha256` 摘要，不接收、保存或
/// 输出原始待签名内容。
#[derive(Clone, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct SigningPayloadDigest(String);

impl SigningPayloadDigest {
    pub fn new(value: impl Into<String>) -> SigningResult<Self> {
        let value = value.into();
        let Some(hex) = value.strip_prefix("sha256:") else {
            return Err(SigningError::InvalidDigest {
                field: "payload_digest",
                reason: "digest must start with sha256:",
            });
        };

        if hex.len() != 64 {
            return Err(SigningError::InvalidDigest {
                field: "payload_digest",
                reason: "sha256 digest must contain exactly 64 hex characters",
            });
        }

        if !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(SigningError::InvalidDigest {
                field: "payload_digest",
                reason: "sha256 digest must be hexadecimal",
            });
        }

        Ok(Self(format!("sha256:{}", hex.to_ascii_lowercase())))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn redacted(&self) -> RedactedValue {
        RedactedValue(format!("sha256:<redacted>:{}", ascii_suffix(&self.0, 8)))
    }
}

impl fmt::Debug for SigningPayloadDigest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("SigningPayloadDigest")
            .field(&self.redacted())
            .finish()
    }
}

/// 脱敏字符串。
#[derive(Clone, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct RedactedValue(String);

impl RedactedValue {
    fn from_reference(value: &str) -> Self {
        Self(format!(
            "<redacted:len={}:suffix={}>",
            value.len(),
            ascii_suffix(value, 6)
        ))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for RedactedValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl fmt::Display for RedactedValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

fn ascii_suffix(value: &str, max_len: usize) -> &str {
    let start = value.len().saturating_sub(max_len);
    &value[start..]
}

/// 签名目的。
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum SigningPurpose {
    SubmitOrder,
    CancelOrder,
    QueryOrder,
    QueryAccount,
    TransferRequest,
    SessionAuth,
    Message,
    TransactionEnvelope,
}

impl SigningPurpose {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::SubmitOrder => "submit_order",
            Self::CancelOrder => "cancel_order",
            Self::QueryOrder => "query_order",
            Self::QueryAccount => "query_account",
            Self::TransferRequest => "transfer_request",
            Self::SessionAuth => "session_auth",
            Self::Message => "message",
            Self::TransactionEnvelope => "transaction_envelope",
        }
    }
}

impl fmt::Display for SigningPurpose {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// 签名策略模式。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SigningPolicyMode {
    Disabled,
    AuditOnly,
    RealSigningEnabled,
}

impl SigningPolicyMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Disabled => "disabled",
            Self::AuditOnly => "audit_only",
            Self::RealSigningEnabled => "real_signing_enabled",
        }
    }
}

/// 签名请求审计上下文。
///
/// 中文说明：审计上下文只携带事件和审批引用，不携带密钥、payload 或签名原文。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SigningAuditContext {
    correlation_id: Option<CorrelationId>,
    causation_id: Option<CausationId>,
    event_refs: Vec<EventId>,
    approval_ref: Option<SigningApprovalRef>,
}

impl SigningAuditContext {
    pub fn with_correlation_id(mut self, correlation_id: CorrelationId) -> Self {
        self.correlation_id = Some(correlation_id);
        self
    }

    pub fn with_causation_id(mut self, causation_id: CausationId) -> Self {
        self.causation_id = Some(causation_id);
        self
    }

    pub fn with_event_ref(mut self, event_ref: EventId) -> Self {
        self.event_refs.push(event_ref);
        self
    }

    pub fn with_approval_ref(mut self, approval_ref: SigningApprovalRef) -> Self {
        self.approval_ref = Some(approval_ref);
        self
    }

    pub fn correlation_id(&self) -> Option<&CorrelationId> {
        self.correlation_id.as_ref()
    }

    pub fn causation_id(&self) -> Option<&CausationId> {
        self.causation_id.as_ref()
    }

    pub fn event_refs(&self) -> &[EventId] {
        &self.event_refs
    }

    pub fn approval_ref(&self) -> Option<&SigningApprovalRef> {
        self.approval_ref.as_ref()
    }
}

/// 签名请求。
///
/// 中文说明：请求对象只包含摘要和引用。策略、执行或运营报告不得通过该对象
/// 获得明文密钥或原始待签名 payload。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SigningRequest {
    request_id: SigningRequestId,
    policy_ref: SigningPolicyRef,
    purpose: SigningPurpose,
    venue_id: VenueId,
    account_id: AccountId,
    payload_digest: SigningPayloadDigest,
    audit_context: SigningAuditContext,
}

impl SigningRequest {
    pub fn new(
        request_id: SigningRequestId,
        policy_ref: SigningPolicyRef,
        purpose: SigningPurpose,
        venue_id: VenueId,
        account_id: AccountId,
        payload_digest: SigningPayloadDigest,
    ) -> Self {
        Self {
            request_id,
            policy_ref,
            purpose,
            venue_id,
            account_id,
            payload_digest,
            audit_context: SigningAuditContext::default(),
        }
    }

    pub fn with_audit_context(mut self, audit_context: SigningAuditContext) -> Self {
        self.audit_context = audit_context;
        self
    }

    pub fn request_id(&self) -> &SigningRequestId {
        &self.request_id
    }

    pub fn policy_ref(&self) -> &SigningPolicyRef {
        &self.policy_ref
    }

    pub fn purpose(&self) -> SigningPurpose {
        self.purpose
    }

    pub fn venue_id(&self) -> &VenueId {
        &self.venue_id
    }

    pub fn account_id(&self) -> &AccountId {
        &self.account_id
    }

    pub fn payload_digest(&self) -> &SigningPayloadDigest {
        &self.payload_digest
    }

    pub fn audit_context(&self) -> &SigningAuditContext {
        &self.audit_context
    }

    pub fn audit_ref(&self) -> SigningAuditRef {
        SigningAuditRef::for_request(self)
    }
}

impl SigningAuditRef {
    pub fn for_request(request: &SigningRequest) -> Self {
        Self(format!(
            "signing-audit/{}/{}",
            request.request_id.as_str(),
            ascii_suffix(request.payload_digest.as_str(), 8)
        ))
    }
}

/// 签名策略。
///
/// 中文说明：策略只表达“哪些请求允许进入签名边界进行审计和拒绝/签名尝试”，
/// 不包含任何密钥材料，也不默认启用真实签名。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SigningPolicy {
    policy_ref: SigningPolicyRef,
    mode: SigningPolicyMode,
    allowed_purposes: BTreeSet<SigningPurpose>,
    require_approval_ref: bool,
}

impl SigningPolicy {
    pub fn new(
        policy_ref: SigningPolicyRef,
        mode: SigningPolicyMode,
        allowed_purposes: impl IntoIterator<Item = SigningPurpose>,
    ) -> SigningResult<Self> {
        let allowed_purposes = allowed_purposes.into_iter().collect::<BTreeSet<_>>();
        if allowed_purposes.is_empty() {
            return Err(SigningError::InvalidRequest {
                field: "allowed_purposes",
                reason: "policy must allow at least one signing purpose",
            });
        }

        Ok(Self {
            policy_ref,
            mode,
            allowed_purposes,
            require_approval_ref: false,
        })
    }

    pub fn audit_only(policy_ref: SigningPolicyRef) -> Self {
        Self {
            policy_ref,
            mode: SigningPolicyMode::AuditOnly,
            allowed_purposes: all_signing_purposes(),
            require_approval_ref: false,
        }
    }

    pub fn disabled(policy_ref: SigningPolicyRef) -> Self {
        Self {
            policy_ref,
            mode: SigningPolicyMode::Disabled,
            allowed_purposes: all_signing_purposes(),
            require_approval_ref: false,
        }
    }

    pub fn real_signing_enabled(policy_ref: SigningPolicyRef) -> Self {
        Self {
            policy_ref,
            mode: SigningPolicyMode::RealSigningEnabled,
            allowed_purposes: all_signing_purposes(),
            require_approval_ref: false,
        }
    }

    pub fn requiring_approval(mut self) -> Self {
        self.require_approval_ref = true;
        self
    }

    pub fn policy_ref(&self) -> &SigningPolicyRef {
        &self.policy_ref
    }

    pub fn mode(&self) -> SigningPolicyMode {
        self.mode
    }

    pub fn allowed_purposes(&self) -> &BTreeSet<SigningPurpose> {
        &self.allowed_purposes
    }

    pub fn require_approval_ref(&self) -> bool {
        self.require_approval_ref
    }

    pub fn validate_request(&self, request: &SigningRequest) -> SigningResult<()> {
        let audit_ref = request.audit_ref();

        if self.mode == SigningPolicyMode::Disabled {
            return Err(SigningError::PolicyDisabled { audit_ref });
        }

        if self.policy_ref != *request.policy_ref() {
            return Err(SigningError::PolicyMismatch {
                audit_ref,
                expected_policy: self.policy_ref.redacted(),
                actual_policy: request.policy_ref().redacted(),
            });
        }

        if !self.allowed_purposes.contains(&request.purpose()) {
            return Err(SigningError::PurposeNotAllowed {
                audit_ref,
                purpose: request.purpose(),
            });
        }

        if self.require_approval_ref && request.audit_context().approval_ref().is_none() {
            return Err(SigningError::ApprovalRequired { audit_ref });
        }

        Ok(())
    }
}

fn all_signing_purposes() -> BTreeSet<SigningPurpose> {
    [
        SigningPurpose::SubmitOrder,
        SigningPurpose::CancelOrder,
        SigningPurpose::QueryOrder,
        SigningPurpose::QueryAccount,
        SigningPurpose::TransferRequest,
        SigningPurpose::SessionAuth,
        SigningPurpose::Message,
        SigningPurpose::TransactionEnvelope,
    ]
    .into_iter()
    .collect()
}

/// 签名提供方边界。
///
/// 中文说明：trait 不暴露密钥读取接口。阶段 11 只提供 `NullSigner`，真实签名
/// 实现不存在或不可用。
pub trait SigningProvider {
    fn sign(
        &self,
        request: &SigningRequest,
        policy: &SigningPolicy,
    ) -> SigningResult<SigningSuccess>;
}

/// 空签名器。
///
/// 中文说明：空签名器会先验证请求和策略，然后始终 fail closed，返回带审计
/// 引用的 `RealSigningUnavailable`，不会产生真实签名材料。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct NullSigner;

impl NullSigner {
    pub fn redacted_attempt(
        &self,
        request: &SigningRequest,
        policy: &SigningPolicy,
    ) -> RedactedSigningLogEntry {
        match self.sign(request, policy) {
            Ok(success) => RedactedSigningLogEntry::from_success(request, &success),
            Err(error) => RedactedSigningLogEntry::from_error(request, &error),
        }
    }
}

impl SigningProvider for NullSigner {
    fn sign(
        &self,
        request: &SigningRequest,
        policy: &SigningPolicy,
    ) -> SigningResult<SigningSuccess> {
        policy.validate_request(request)?;
        Err(SigningError::RealSigningUnavailable {
            audit_ref: request.audit_ref(),
        })
    }
}

/// 签名成功结果。
///
/// 中文说明：成功结果只允许携带外部签名引用，不携带签名原文、私钥或 API secret。
#[derive(Clone, Eq, PartialEq)]
pub struct SigningSuccess {
    audit_ref: SigningAuditRef,
    signature_ref: SignatureRef,
}

impl SigningSuccess {
    pub fn new(audit_ref: SigningAuditRef, signature_ref: SignatureRef) -> Self {
        Self {
            audit_ref,
            signature_ref,
        }
    }

    pub fn audit_ref(&self) -> &SigningAuditRef {
        &self.audit_ref
    }

    pub fn signature_ref(&self) -> &SignatureRef {
        &self.signature_ref
    }
}

impl fmt::Debug for SigningSuccess {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SigningSuccess")
            .field("audit_ref", &self.audit_ref)
            .field("signature_ref", &self.signature_ref.redacted())
            .finish()
    }
}

/// 签名失败码。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SigningFailureCode {
    InvalidToken,
    InvalidDigest,
    InvalidRequest,
    PolicyMismatch,
    PurposeNotAllowed,
    ApprovalRequired,
    PolicyDisabled,
    RealSigningUnavailable,
    RealSigningPolicyNotEnabled,
    SecretUnavailable,
    ClockUnavailable,
}

impl SigningFailureCode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::InvalidToken => "invalid_token",
            Self::InvalidDigest => "invalid_digest",
            Self::InvalidRequest => "invalid_request",
            Self::PolicyMismatch => "policy_mismatch",
            Self::PurposeNotAllowed => "purpose_not_allowed",
            Self::ApprovalRequired => "approval_required",
            Self::PolicyDisabled => "policy_disabled",
            Self::RealSigningUnavailable => "real_signing_unavailable",
            Self::RealSigningPolicyNotEnabled => "real_signing_policy_not_enabled",
            Self::SecretUnavailable => "secret_unavailable",
            Self::ClockUnavailable => "clock_unavailable",
        }
    }
}

impl fmt::Display for SigningFailureCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// 签名尝试状态。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SigningAttemptStatus {
    Signed,
    Rejected,
    Unavailable,
}

impl SigningAttemptStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Signed => "signed",
            Self::Rejected => "rejected",
            Self::Unavailable => "unavailable",
        }
    }
}

impl fmt::Display for SigningAttemptStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// 脱敏签名日志条目。
///
/// 中文说明：该条目可用于日志、事件或运营报告。它只包含引用、枚举和脱敏
/// 摘要，不包含 payload、密钥或签名原文。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RedactedSigningLogEntry {
    request_id: RedactedValue,
    audit_ref: SigningAuditRef,
    policy_ref: RedactedValue,
    venue_id: RedactedValue,
    account_id: RedactedValue,
    payload_digest: RedactedValue,
    purpose: SigningPurpose,
    status: SigningAttemptStatus,
    reason_code: Option<SigningFailureCode>,
}

impl RedactedSigningLogEntry {
    pub fn from_success(request: &SigningRequest, success: &SigningSuccess) -> Self {
        Self::from_parts(
            request,
            success.audit_ref().clone(),
            SigningAttemptStatus::Signed,
            None,
        )
    }

    pub fn from_error(request: &SigningRequest, error: &SigningError) -> Self {
        let audit_ref = error
            .audit_ref()
            .cloned()
            .unwrap_or_else(|| request.audit_ref());
        Self::from_parts(
            request,
            audit_ref,
            error.attempt_status(),
            Some(error.code()),
        )
    }

    fn from_parts(
        request: &SigningRequest,
        audit_ref: SigningAuditRef,
        status: SigningAttemptStatus,
        reason_code: Option<SigningFailureCode>,
    ) -> Self {
        Self {
            request_id: request.request_id().redacted(),
            audit_ref,
            policy_ref: request.policy_ref().redacted(),
            venue_id: RedactedValue::from_reference(request.venue_id().as_str()),
            account_id: RedactedValue::from_reference(request.account_id().as_str()),
            payload_digest: request.payload_digest().redacted(),
            purpose: request.purpose(),
            status,
            reason_code,
        }
    }

    pub fn status(&self) -> SigningAttemptStatus {
        self.status
    }

    pub fn reason_code(&self) -> Option<SigningFailureCode> {
        self.reason_code
    }

    pub fn audit_ref(&self) -> &SigningAuditRef {
        &self.audit_ref
    }
}

impl fmt::Display for RedactedSigningLogEntry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "signing_attempt request_id={} audit_ref={} policy_ref={} venue_id={} account_id={} purpose={} payload_digest={} status={}",
            self.request_id,
            self.audit_ref,
            self.policy_ref,
            self.venue_id,
            self.account_id,
            self.purpose,
            self.payload_digest,
            self.status
        )?;

        if let Some(reason_code) = self.reason_code {
            write!(f, " reason_code={reason_code}")?;
        }

        Ok(())
    }
}

/// 脱敏签名报告。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RedactedSigningReport {
    entries: Vec<RedactedSigningLogEntry>,
}

impl RedactedSigningReport {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, entry: RedactedSigningLogEntry) {
        self.entries.push(entry);
    }

    pub fn entries(&self) -> &[RedactedSigningLogEntry] {
        &self.entries
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

impl fmt::Display for RedactedSigningReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "signing_report entries={}", self.entries.len())?;
        for entry in &self.entries {
            writeln!(f, "{entry}")?;
        }
        Ok(())
    }
}

/// 真实签名边界。
///
/// 中文说明：该模块只有在 `real-signing` feature 下编译。它只提供 HMAC-SHA256
/// 传输签名能力，不保存真实凭证，不实现 fixture 凭证，也不在 `Debug` 或
/// `Display` 中输出 API key、API secret、待签名 query 或 signature。
#[cfg(feature = "real-signing")]
pub mod real {
    use std::collections::BTreeSet;
    use std::env;
    use std::fmt;
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::*;

    /// Binance REST API key 认证头名称。
    pub const BINANCE_API_KEY_HEADER: &str = "X-MBX-APIKEY";

    /// 真实签名提供方接口。
    ///
    /// 中文说明：调用方传入非敏感订单参数、审计引用和策略；实现负责补充
    /// `timestamp`（时间戳）并生成 `signature`（签名）。凭证只能来自环境变量
    /// 或调用方提供的外部 secret provider。
    pub trait RealSigningProvider {
        fn sign_binance_hmac(
            &self,
            input: BinanceHmacSigningInput,
            policy: &SigningPolicy,
        ) -> SigningResult<BinanceSignedEndpoint>;
    }

    /// Binance 凭证提供方。
    ///
    /// 中文说明：外部 secret provider 通过实现该 trait 接入。trait 不提供日志
    /// 接口，也不要求凭证可克隆，避免无意扩散密钥材料。
    pub trait BinanceCredentialProvider {
        fn load_binance_credentials(
            &self,
            audit_ref: &SigningAuditRef,
        ) -> SigningResult<BinanceApiCredentials>;
    }

    /// Binance 时间戳提供方。
    ///
    /// 中文说明：Binance signed endpoint 要求 `timestamp` 参数。默认实现使用
    /// 当前系统时间的毫秒时间戳；测试或外部运行时可注入受控时间源。
    pub trait BinanceTimestampProvider {
        fn timestamp_millis(&self, audit_ref: &SigningAuditRef) -> SigningResult<u64>;
    }

    /// 使用环境变量读取 Binance 凭证的 provider。
    #[derive(Clone, Eq, PartialEq)]
    pub struct EnvBinanceCredentialProvider {
        api_key_env: EnvSecretName,
        secret_key_env: EnvSecretName,
    }

    impl EnvBinanceCredentialProvider {
        pub fn from_default_env() -> SigningResult<Self> {
            Self::from_env_names("BINANCE_API_KEY", "BINANCE_API_SECRET")
        }

        pub fn from_env_names(
            api_key_env: impl Into<String>,
            secret_key_env: impl Into<String>,
        ) -> SigningResult<Self> {
            Ok(Self {
                api_key_env: EnvSecretName::new(api_key_env)?,
                secret_key_env: EnvSecretName::new(secret_key_env)?,
            })
        }

        pub fn api_key_env(&self) -> &EnvSecretName {
            &self.api_key_env
        }

        pub fn secret_key_env(&self) -> &EnvSecretName {
            &self.secret_key_env
        }
    }

    impl fmt::Debug for EnvBinanceCredentialProvider {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.debug_struct("EnvBinanceCredentialProvider")
                .field("api_key_env", &self.api_key_env)
                .field("secret_key_env", &self.secret_key_env)
                .finish()
        }
    }

    impl BinanceCredentialProvider for EnvBinanceCredentialProvider {
        fn load_binance_credentials(
            &self,
            audit_ref: &SigningAuditRef,
        ) -> SigningResult<BinanceApiCredentials> {
            let api_key = read_env_secret(&self.api_key_env, audit_ref)?;
            let secret_key = read_env_secret(&self.secret_key_env, audit_ref)?;
            BinanceApiCredentials::new(api_key, secret_key)
        }
    }

    fn read_env_secret(name: &EnvSecretName, audit_ref: &SigningAuditRef) -> SigningResult<String> {
        env::var(name.as_str()).map_err(|error| {
            let reason = match error {
                env::VarError::NotPresent => "environment variable is not present",
                env::VarError::NotUnicode(_) => "environment variable is not valid unicode",
            };
            SigningError::SecretUnavailable {
                audit_ref: audit_ref.clone(),
                reason,
            }
        })
    }

    /// 环境变量名。
    ///
    /// 中文说明：这里保存的是变量名，不是变量值。`Debug` 仍然脱敏，避免把
    /// 运行环境命名细节扩散到日志。
    #[derive(Clone, Eq, Hash, Ord, PartialEq, PartialOrd)]
    pub struct EnvSecretName(String);

    impl EnvSecretName {
        pub fn new(value: impl Into<String>) -> SigningResult<Self> {
            let value = value.into();
            validate_env_secret_name(&value)?;
            Ok(Self(value))
        }

        pub fn as_str(&self) -> &str {
            &self.0
        }
    }

    impl fmt::Debug for EnvSecretName {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.debug_tuple("EnvSecretName")
                .field(&RedactedValue::from_reference(&self.0))
                .finish()
        }
    }

    /// 系统时间戳提供方。
    #[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
    pub struct SystemBinanceTimestampProvider;

    impl BinanceTimestampProvider for SystemBinanceTimestampProvider {
        fn timestamp_millis(&self, audit_ref: &SigningAuditRef) -> SigningResult<u64> {
            let duration = SystemTime::now().duration_since(UNIX_EPOCH).map_err(|_| {
                SigningError::ClockUnavailable {
                    audit_ref: audit_ref.clone(),
                }
            })?;
            u64::try_from(duration.as_millis()).map_err(|_| SigningError::ClockUnavailable {
                audit_ref: audit_ref.clone(),
            })
        }
    }

    /// Binance HMAC-SHA256 签名提供方。
    pub struct BinanceHmacSha256SigningProvider<C, T> {
        credentials: C,
        timestamp: T,
    }

    /// 默认真实签名 provider 类型。
    pub type RealSigningProviderFromEnv = BinanceHmacSha256SigningProvider<
        EnvBinanceCredentialProvider,
        SystemBinanceTimestampProvider,
    >;

    impl<C, T> BinanceHmacSha256SigningProvider<C, T> {
        pub fn new(credentials: C, timestamp: T) -> Self {
            Self {
                credentials,
                timestamp,
            }
        }

        pub fn credentials(&self) -> &C {
            &self.credentials
        }

        pub fn timestamp_provider(&self) -> &T {
            &self.timestamp
        }
    }

    impl RealSigningProviderFromEnv {
        pub fn from_default_env() -> SigningResult<Self> {
            Ok(Self::new(
                EnvBinanceCredentialProvider::from_default_env()?,
                SystemBinanceTimestampProvider,
            ))
        }

        pub fn from_env_names(
            api_key_env: impl Into<String>,
            secret_key_env: impl Into<String>,
        ) -> SigningResult<Self> {
            Ok(Self::new(
                EnvBinanceCredentialProvider::from_env_names(api_key_env, secret_key_env)?,
                SystemBinanceTimestampProvider,
            ))
        }
    }

    impl<C, T> fmt::Debug for BinanceHmacSha256SigningProvider<C, T> {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.debug_struct("BinanceHmacSha256SigningProvider")
                .field("credentials", &"<redacted>")
                .field("timestamp", &"<configured>")
                .finish()
        }
    }

    impl<C, T> RealSigningProvider for BinanceHmacSha256SigningProvider<C, T>
    where
        C: BinanceCredentialProvider,
        T: BinanceTimestampProvider,
    {
        fn sign_binance_hmac(
            &self,
            input: BinanceHmacSigningInput,
            policy: &SigningPolicy,
        ) -> SigningResult<BinanceSignedEndpoint> {
            let pending_audit_ref = input.pending_audit_ref()?;
            let timestamp_millis = self.timestamp.timestamp_millis(&pending_audit_ref)?;
            let canonical_payload = input.canonical_payload(timestamp_millis);
            let payload_digest = SigningPayloadDigest::new(format!(
                "sha256:{}",
                sha256_hex(canonical_payload.as_bytes())
            ))?;
            let request = input.into_signing_request(payload_digest);
            policy.validate_request(&request)?;

            if policy.mode() != SigningPolicyMode::RealSigningEnabled {
                return Err(SigningError::RealSigningPolicyNotEnabled {
                    audit_ref: request.audit_ref(),
                });
            }

            let credentials = self
                .credentials
                .load_binance_credentials(&request.audit_ref())?;
            let signature = hmac_sha256_hex(
                credentials.secret_key.expose_bytes(),
                canonical_payload.as_bytes(),
            );
            let signature_ref = SignatureRef::new(format!(
                "signature-ref/binance-hmac/{}",
                ascii_suffix(request.payload_digest().as_str(), 24)
            ))?;
            let success = SigningSuccess::new(request.audit_ref(), signature_ref);
            let signed_query = format!("{canonical_payload}&signature={signature}");

            Ok(BinanceSignedEndpoint {
                api_key: credentials.api_key,
                timestamp_millis,
                signed_query: SecretString::new("signed_query", signed_query)?,
                signature: BinanceHmacSignature(SecretString::new("signature", signature)?),
                request,
                success,
            })
        }
    }

    /// Binance HMAC 签名输入。
    ///
    /// 中文说明：输入只包含非密钥的请求上下文和待发送参数。Provider 会按
    /// Binance signed endpoint 要求补充 `timestamp`，并拒绝调用方预置
    /// `signature` 或 `timestamp` 参数。
    #[derive(Clone, Eq, PartialEq)]
    pub struct BinanceHmacSigningInput {
        request_id: SigningRequestId,
        policy_ref: SigningPolicyRef,
        purpose: SigningPurpose,
        venue_id: VenueId,
        account_id: AccountId,
        audit_context: SigningAuditContext,
        params: Vec<BinanceRequestParam>,
    }

    impl BinanceHmacSigningInput {
        pub fn new(
            request_id: SigningRequestId,
            policy_ref: SigningPolicyRef,
            purpose: SigningPurpose,
            venue_id: VenueId,
            account_id: AccountId,
            params: impl IntoIterator<Item = BinanceRequestParam>,
        ) -> SigningResult<Self> {
            let params = params.into_iter().collect::<Vec<_>>();
            validate_binance_params(&params)?;
            Ok(Self {
                request_id,
                policy_ref,
                purpose,
                venue_id,
                account_id,
                audit_context: SigningAuditContext::default(),
                params,
            })
        }

        pub fn with_audit_context(mut self, audit_context: SigningAuditContext) -> Self {
            self.audit_context = audit_context;
            self
        }

        pub fn request_id(&self) -> &SigningRequestId {
            &self.request_id
        }

        pub fn policy_ref(&self) -> &SigningPolicyRef {
            &self.policy_ref
        }

        pub fn params(&self) -> &[BinanceRequestParam] {
            &self.params
        }

        fn pending_audit_ref(&self) -> SigningResult<SigningAuditRef> {
            SigningAuditRef::new(format!(
                "signing-audit/{}/pending",
                self.request_id.as_str()
            ))
        }

        fn canonical_payload(&self, timestamp_millis: u64) -> String {
            let mut payload = String::new();
            for (index, param) in self.params.iter().enumerate() {
                if index > 0 {
                    payload.push('&');
                }
                param.push_encoded_pair(&mut payload);
            }
            if !payload.is_empty() {
                payload.push('&');
            }
            payload.push_str("timestamp=");
            payload.push_str(&timestamp_millis.to_string());
            payload
        }

        fn into_signing_request(self, payload_digest: SigningPayloadDigest) -> SigningRequest {
            SigningRequest::new(
                self.request_id,
                self.policy_ref,
                self.purpose,
                self.venue_id,
                self.account_id,
                payload_digest,
            )
            .with_audit_context(self.audit_context)
        }
    }

    impl fmt::Debug for BinanceHmacSigningInput {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.debug_struct("BinanceHmacSigningInput")
                .field("request_id", &self.request_id)
                .field("policy_ref", &self.policy_ref.redacted())
                .field("purpose", &self.purpose)
                .field(
                    "venue_id",
                    &RedactedValue::from_reference(self.venue_id.as_str()),
                )
                .field(
                    "account_id",
                    &RedactedValue::from_reference(self.account_id.as_str()),
                )
                .field("params", &self.params)
                .finish()
        }
    }

    /// Binance query/body 参数。
    #[derive(Clone, Eq, PartialEq)]
    pub struct BinanceRequestParam {
        name: String,
        value: String,
    }

    impl BinanceRequestParam {
        pub fn new(name: impl Into<String>, value: impl Into<String>) -> SigningResult<Self> {
            let name = name.into();
            let value = value.into();
            validate_binance_param_name(&name)?;
            validate_binance_param_value(&value)?;
            Ok(Self { name, value })
        }

        pub fn name(&self) -> &str {
            &self.name
        }

        pub fn value_for_transport(&self) -> &str {
            &self.value
        }

        fn push_encoded_pair(&self, output: &mut String) {
            output.push_str(&percent_encode_component(&self.name));
            output.push('=');
            output.push_str(&percent_encode_component(&self.value));
        }
    }

    impl fmt::Debug for BinanceRequestParam {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.debug_struct("BinanceRequestParam")
                .field("name", &self.name)
                .field("value", &RedactedValue::from_reference(&self.value))
                .finish()
        }
    }

    /// Binance API 凭证。
    ///
    /// 中文说明：该类型只在内存中短暂持有 API key 和 secret key。`Debug` 不输出
    /// 原文；secret key 用完后随对象 drop 清零。
    pub struct BinanceApiCredentials {
        api_key: BinanceApiKey,
        secret_key: BinanceSecretKey,
    }

    impl BinanceApiCredentials {
        pub fn new(
            api_key: impl Into<String>,
            secret_key: impl Into<String>,
        ) -> SigningResult<Self> {
            Ok(Self {
                api_key: BinanceApiKey::new(api_key)?,
                secret_key: BinanceSecretKey::new(secret_key)?,
            })
        }

        pub fn from_parts(api_key: BinanceApiKey, secret_key: BinanceSecretKey) -> Self {
            Self {
                api_key,
                secret_key,
            }
        }
    }

    impl fmt::Debug for BinanceApiCredentials {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.debug_struct("BinanceApiCredentials")
                .field("api_key", &"<redacted>")
                .field("secret_key", &"<redacted>")
                .finish()
        }
    }

    /// Binance API key。
    pub struct BinanceApiKey(SecretString);

    impl BinanceApiKey {
        pub fn new(value: impl Into<String>) -> SigningResult<Self> {
            Ok(Self(SecretString::new("api_key", value.into())?))
        }

        pub fn expose_for_transport(&self) -> &str {
            self.0.expose_str()
        }
    }

    impl fmt::Debug for BinanceApiKey {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("BinanceApiKey(<redacted>)")
        }
    }

    /// Binance API secret。
    pub struct BinanceSecretKey(SecretBytes);

    impl BinanceSecretKey {
        pub fn new(value: impl Into<String>) -> SigningResult<Self> {
            Self::from_bytes(value.into().into_bytes())
        }

        pub fn from_bytes(value: Vec<u8>) -> SigningResult<Self> {
            Ok(Self(SecretBytes::new("secret_key", value)?))
        }

        fn expose_bytes(&self) -> &[u8] {
            self.0.expose_bytes()
        }
    }

    impl fmt::Debug for BinanceSecretKey {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("BinanceSecretKey(<redacted>)")
        }
    }

    /// HMAC signature（哈希消息认证码签名）。
    pub struct BinanceHmacSignature(SecretString);

    impl BinanceHmacSignature {
        pub fn as_str(&self) -> &str {
            self.0.expose_str()
        }
    }

    impl fmt::Debug for BinanceHmacSignature {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("BinanceHmacSignature(<redacted>)")
        }
    }

    /// 已签名 Binance endpoint 传输材料。
    ///
    /// 中文说明：该对象包含 HTTP 发送所需的 API key 头和值、带 `timestamp` 和
    /// `signature` 的 query string。它不能被直接显示；`Debug` 会脱敏。
    pub struct BinanceSignedEndpoint {
        api_key: BinanceApiKey,
        timestamp_millis: u64,
        signed_query: SecretString,
        signature: BinanceHmacSignature,
        request: SigningRequest,
        success: SigningSuccess,
    }

    impl BinanceSignedEndpoint {
        pub fn api_key_header_name(&self) -> &'static str {
            BINANCE_API_KEY_HEADER
        }

        pub fn api_key_header_value(&self) -> &str {
            self.api_key.expose_for_transport()
        }

        pub fn signed_query_for_transport(&self) -> &str {
            self.signed_query.expose_str()
        }

        pub fn timestamp_millis(&self) -> u64 {
            self.timestamp_millis
        }

        pub fn signature(&self) -> &BinanceHmacSignature {
            &self.signature
        }

        pub fn signing_request(&self) -> &SigningRequest {
            &self.request
        }

        pub fn signing_success(&self) -> &SigningSuccess {
            &self.success
        }

        pub fn redacted_log_entry(&self) -> RedactedSigningLogEntry {
            RedactedSigningLogEntry::from_success(&self.request, &self.success)
        }
    }

    impl fmt::Debug for BinanceSignedEndpoint {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.debug_struct("BinanceSignedEndpoint")
                .field("api_key_header_name", &BINANCE_API_KEY_HEADER)
                .field("api_key_header_value", &"<redacted>")
                .field("signed_query", &"<redacted>")
                .field("timestamp_millis", &self.timestamp_millis)
                .field("signing_request", &self.signing_request())
                .field("signing_success", &self.signing_success())
                .finish()
        }
    }

    struct SecretString {
        bytes: Vec<u8>,
    }

    impl SecretString {
        fn new(field: &'static str, value: impl Into<String>) -> SigningResult<Self> {
            let value = value.into();
            if value.is_empty() {
                return Err(SigningError::InvalidRequest {
                    field,
                    reason: "secret transport value cannot be empty",
                });
            }
            if value.len() > 8192 {
                return Err(SigningError::InvalidRequest {
                    field,
                    reason: "secret transport value is too long",
                });
            }
            if value
                .bytes()
                .any(|byte| byte == 0 || byte.is_ascii_control())
            {
                return Err(SigningError::InvalidRequest {
                    field,
                    reason: "secret transport value contains a control byte",
                });
            }
            Ok(Self {
                bytes: value.into_bytes(),
            })
        }

        fn expose_str(&self) -> &str {
            std::str::from_utf8(&self.bytes)
                .expect("SecretString only accepts String-derived UTF-8 bytes")
        }
    }

    impl Drop for SecretString {
        fn drop(&mut self) {
            wipe_bytes(&mut self.bytes);
        }
    }

    impl fmt::Debug for SecretString {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("<redacted>")
        }
    }

    struct SecretBytes {
        bytes: Vec<u8>,
    }

    impl SecretBytes {
        fn new(field: &'static str, bytes: Vec<u8>) -> SigningResult<Self> {
            if bytes.is_empty() {
                return Err(SigningError::InvalidRequest {
                    field,
                    reason: "secret bytes cannot be empty",
                });
            }
            if bytes.len() > 8192 {
                return Err(SigningError::InvalidRequest {
                    field,
                    reason: "secret bytes value is too long",
                });
            }
            Ok(Self { bytes })
        }

        fn expose_bytes(&self) -> &[u8] {
            &self.bytes
        }
    }

    impl Drop for SecretBytes {
        fn drop(&mut self) {
            wipe_bytes(&mut self.bytes);
        }
    }

    impl fmt::Debug for SecretBytes {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("<redacted>")
        }
    }

    fn validate_env_secret_name(value: &str) -> SigningResult<()> {
        if value.is_empty() {
            return Err(SigningError::InvalidToken {
                field: "env_secret_name",
                reason: "environment variable name cannot be empty",
            });
        }
        if value.len() > 128 {
            return Err(SigningError::InvalidToken {
                field: "env_secret_name",
                reason: "environment variable name is too long",
            });
        }
        let mut bytes = value.bytes();
        let Some(first) = bytes.next() else {
            return Err(SigningError::InvalidToken {
                field: "env_secret_name",
                reason: "environment variable name cannot be empty",
            });
        };
        if !(first.is_ascii_alphabetic() || first == b'_') {
            return Err(SigningError::InvalidToken {
                field: "env_secret_name",
                reason: "environment variable name must start with a letter or underscore",
            });
        }
        if bytes.any(|byte| !(byte.is_ascii_alphanumeric() || byte == b'_')) {
            return Err(SigningError::InvalidToken {
                field: "env_secret_name",
                reason: "environment variable name contains an unsupported byte",
            });
        }
        Ok(())
    }

    fn validate_binance_params(params: &[BinanceRequestParam]) -> SigningResult<()> {
        let mut names = BTreeSet::new();
        for param in params {
            if !names.insert(param.name.as_str()) {
                return Err(SigningError::InvalidRequest {
                    field: "binance_param",
                    reason: "duplicate parameter name",
                });
            }
        }
        Ok(())
    }

    fn validate_binance_param_name(value: &str) -> SigningResult<()> {
        if value.is_empty() {
            return Err(SigningError::InvalidRequest {
                field: "binance_param_name",
                reason: "parameter name cannot be empty",
            });
        }
        if value.len() > 64 {
            return Err(SigningError::InvalidRequest {
                field: "binance_param_name",
                reason: "parameter name is too long",
            });
        }
        if value.eq_ignore_ascii_case("signature") || value.eq_ignore_ascii_case("timestamp") {
            return Err(SigningError::InvalidRequest {
                field: "binance_param_name",
                reason: "signature boundary owns signature and timestamp parameters",
            });
        }
        if looks_like_secret_label(value) {
            return Err(SigningError::InvalidRequest {
                field: "binance_param_name",
                reason: "parameter name must not look like key material",
            });
        }
        if value
            .bytes()
            .any(|byte| !(byte.is_ascii_alphanumeric() || byte == b'_'))
        {
            return Err(SigningError::InvalidRequest {
                field: "binance_param_name",
                reason: "parameter name must be ASCII letters, digits or underscore",
            });
        }
        Ok(())
    }

    fn validate_binance_param_value(value: &str) -> SigningResult<()> {
        if value.len() > 4096 {
            return Err(SigningError::InvalidRequest {
                field: "binance_param_value",
                reason: "parameter value is too long",
            });
        }
        if value
            .bytes()
            .any(|byte| byte == 0 || byte.is_ascii_control())
        {
            return Err(SigningError::InvalidRequest {
                field: "binance_param_value",
                reason: "parameter value contains a control byte",
            });
        }
        Ok(())
    }

    fn percent_encode_component(value: &str) -> String {
        const HEX: &[u8; 16] = b"0123456789ABCDEF";
        let mut encoded = String::with_capacity(value.len());
        for byte in value.as_bytes() {
            if is_unreserved_percent_byte(*byte) {
                encoded.push(*byte as char);
            } else {
                encoded.push('%');
                encoded.push(HEX[(byte >> 4) as usize] as char);
                encoded.push(HEX[(byte & 0x0f) as usize] as char);
            }
        }
        encoded
    }

    fn is_unreserved_percent_byte(byte: u8) -> bool {
        byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~')
    }

    fn hmac_sha256_hex(secret_key: &[u8], payload: &[u8]) -> String {
        let mut key_block = [0_u8; 64];
        if secret_key.len() > key_block.len() {
            let digest = sha256(secret_key);
            key_block[..digest.len()].copy_from_slice(&digest);
        } else {
            key_block[..secret_key.len()].copy_from_slice(secret_key);
        }

        let mut inner_pad = [0x36_u8; 64];
        let mut outer_pad = [0x5c_u8; 64];
        for index in 0..64 {
            inner_pad[index] ^= key_block[index];
            outer_pad[index] ^= key_block[index];
        }

        let mut inner_input = Vec::with_capacity(inner_pad.len() + payload.len());
        inner_input.extend_from_slice(&inner_pad);
        inner_input.extend_from_slice(payload);
        let mut inner_digest = sha256(&inner_input);

        let mut outer_input = Vec::with_capacity(outer_pad.len() + inner_digest.len());
        outer_input.extend_from_slice(&outer_pad);
        outer_input.extend_from_slice(&inner_digest);
        let digest = sha256(&outer_input);

        wipe_bytes(&mut key_block);
        wipe_bytes(&mut inner_pad);
        wipe_bytes(&mut outer_pad);
        wipe_bytes(&mut inner_input);
        wipe_bytes(&mut inner_digest);
        wipe_bytes(&mut outer_input);

        hex_lower(&digest)
    }

    fn sha256_hex(input: &[u8]) -> String {
        hex_lower(&sha256(input))
    }

    fn hex_lower(input: &[u8]) -> String {
        const HEX: &[u8; 16] = b"0123456789abcdef";
        let mut output = String::with_capacity(input.len() * 2);
        for byte in input {
            output.push(HEX[(byte >> 4) as usize] as char);
            output.push(HEX[(byte & 0x0f) as usize] as char);
        }
        output
    }

    fn sha256(input: &[u8]) -> [u8; 32] {
        const H0: [u32; 8] = [
            0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
            0x5be0cd19,
        ];
        const K: [u32; 64] = [
            0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
            0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
            0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
            0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
            0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
            0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
            0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
            0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
            0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
            0xc67178f2,
        ];

        let mut message = input.to_vec();
        let bit_len = (message.len() as u64) * 8;
        message.push(0x80);
        while message.len() % 64 != 56 {
            message.push(0);
        }
        message.extend_from_slice(&bit_len.to_be_bytes());

        let mut state = H0;
        for chunk in message.chunks_exact(64) {
            let mut schedule = [0_u32; 64];
            let mut index = 0;
            while index < 16 {
                let offset = index * 4;
                schedule[index] = u32::from_be_bytes([
                    chunk[offset],
                    chunk[offset + 1],
                    chunk[offset + 2],
                    chunk[offset + 3],
                ]);
                index += 1;
            }

            while index < 64 {
                let s0 = schedule[index - 15].rotate_right(7)
                    ^ schedule[index - 15].rotate_right(18)
                    ^ (schedule[index - 15] >> 3);
                let s1 = schedule[index - 2].rotate_right(17)
                    ^ schedule[index - 2].rotate_right(19)
                    ^ (schedule[index - 2] >> 10);
                schedule[index] = schedule[index - 16]
                    .wrapping_add(s0)
                    .wrapping_add(schedule[index - 7])
                    .wrapping_add(s1);
                index += 1;
            }

            let mut a = state[0];
            let mut b = state[1];
            let mut c = state[2];
            let mut d = state[3];
            let mut e = state[4];
            let mut f = state[5];
            let mut g = state[6];
            let mut h = state[7];

            let mut round = 0;
            while round < 64 {
                let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
                let choice = (e & f) ^ ((!e) & g);
                let temp1 = h
                    .wrapping_add(s1)
                    .wrapping_add(choice)
                    .wrapping_add(K[round])
                    .wrapping_add(schedule[round]);
                let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
                let majority = (a & b) ^ (a & c) ^ (b & c);
                let temp2 = s0.wrapping_add(majority);

                h = g;
                g = f;
                f = e;
                e = d.wrapping_add(temp1);
                d = c;
                c = b;
                b = a;
                a = temp1.wrapping_add(temp2);

                round += 1;
            }

            state[0] = state[0].wrapping_add(a);
            state[1] = state[1].wrapping_add(b);
            state[2] = state[2].wrapping_add(c);
            state[3] = state[3].wrapping_add(d);
            state[4] = state[4].wrapping_add(e);
            state[5] = state[5].wrapping_add(f);
            state[6] = state[6].wrapping_add(g);
            state[7] = state[7].wrapping_add(h);
        }

        let mut digest = [0_u8; 32];
        for (index, word) in state.into_iter().enumerate() {
            digest[index * 4..index * 4 + 4].copy_from_slice(&word.to_be_bytes());
        }
        wipe_bytes(&mut message);
        digest
    }

    fn wipe_bytes(bytes: &mut [u8]) {
        bytes.fill(0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use arb_domain::{AccountId, VenueId};

    #[cfg(not(feature = "real-signing"))]
    #[test]
    fn default_build_has_no_real_signing() {
        assert!(!REAL_SIGNING_FEATURE_ENABLED);
    }

    #[test]
    fn null_signer_fails_closed_with_audit_ref() {
        let request = sample_request();
        let policy = SigningPolicy::audit_only(request.policy_ref().clone());
        let error = NullSigner
            .sign(&request, &policy)
            .expect_err("null signer must fail");

        assert_eq!(error.code(), SigningFailureCode::RealSigningUnavailable);
        assert_eq!(error.audit_ref(), Some(&request.audit_ref()));
    }

    #[test]
    fn signing_failure_is_not_success() {
        let request = sample_request();
        let policy = SigningPolicy::new(
            request.policy_ref().clone(),
            SigningPolicyMode::AuditOnly,
            [SigningPurpose::CancelOrder],
        )
        .expect("valid policy");

        let result = NullSigner.sign(&request, &policy);

        assert!(matches!(
            result,
            Err(SigningError::PurposeNotAllowed { .. })
        ));
    }

    #[test]
    fn disabled_policy_rejects_before_signing_attempt() {
        let request = sample_request();
        let policy = SigningPolicy::disabled(request.policy_ref().clone());

        let entry = NullSigner.redacted_attempt(&request, &policy);

        assert_eq!(entry.status(), SigningAttemptStatus::Rejected);
        assert_eq!(
            entry.reason_code(),
            Some(SigningFailureCode::PolicyDisabled)
        );
    }

    #[test]
    fn approval_policy_requires_audit_reference() {
        let request = sample_request();
        let policy = SigningPolicy::audit_only(request.policy_ref().clone()).requiring_approval();

        let error = policy
            .validate_request(&request)
            .expect_err("missing approval must fail");

        assert_eq!(error.code(), SigningFailureCode::ApprovalRequired);
        assert_eq!(error.audit_ref(), Some(&request.audit_ref()));
    }

    #[test]
    fn redacted_log_and_report_do_not_expose_sensitive_material() {
        let digest = "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa11111111";
        let request = SigningRequest::new(
            SigningRequestId::new("signing-request/redaction-case").expect("request id"),
            SigningPolicyRef::new("kms-policy/main-prod-wallet-alpha").expect("policy ref"),
            SigningPurpose::SubmitOrder,
            VenueId::new("binance/spot").expect("venue id"),
            AccountId::new("account/main-prod-wallet-alpha").expect("account id"),
            SigningPayloadDigest::new(digest).expect("digest"),
        );
        let policy = SigningPolicy::audit_only(request.policy_ref().clone());
        let error = NullSigner.sign(&request, &policy).expect_err("null signer");
        let entry = RedactedSigningLogEntry::from_error(&request, &error);
        let rendered_entry = entry.to_string();

        assert!(!rendered_entry.contains(digest));
        assert!(!rendered_entry.contains("kms-policy/main-prod-wallet-alpha"));
        assert!(!rendered_entry.contains("account/main-prod-wallet-alpha"));
        assert!(rendered_entry.contains("sha256:<redacted>:11111111"));
        assert!(rendered_entry.contains("reason_code=real_signing_unavailable"));

        let mut report = RedactedSigningReport::new();
        report.push(entry);
        let rendered_report = report.to_string();

        assert!(!rendered_report.contains(digest));
        assert!(!rendered_report.contains("kms-policy/main-prod-wallet-alpha"));
        assert!(!rendered_report.contains("account/main-prod-wallet-alpha"));
    }

    #[test]
    fn invalid_input_errors_do_not_echo_candidate_secret() {
        let secret_like = "signing-policy/sensitive-label-prod-alpha";
        let error = SigningPolicyRef::new(secret_like).expect_err("secret labels are rejected");
        let rendered = format!("{error:?} {error}");

        assert!(!rendered.contains(secret_like));
        assert!(rendered.contains("key material"));
    }

    #[test]
    fn audit_ref_is_stable_and_derived_from_request_reference() {
        let request = sample_request();

        let first = request.audit_ref();
        let second = request.audit_ref();

        assert_eq!(first, second);
        assert!(first.as_str().starts_with("signing-audit/signing-request/"));
        assert!(first.as_str().ends_with("abcdef12"));
    }

    #[test]
    fn policy_mismatch_uses_redacted_values() {
        let request = sample_request();
        let policy = SigningPolicy::audit_only(
            SigningPolicyRef::new("mock-policy/other-policy-alpha").expect("policy ref"),
        );

        let error = policy
            .validate_request(&request)
            .expect_err("policy mismatch must fail");
        let rendered = error.to_string();

        assert_eq!(error.code(), SigningFailureCode::PolicyMismatch);
        assert!(!rendered.contains(request.policy_ref().as_str()));
        assert!(rendered.contains("<redacted:"));
    }

    #[cfg(feature = "real-signing")]
    #[test]
    fn real_signing_build_exposes_binance_hmac_provider() {
        use crate::real::{
            BinanceHmacSha256SigningProvider, BinanceTimestampProvider, RealSigningProvider,
            BINANCE_API_KEY_HEADER,
        };

        assert!(REAL_SIGNING_FEATURE_ENABLED);

        let policy_ref = SigningPolicyRef::new("kms-policy/binance-hmac-unit").expect("policy ref");
        let policy = SigningPolicy::real_signing_enabled(policy_ref.clone());
        let input = sample_binance_input(policy_ref);
        let signer = BinanceHmacSha256SigningProvider::new(
            GeneratedCredentialProvider { seed: 3 },
            FixedTimestamp(1_700_000_000_123),
        );

        let signed = signer
            .sign_binance_hmac(input, &policy)
            .expect("real-signing feature should sign with generated test material");

        assert_eq!(signed.api_key_header_name(), BINANCE_API_KEY_HEADER);
        assert_eq!(signed.timestamp_millis(), 1_700_000_000_123);
        assert!(signed
            .signed_query_for_transport()
            .contains("timestamp=1700000000123"));
        assert!(signed.signed_query_for_transport().contains("&signature="));
        assert!(signed
            .signed_query_for_transport()
            .contains("newClientOrderId=order-%E4%B8%80"));
        assert_eq!(signed.signature().as_str().len(), 64);
        assert!(signed
            .signature()
            .as_str()
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit()));

        let raw_api_key = signed.api_key_header_value().to_owned();
        let raw_signature = signed.signature().as_str().to_owned();
        let raw_query = signed.signed_query_for_transport().to_owned();
        let rendered_debug = format!("{signed:?}");
        let rendered_log = signed.redacted_log_entry().to_string();

        assert!(!rendered_debug.contains(&raw_api_key));
        assert!(!rendered_debug.contains(&raw_signature));
        assert!(!rendered_debug.contains(&raw_query));
        assert!(!rendered_log.contains(&raw_api_key));
        assert!(!rendered_log.contains(&raw_signature));
        assert!(!rendered_log.contains(&raw_query));
        assert_eq!(
            signed.redacted_log_entry().status(),
            SigningAttemptStatus::Signed
        );
        assert_eq!(signed.redacted_log_entry().reason_code(), None);

        let changed_timestamp = BinanceHmacSha256SigningProvider::new(
            GeneratedCredentialProvider { seed: 3 },
            FixedTimestamp(1_700_000_000_124),
        )
        .sign_binance_hmac(sample_binance_input(policy.policy_ref().clone()), &policy)
        .expect("changed timestamp still signs");
        assert_ne!(
            raw_signature,
            changed_timestamp.signature().as_str(),
            "timestamp must be part of the signed payload"
        );

        let changed_credentials = BinanceHmacSha256SigningProvider::new(
            GeneratedCredentialProvider { seed: 11 },
            FixedTimestamp(1_700_000_000_123),
        )
        .sign_binance_hmac(sample_binance_input(policy.policy_ref().clone()), &policy)
        .expect("changed credentials still sign");
        assert_ne!(
            raw_signature,
            changed_credentials.signature().as_str(),
            "credential source must affect the HMAC signature"
        );

        fn _assert_timestamp_provider<T: BinanceTimestampProvider>(provider: T) -> T {
            provider
        }
        let _ = _assert_timestamp_provider(FixedTimestamp(1));
    }

    #[cfg(feature = "real-signing")]
    #[test]
    fn real_signing_requires_explicit_policy_mode_before_loading_credentials() {
        use crate::real::{BinanceHmacSha256SigningProvider, RealSigningProvider};

        let policy_ref =
            SigningPolicyRef::new("kms-policy/binance-hmac-audit-only").expect("policy ref");
        let input = sample_binance_input(policy_ref.clone());
        let policy = SigningPolicy::audit_only(policy_ref);
        let signer =
            BinanceHmacSha256SigningProvider::new(PanicCredentialProvider, FixedTimestamp(42));

        let error = signer
            .sign_binance_hmac(input, &policy)
            .expect_err("audit-only policy must not produce a real signature");

        assert_eq!(
            error.code(),
            SigningFailureCode::RealSigningPolicyNotEnabled
        );
    }

    #[cfg(feature = "real-signing")]
    #[test]
    fn binance_params_reserve_signature_and_timestamp_for_boundary() {
        use crate::real::BinanceRequestParam;

        let timestamp_error =
            BinanceRequestParam::new("timestamp", "1700000000000").expect_err("reserved");
        let signature_error =
            BinanceRequestParam::new("signature", "abcdef").expect_err("reserved");

        assert_eq!(timestamp_error.code(), SigningFailureCode::InvalidRequest);
        assert_eq!(signature_error.code(), SigningFailureCode::InvalidRequest);
    }

    #[cfg(feature = "real-signing")]
    fn sample_binance_input(policy_ref: SigningPolicyRef) -> crate::real::BinanceHmacSigningInput {
        use crate::real::{BinanceHmacSigningInput, BinanceRequestParam};

        BinanceHmacSigningInput::new(
            SigningRequestId::new("signing-request/binance-hmac-unit").expect("request id"),
            policy_ref,
            SigningPurpose::SubmitOrder,
            VenueId::new("binance/spot").expect("venue id"),
            AccountId::new("account/paper-binance").expect("account id"),
            [
                BinanceRequestParam::new("symbol", "BTCUSDT").expect("param"),
                BinanceRequestParam::new("side", "BUY").expect("param"),
                BinanceRequestParam::new("type", "LIMIT").expect("param"),
                BinanceRequestParam::new("timeInForce", "GTC").expect("param"),
                BinanceRequestParam::new("quantity", "1").expect("param"),
                BinanceRequestParam::new("price", "0.1").expect("param"),
                BinanceRequestParam::new("recvWindow", "5000").expect("param"),
                BinanceRequestParam::new("newClientOrderId", "order-一").expect("param"),
            ],
        )
        .expect("binance input")
    }

    #[cfg(feature = "real-signing")]
    #[derive(Clone, Copy, Debug)]
    struct FixedTimestamp(u64);

    #[cfg(feature = "real-signing")]
    impl crate::real::BinanceTimestampProvider for FixedTimestamp {
        fn timestamp_millis(&self, _audit_ref: &SigningAuditRef) -> SigningResult<u64> {
            Ok(self.0)
        }
    }

    #[cfg(feature = "real-signing")]
    #[derive(Clone, Copy, Debug)]
    struct GeneratedCredentialProvider {
        seed: u8,
    }

    #[cfg(feature = "real-signing")]
    impl crate::real::BinanceCredentialProvider for GeneratedCredentialProvider {
        fn load_binance_credentials(
            &self,
            _audit_ref: &SigningAuditRef,
        ) -> SigningResult<crate::real::BinanceApiCredentials> {
            crate::real::BinanceApiCredentials::new(
                generated_ascii(48, self.seed),
                generated_ascii(64, self.seed.wrapping_add(19)),
            )
        }
    }

    #[cfg(feature = "real-signing")]
    #[derive(Clone, Copy, Debug)]
    struct PanicCredentialProvider;

    #[cfg(feature = "real-signing")]
    impl crate::real::BinanceCredentialProvider for PanicCredentialProvider {
        fn load_binance_credentials(
            &self,
            _audit_ref: &SigningAuditRef,
        ) -> SigningResult<crate::real::BinanceApiCredentials> {
            panic!("credential provider must not be called before policy enables real signing")
        }
    }

    #[cfg(feature = "real-signing")]
    fn generated_ascii(len: usize, seed: u8) -> String {
        const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
        let mut bytes = Vec::with_capacity(len);
        for index in 0..len {
            let alphabet_index = (index + usize::from(seed)) % ALPHABET.len();
            bytes.push(ALPHABET[alphabet_index]);
        }
        String::from_utf8(bytes).expect("generated ASCII")
    }

    fn sample_request() -> SigningRequest {
        SigningRequest::new(
            SigningRequestId::new("signing-request/unit-test").expect("request id"),
            SigningPolicyRef::new("mock-policy/null-signer").expect("policy ref"),
            SigningPurpose::SubmitOrder,
            VenueId::new("binance/spot").expect("venue id"),
            AccountId::new("account/paper-main").expect("account id"),
            SigningPayloadDigest::new(
                "sha256:00000000000000000000000000000000000000000000000000000000abcdef12",
            )
            .expect("digest"),
        )
    }
}
