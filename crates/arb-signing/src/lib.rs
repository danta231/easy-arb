//! `arb-signing` 签名边界和空签名器。
//!
//! 中文说明：本 crate 只定义受控签名请求、签名策略、审计引用和默认拒绝的
//! 空签名器。默认 feature 为空，不连接真实密钥、不保存明文密钥、不输出
//! 明文签名材料。
//!
//! ```compile_fail
//! use arb_signing::real::RealSigningProvider;
//! ```

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
        }
    }

    /// 返回可审计引用；创建请求前的格式错误没有审计引用。
    pub fn audit_ref(&self) -> Option<&SigningAuditRef> {
        match self {
            Self::PolicyMismatch { audit_ref, .. }
            | Self::PurposeNotAllowed { audit_ref, .. }
            | Self::ApprovalRequired { audit_ref }
            | Self::PolicyDisabled { audit_ref }
            | Self::RealSigningUnavailable { audit_ref } => Some(audit_ref),
            Self::InvalidToken { .. }
            | Self::InvalidDigest { .. }
            | Self::InvalidRequest { .. } => None,
        }
    }

    fn attempt_status(&self) -> SigningAttemptStatus {
        match self {
            Self::RealSigningUnavailable { .. } => SigningAttemptStatus::Unavailable,
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
}

impl SigningPolicyMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Disabled => "disabled",
            Self::AuditOnly => "audit_only",
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
