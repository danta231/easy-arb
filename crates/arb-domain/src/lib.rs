//! `arb-domain` 领域基础类型。
//!
//! 中文说明：本 crate 位于最底层，只提供强类型 ID、十进制数值、UTC
//! 时间边界、核心状态枚举和领域错误。它不能依赖任何内部业务模块，也不
//! 表达下单、签名、转账或运行时装配能力。

#![forbid(unsafe_code)]

use std::cmp::Ordering;
use std::error::Error;
use std::fmt;
use std::str::FromStr;

/// 领域层统一返回类型。
///
/// 中文说明：调用方需要显式处理解析失败、越界和非法状态，未知外部状态
/// 不能被当作成功。
pub type DomainResult<T> = Result<T, DomainError>;

/// 领域基础错误。
///
/// 中文说明：这里仅描述领域类型自身的错误，不包含网络、配置、执行或签名错误。
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DomainError {
    /// ID 字符串为空。
    EmptyId { type_name: &'static str },
    /// ID 字符串包含非法字符或格式。
    InvalidId {
        type_name: &'static str,
        value: String,
        reason: &'static str,
    },
    /// decimal 字符串格式非法。
    InvalidDecimal { value: String, reason: &'static str },
    /// decimal 运算或解析超出内部安全范围。
    DecimalOverflow { value: String },
    /// 当前领域类型不允许负数。
    NegativeNotAllowed {
        type_name: &'static str,
        value: Decimal,
    },
    /// UTC 时间字符串或时间分量非法。
    InvalidTimestamp { value: String, reason: &'static str },
    /// 状态枚举值无法识别。
    InvalidState {
        type_name: &'static str,
        value: String,
    },
}

impl fmt::Display for DomainError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyId { type_name } => write!(f, "{type_name} cannot be empty"),
            Self::InvalidId {
                type_name,
                value,
                reason,
            } => write!(f, "{type_name} `{value}` is invalid: {reason}"),
            Self::InvalidDecimal { value, reason } => {
                write!(f, "decimal `{value}` is invalid: {reason}")
            }
            Self::DecimalOverflow { value } => write!(f, "decimal `{value}` is out of range"),
            Self::NegativeNotAllowed { type_name, value } => {
                write!(f, "{type_name} cannot be negative: {value}")
            }
            Self::InvalidTimestamp { value, reason } => {
                write!(f, "UTC timestamp `{value}` is invalid: {reason}")
            }
            Self::InvalidState { type_name, value } => {
                write!(f, "{type_name} state `{value}` is invalid")
            }
        }
    }
}

impl Error for DomainError {}

macro_rules! define_id_type {
    ($name:ident, $doc:literal) => {
        #[doc = $doc]
        ///
        /// 中文说明：ID 使用 newtype 包装，避免不同业务标识被误传。
        #[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
        pub struct $name(String);

        impl $name {
            /// 从原始字符串创建 ID。
            ///
            /// 中文说明：ID 必须非空，只允许 ASCII 字母、数字、`_`、`-`、`.`、`:`、`/`。
            pub fn new(value: impl Into<String>) -> DomainResult<Self> {
                let value = value.into();
                validate_id(stringify!($name), &value)?;
                Ok(Self(value))
            }

            /// 返回底层稳定字符串。
            pub fn as_str(&self) -> &str {
                &self.0
            }

            /// 拆出底层字符串，供合同层序列化使用。
            pub fn into_string(self) -> String {
                self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }

        impl FromStr for $name {
            type Err = DomainError;

            fn from_str(value: &str) -> Result<Self, Self::Err> {
                Self::new(value)
            }
        }
    };
}

/// 交易场所稳定标识。
///
/// ```compile_fail
/// use arb_domain::{AssetId, VenueId};
///
/// fn needs_asset(asset_id: AssetId) {
///     let _ = asset_id;
/// }
///
/// let venue_id = VenueId::new("binance").unwrap();
/// needs_asset(venue_id);
/// ```
pub use ids::VenueId;

mod ids {
    use super::*;

    define_id_type!(VenueId, "交易场所 ID。");
    define_id_type!(AssetId, "资产 ID。");
    define_id_type!(InstrumentId, "工具 ID。");
    define_id_type!(AccountId, "账户或托管位置 ID。");
    define_id_type!(StrategyId, "策略 ID。");
    define_id_type!(EventId, "事件 ID。");
    define_id_type!(PortfolioStateId, "组合状态快照 ID。");
    define_id_type!(CandidateTransitionId, "候选组合转换 ID。");
    define_id_type!(RiskDecisionId, "风控决策 ID。");
    define_id_type!(ExecutionPlanId, "执行计划 ID。");
    define_id_type!(ExecutionReportId, "执行报告 ID。");
    define_id_type!(LedgerEntryId, "账本分录 ID。");
    define_id_type!(IncidentId, "事故 ID。");
    define_id_type!(CorrelationId, "关联链路 ID。");
    define_id_type!(CausationId, "因果链路 ID。");
    define_id_type!(OrderId, "订单引用 ID。");
    define_id_type!(PositionId, "仓位 ID。");
    define_id_type!(ConfigVersionId, "配置版本 ID。");
}

pub use ids::{
    AccountId, AssetId, CandidateTransitionId, CausationId, ConfigVersionId, CorrelationId,
    EventId, ExecutionPlanId, ExecutionReportId, IncidentId, InstrumentId, LedgerEntryId, OrderId,
    PortfolioStateId, PositionId, RiskDecisionId, StrategyId,
};

fn validate_id(type_name: &'static str, value: &str) -> DomainResult<()> {
    if value.is_empty() {
        return Err(DomainError::EmptyId { type_name });
    }

    if value.bytes().any(|byte| {
        !(byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.' | b':' | b'/'))
    }) {
        return Err(DomainError::InvalidId {
            type_name,
            value: value.to_owned(),
            reason:
                "only ASCII letters, digits, underscore, dash, dot, colon and slash are allowed",
        });
    }

    Ok(())
}

/// 精确十进制数。
///
/// 中文说明：Decimal 保存整数原子值和小数位数，解析与显示都不经过二进制浮点，
/// 用于金额、价格、数量、利率、收益和基点等核心路径。
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct Decimal {
    atoms: i128,
    scale: u32,
}

impl Decimal {
    /// 创建一个已经按 `scale` 缩放过的十进制数。
    pub fn from_scaled_atoms(atoms: i128, scale: u32) -> Self {
        Self { atoms, scale }
    }

    /// 返回按小数位缩放后的原子值。
    pub fn atoms(self) -> i128 {
        self.atoms
    }

    /// 返回小数位数。
    pub fn scale(self) -> u32 {
        self.scale
    }

    /// 判断是否小于零。
    pub fn is_negative(self) -> bool {
        self.atoms < 0
    }

    /// 判断是否等于零。
    pub fn is_zero(self) -> bool {
        self.atoms == 0
    }

    /// 安全加法，结果使用两侧较大的小数位数。
    pub fn checked_add(self, other: Self) -> DomainResult<Self> {
        let target_scale = self.scale.max(other.scale);
        let left = self.rescaled_atoms(target_scale)?;
        let right = other.rescaled_atoms(target_scale)?;
        let atoms = left
            .checked_add(right)
            .ok_or_else(|| DomainError::DecimalOverflow {
                value: format!("{self}+{other}"),
            })?;
        Ok(Self {
            atoms,
            scale: target_scale,
        })
    }

    /// 安全减法，结果使用两侧较大的小数位数。
    pub fn checked_sub(self, other: Self) -> DomainResult<Self> {
        let target_scale = self.scale.max(other.scale);
        let left = self.rescaled_atoms(target_scale)?;
        let right = other.rescaled_atoms(target_scale)?;
        let atoms = left
            .checked_sub(right)
            .ok_or_else(|| DomainError::DecimalOverflow {
                value: format!("{self}-{other}"),
            })?;
        Ok(Self {
            atoms,
            scale: target_scale,
        })
    }

    /// 安全取反。
    pub fn checked_neg(self) -> DomainResult<Self> {
        let atoms = self
            .atoms
            .checked_neg()
            .ok_or_else(|| DomainError::DecimalOverflow {
                value: self.to_string(),
            })?;
        Ok(Self {
            atoms,
            scale: self.scale,
        })
    }

    fn rescaled_atoms(self, target_scale: u32) -> DomainResult<i128> {
        if target_scale < self.scale {
            return Err(DomainError::InvalidDecimal {
                value: self.to_string(),
                reason: "target scale cannot be lower than current scale",
            });
        }
        let scale_delta = target_scale - self.scale;
        let multiplier =
            checked_pow10_i128(scale_delta).ok_or_else(|| DomainError::DecimalOverflow {
                value: self.to_string(),
            })?;
        self.atoms
            .checked_mul(multiplier)
            .ok_or_else(|| DomainError::DecimalOverflow {
                value: self.to_string(),
            })
    }
}

impl fmt::Display for Decimal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let negative = self.atoms < 0;
        let abs_atoms = self.atoms.unsigned_abs();
        let scale = self.scale as usize;
        let digits = abs_atoms.to_string();

        if negative {
            f.write_str("-")?;
        }

        if scale == 0 {
            return f.write_str(&digits);
        }

        if digits.len() > scale {
            let split = digits.len() - scale;
            write!(f, "{}.{}", &digits[..split], &digits[split..])
        } else {
            f.write_str("0.")?;
            for _ in 0..(scale - digits.len()) {
                f.write_str("0")?;
            }
            f.write_str(&digits)
        }
    }
}

impl FromStr for Decimal {
    type Err = DomainError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        parse_decimal(value)
    }
}

impl PartialOrd for Decimal {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        let scale = self.scale.max(other.scale);
        let left = self.rescaled_atoms(scale).ok()?;
        let right = other.rescaled_atoms(scale).ok()?;
        Some(left.cmp(&right))
    }
}

macro_rules! define_decimal_type {
    ($name:ident, $allow_negative:expr, $doc:literal) => {
        #[doc = $doc]
        ///
        /// 中文说明：该类型包装 Decimal，核心路径不使用二进制浮点。
        #[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, PartialOrd)]
        pub struct $name(Decimal);

        impl $name {
            /// 从 Decimal 创建领域数值。
            pub fn new(value: Decimal) -> DomainResult<Self> {
                if !$allow_negative && value.is_negative() {
                    return Err(DomainError::NegativeNotAllowed {
                        type_name: stringify!($name),
                        value,
                    });
                }
                Ok(Self(value))
            }

            /// 返回内部 Decimal 值。
            pub fn as_decimal(self) -> Decimal {
                self.0
            }

            /// 返回原子值。
            pub fn atoms(self) -> i128 {
                self.0.atoms()
            }

            /// 返回小数位数。
            pub fn scale(self) -> u32 {
                self.0.scale()
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.0.fmt(f)
            }
        }

        impl FromStr for $name {
            type Err = DomainError;

            fn from_str(value: &str) -> Result<Self, Self::Err> {
                Self::new(Decimal::from_str(value)?)
            }
        }
    };
}

define_decimal_type!(Amount, false, "金额。");
define_decimal_type!(Quantity, false, "数量。");
define_decimal_type!(Price, false, "价格。");
define_decimal_type!(Rate, true, "利率。");
define_decimal_type!(Yield, true, "收益率或收益度量。");
define_decimal_type!(BasisPoints, true, "基点。");
define_decimal_type!(Pnl, true, "盈亏。");

fn parse_decimal(value: &str) -> DomainResult<Decimal> {
    if value.is_empty() {
        return Err(invalid_decimal(value, "value cannot be empty"));
    }
    if value.trim() != value {
        return Err(invalid_decimal(
            value,
            "leading or trailing whitespace is not allowed",
        ));
    }
    if value.contains(['e', 'E']) {
        return Err(invalid_decimal(value, "exponent notation is not allowed"));
    }

    let unsigned = value.strip_prefix('-').unwrap_or(value);
    let negative = unsigned.len() != value.len();
    if unsigned.is_empty() {
        return Err(invalid_decimal(value, "missing digits"));
    }

    let mut dot_seen = false;
    let mut digits_seen = false;
    let mut fractional_digits = 0_u32;
    let mut atoms = 0_i128;

    for byte in unsigned.bytes() {
        match byte {
            b'0'..=b'9' => {
                digits_seen = true;
                if dot_seen {
                    fractional_digits = fractional_digits.checked_add(1).ok_or_else(|| {
                        DomainError::DecimalOverflow {
                            value: value.to_owned(),
                        }
                    })?;
                }
                atoms = atoms
                    .checked_mul(10)
                    .and_then(|current| current.checked_add((byte - b'0') as i128))
                    .ok_or_else(|| DomainError::DecimalOverflow {
                        value: value.to_owned(),
                    })?;
            }
            b'.' => {
                if dot_seen {
                    return Err(invalid_decimal(value, "multiple decimal points"));
                }
                dot_seen = true;
            }
            _ => {
                return Err(invalid_decimal(
                    value,
                    "only digits, one decimal point and an optional leading minus are allowed",
                ));
            }
        }
    }

    if !digits_seen {
        return Err(invalid_decimal(value, "missing digits"));
    }
    if negative && atoms == 0 {
        return Err(invalid_decimal(value, "negative zero is not allowed"));
    }
    if dot_seen && unsigned.ends_with('.') {
        return Err(invalid_decimal(
            value,
            "fractional part must contain at least one digit",
        ));
    }
    if unsigned.starts_with('.') {
        return Err(invalid_decimal(
            value,
            "integer part must contain at least one digit",
        ));
    }

    if negative {
        atoms = atoms
            .checked_neg()
            .ok_or_else(|| DomainError::DecimalOverflow {
                value: value.to_owned(),
            })?;
    }

    Ok(Decimal {
        atoms,
        scale: fractional_digits,
    })
}

fn invalid_decimal(value: &str, reason: &'static str) -> DomainError {
    DomainError::InvalidDecimal {
        value: value.to_owned(),
        reason,
    }
}

fn checked_pow10_i128(exponent: u32) -> Option<i128> {
    let mut value = 1_i128;
    for _ in 0..exponent {
        value = value.checked_mul(10)?;
    }
    Some(value)
}

/// UTC 时间戳边界。
///
/// 中文说明：领域层只接受带 `Z` 的 UTC RFC3339 秒级或纳秒级字符串，
/// 不接受本地时区、隐式时区或偏移时区。
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct UtcTimestamp {
    unix_seconds: i64,
    nanoseconds: u32,
}

impl UtcTimestamp {
    /// 从 Unix 秒和纳秒创建 UTC 时间戳。
    pub fn from_unix_parts(unix_seconds: i64, nanoseconds: u32) -> DomainResult<Self> {
        if nanoseconds >= NANOS_PER_SECOND {
            return Err(DomainError::InvalidTimestamp {
                value: format!("{unix_seconds}.{nanoseconds:09}"),
                reason: "nanoseconds must be below one second",
            });
        }

        let days = unix_seconds.div_euclid(SECONDS_PER_DAY);
        let (year, _, _) = civil_from_days(days);
        if !(0..=9999).contains(&year) {
            return Err(DomainError::InvalidTimestamp {
                value: unix_seconds.to_string(),
                reason: "year must be within 0000..=9999",
            });
        }

        Ok(Self {
            unix_seconds,
            nanoseconds,
        })
    }

    /// 解析严格 UTC RFC3339 字符串，例如 `2026-05-10T12:00:00Z`。
    pub fn parse_rfc3339_z(value: &str) -> DomainResult<Self> {
        parse_utc_timestamp(value)
    }

    /// 返回 Unix 秒。
    pub fn unix_seconds(self) -> i64 {
        self.unix_seconds
    }

    /// 返回纳秒分量。
    pub fn nanoseconds(self) -> u32 {
        self.nanoseconds
    }
}

impl fmt::Display for UtcTimestamp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let days = self.unix_seconds.div_euclid(SECONDS_PER_DAY);
        let seconds_of_day = self.unix_seconds.rem_euclid(SECONDS_PER_DAY);
        let (year, month, day) = civil_from_days(days);
        let hour = seconds_of_day / 3_600;
        let minute = (seconds_of_day % 3_600) / 60;
        let second = seconds_of_day % 60;

        write!(
            f,
            "{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}"
        )?;
        if self.nanoseconds != 0 {
            let mut fraction = format!("{:09}", self.nanoseconds);
            while fraction.ends_with('0') {
                fraction.pop();
            }
            write!(f, ".{fraction}")?;
        }
        f.write_str("Z")
    }
}

impl FromStr for UtcTimestamp {
    type Err = DomainError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::parse_rfc3339_z(value)
    }
}

const SECONDS_PER_DAY: i64 = 86_400;
const NANOS_PER_SECOND: u32 = 1_000_000_000;

fn parse_utc_timestamp(value: &str) -> DomainResult<UtcTimestamp> {
    if !value.ends_with('Z') {
        return Err(invalid_timestamp(value, "timestamp must end with Z"));
    }

    let without_z = &value[..value.len() - 1];
    let (date, time) = without_z
        .split_once('T')
        .ok_or_else(|| invalid_timestamp(value, "timestamp must contain T separator"))?;

    let (year, month, day) = parse_date(value, date)?;
    let (hour, minute, second, nanoseconds) = parse_time(value, time)?;
    let days = days_from_civil(year, month, day);
    let seconds = days
        .checked_mul(SECONDS_PER_DAY)
        .and_then(|base| base.checked_add((hour * 3_600 + minute * 60 + second) as i64))
        .ok_or_else(|| invalid_timestamp(value, "timestamp is out of range"))?;

    UtcTimestamp::from_unix_parts(seconds, nanoseconds)
}

fn parse_date(original: &str, value: &str) -> DomainResult<(i32, u32, u32)> {
    if value.len() != 10 {
        return Err(invalid_timestamp(original, "date must use YYYY-MM-DD"));
    }
    if !value.is_ascii() {
        return Err(invalid_timestamp(original, "date must be ASCII"));
    }
    if value.as_bytes()[4] != b'-' || value.as_bytes()[7] != b'-' {
        return Err(invalid_timestamp(original, "date must use YYYY-MM-DD"));
    }
    let year = parse_fixed_digits(original, &value[0..4], "year")? as i32;
    let month = parse_fixed_digits(original, &value[5..7], "month")?;
    let day = parse_fixed_digits(original, &value[8..10], "day")?;
    if month == 0 || month > 12 {
        return Err(invalid_timestamp(original, "month must be in 1..=12"));
    }
    let max_day = days_in_month(year, month);
    if day == 0 || day > max_day {
        return Err(invalid_timestamp(original, "day is not valid for month"));
    }
    Ok((year, month, day))
}

fn parse_time(original: &str, value: &str) -> DomainResult<(u32, u32, u32, u32)> {
    if value.len() < 8 {
        return Err(invalid_timestamp(
            original,
            "time must use HH:MM:SS with optional fractional seconds",
        ));
    }
    if !value.is_ascii() {
        return Err(invalid_timestamp(original, "time must be ASCII"));
    }
    if value.as_bytes()[2] != b':' || value.as_bytes()[5] != b':' {
        return Err(invalid_timestamp(original, "time must use HH:MM:SS"));
    }

    let hour = parse_fixed_digits(original, &value[0..2], "hour")?;
    let minute = parse_fixed_digits(original, &value[3..5], "minute")?;
    let second = parse_fixed_digits(original, &value[6..8], "second")?;
    if hour > 23 {
        return Err(invalid_timestamp(original, "hour must be in 0..=23"));
    }
    if minute > 59 {
        return Err(invalid_timestamp(original, "minute must be in 0..=59"));
    }
    if second > 59 {
        return Err(invalid_timestamp(original, "second must be in 0..=59"));
    }

    let nanoseconds = if value.len() == 8 {
        0
    } else {
        let fraction = value
            .strip_prefix(&value[..8])
            .and_then(|rest| rest.strip_prefix('.'))
            .ok_or_else(|| {
                invalid_timestamp(original, "fractional seconds must start with decimal point")
            })?;
        parse_fractional_nanos(original, fraction)?
    };

    Ok((hour, minute, second, nanoseconds))
}

fn parse_fixed_digits(original: &str, value: &str, name: &'static str) -> DomainResult<u32> {
    if !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(invalid_timestamp(original, name));
    }
    value
        .parse::<u32>()
        .map_err(|_| invalid_timestamp(original, name))
}

fn parse_fractional_nanos(original: &str, value: &str) -> DomainResult<u32> {
    if value.is_empty() {
        return Err(invalid_timestamp(
            original,
            "fractional seconds cannot be empty",
        ));
    }
    if value.len() > 9 {
        return Err(invalid_timestamp(
            original,
            "fractional seconds support at most 9 digits",
        ));
    }
    if !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(invalid_timestamp(
            original,
            "fractional seconds must contain only digits",
        ));
    }

    let mut nanos = 0_u32;
    for byte in value.bytes() {
        nanos = nanos * 10 + u32::from(byte - b'0');
    }
    for _ in value.len()..9 {
        nanos *= 10;
    }
    Ok(nanos)
}

fn invalid_timestamp(value: &str, reason: &'static str) -> DomainError {
    DomainError::InvalidTimestamp {
        value: value.to_owned(),
        reason,
    }
}

fn is_leap_year(year: i32) -> bool {
    (year % 4 == 0 && year % 100 != 0) || year % 400 == 0
}

fn days_in_month(year: i32, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if is_leap_year(year) => 29,
        2 => 28,
        _ => 0,
    }
}

fn days_from_civil(year: i32, month: u32, day: u32) -> i64 {
    let year = i64::from(year) - i64::from(month <= 2);
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let year_of_era = year - era * 400;
    let month = i64::from(month);
    let day = i64::from(day);
    let day_of_year = (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

fn civil_from_days(days: i64) -> (i32, u32, u32) {
    let days = days + 719_468;
    let era = if days >= 0 { days } else { days - 146_096 } / 146_097;
    let day_of_era = days - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = month_prime + if month_prime < 10 { 3 } else { -9 };
    let year = year + i64::from(month <= 2);
    (year as i32, month as u32, day as u32)
}

macro_rules! define_state_enum {
    (
        $name:ident,
        $doc:literal,
        [$($variant:ident => $wire:literal),+ $(,)?]
    ) => {
        #[doc = $doc]
        ///
        /// 中文说明：状态枚举必须显式列出，未知字符串会被拒绝。
        #[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
        pub enum $name {
            $( $variant, )+
        }

        impl $name {
            /// 返回合同层使用的稳定字符串。
            pub fn as_str(self) -> &'static str {
                match self {
                    $( Self::$variant => $wire, )+
                }
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(self.as_str())
            }
        }

        impl FromStr for $name {
            type Err = DomainError;

            fn from_str(value: &str) -> Result<Self, Self::Err> {
                match value {
                    $( $wire => Ok(Self::$variant), )+
                    _ => Err(DomainError::InvalidState {
                        type_name: stringify!($name),
                        value: value.to_owned(),
                    }),
                }
            }
        }
    };
}

define_state_enum!(
    ExecutionStatus,
    "执行生命周期状态。",
    [
        Planned => "planned",
        PendingManualApproval => "pending_manual_approval",
        Simulating => "simulating",
        Dispatching => "dispatching",
        PartiallyFilled => "partially_filled",
        Filled => "filled",
        Failed => "failed",
        Unknown => "unknown",
        Compensating => "compensating",
        Completed => "completed",
        Cancelled => "cancelled"
    ]
);

define_state_enum!(
    RiskStatus,
    "风控决策状态。",
    [
        Approved => "approved",
        ApprovedWithConstraints => "approved_with_constraints",
        Rejected => "rejected",
        NeedsManualApproval => "needs_manual_approval",
        NeedsMoreData => "needs_more_data",
        PausedByKillSwitch => "paused_by_kill_switch"
    ]
);

define_state_enum!(
    CapitalReservationStatus,
    "资本预留状态。",
    [
        Requested => "requested",
        Reserved => "reserved",
        InExecution => "in_execution",
        Released => "released",
        Expired => "expired",
        ReconciliationMismatch => "reconciliation_mismatch",
        Failed => "failed"
    ]
);

define_state_enum!(
    IncidentStatus,
    "事故处理状态。",
    [
        Open => "open",
        Acknowledged => "acknowledged",
        Mitigating => "mitigating",
        Resolved => "resolved",
        Closed => "closed"
    ]
);

define_state_enum!(
    IncidentSeverity,
    "事故严重等级。",
    [
        Low => "low",
        Medium => "medium",
        High => "high",
        Critical => "critical"
    ]
);

define_state_enum!(
    VenueHealthStatus,
    "场所健康状态。",
    [
        Healthy => "healthy",
        Degraded => "degraded",
        Unhealthy => "unhealthy",
        Unknown => "unknown"
    ]
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn id_types_are_distinct_and_validate_format() {
        let venue = VenueId::new("binance/spot").expect("valid venue id");
        let asset = AssetId::new("asset:USDC").expect("valid asset id");

        assert_eq!(venue.as_str(), "binance/spot");
        assert_eq!(asset.as_str(), "asset:USDC");
        assert!(VenueId::new("").is_err());
        assert!(AssetId::new("bad id").is_err());
    }

    #[test]
    fn decimal_string_roundtrip_keeps_scale_and_precision() {
        let cases = [
            "0",
            "0.000000000000000001",
            "1.2300",
            "123456789012345678901234567890.123456789",
            "-42.0100",
        ];

        for case in cases {
            let parsed = Decimal::from_str(case).expect("decimal parses");
            assert_eq!(parsed.to_string(), case);
        }
    }

    #[test]
    fn decimal_rejects_lossy_or_ambiguous_forms() {
        for case in ["", " 1", "1 ", "1e-3", ".1", "1.", "1.2.3", "+1"] {
            assert!(Decimal::from_str(case).is_err(), "{case} should fail");
        }
    }

    #[test]
    fn decimal_domain_types_enforce_sign_boundaries() {
        assert!(Amount::from_str("10.50").is_ok());
        assert!(Price::from_str("0.01").is_ok());
        assert!(Quantity::from_str("-1").is_err());
        assert!(Rate::from_str("-0.0100").is_ok());
        assert!(Pnl::from_str("-99.99").is_ok());
    }

    #[test]
    fn decimal_safe_arithmetic_aligns_scales() {
        let left = Decimal::from_str("1.20").expect("left");
        let right = Decimal::from_str("0.003").expect("right");

        assert_eq!(left.checked_add(right).expect("sum").to_string(), "1.203");
        assert_eq!(
            left.checked_sub(right).expect("difference").to_string(),
            "1.197"
        );
    }

    #[test]
    fn core_domain_source_does_not_contain_binary_float_type() {
        let source = include_str!("lib.rs");
        let forbidden = ['f', '6', '4'].iter().collect::<String>();
        assert!(!source.contains(&forbidden));
    }

    #[test]
    fn utc_timestamp_accepts_only_explicit_utc() {
        let timestamp =
            UtcTimestamp::from_str("2026-05-10T12:34:56.123456789Z").expect("timestamp");

        assert_eq!(timestamp.unix_seconds(), 1_778_416_496);
        assert_eq!(timestamp.nanoseconds(), 123_456_789);
        assert_eq!(timestamp.to_string(), "2026-05-10T12:34:56.123456789Z");
        assert!(UtcTimestamp::from_str("2026-05-10T12:34:56+08:00").is_err());
        assert!(UtcTimestamp::from_str("2026-02-29T00:00:00Z").is_err());
        assert!(UtcTimestamp::from_str("2024-02-29T00:00:00Z").is_ok());
    }

    #[test]
    fn state_enums_parse_known_values_and_reject_unknown_values() {
        assert_eq!(
            RiskStatus::from_str("needs_manual_approval").expect("risk status"),
            RiskStatus::NeedsManualApproval
        );
        assert_eq!(ExecutionStatus::Unknown.as_str(), "unknown");
        assert!(RiskStatus::from_str("silently_approved").is_err());
        assert!(CapitalReservationStatus::from_str("reserved").is_ok());
        assert!(IncidentSeverity::from_str("critical").is_ok());
        assert!(VenueHealthStatus::from_str("unknown").is_ok());
    }
}
