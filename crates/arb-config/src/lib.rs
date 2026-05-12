//! `arb-config` 只读配置边界。
//!
//! 中文说明：本 crate 只把 YAML 配置读取为已校验、不可变的配置对象，并计算
//! 稳定配置哈希。读取配置不会触发风控、执行、签名、转账或账户状态变化。

#![forbid(unsafe_code)]

use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

/// 当前支持的配置 schema 版本。
pub const SUPPORTED_CONFIG_VERSION: &str = "arb-config-v1";

/// 配置层统一返回类型。
pub type ConfigResult<T> = Result<T, ConfigError>;

/// 配置解析和校验错误。
///
/// 中文说明：配置错误必须显式失败，不能把未知字段、危险字段或非法实盘设置当作成功。
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ConfigError {
    Io { path: PathBuf, message: String },
    Parse { line: usize, message: String },
    MissingField { path: String },
    DuplicateField { path: String },
    UnknownField { path: String },
    SensitiveField { path: String },
    InvalidValue { path: String, message: String },
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { path, message } => {
                write!(f, "{}: failed to read config: {message}", path.display())
            }
            Self::Parse { line, message } => {
                write!(f, "line {line}: invalid config YAML: {message}")
            }
            Self::MissingField { path } => write!(f, "{path}: missing required field"),
            Self::DuplicateField { path } => write!(f, "{path}: duplicate field"),
            Self::UnknownField { path } => write!(f, "{path}: unknown field"),
            Self::SensitiveField { path } => {
                write!(f, "{path}: sensitive key material field is not allowed")
            }
            Self::InvalidValue { path, message } => write!(f, "{path}: invalid value: {message}"),
        }
    }
}

impl Error for ConfigError {}

/// 已校验的只读平台配置。
///
/// 中文说明：字段保持私有，调用方只能通过 getter 读取，不能在配置对象上原地修改。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ArbConfig {
    version: ConfigVersion,
    hash: ConfigHash,
    execution: ExecutionConfig,
    kill_switch: KillSwitchConfig,
    signing: SigningConfig,
}

impl ArbConfig {
    /// 从 YAML 字符串读取配置。
    pub fn from_yaml_str(input: &str) -> ConfigResult<Self> {
        let raw = parse_config_yaml(input)?;
        let version = ConfigVersion::new(required_top_string(&raw, "config_version")?)?;

        let execution = ExecutionConfig::from_raw(required_section(&raw, "execution")?)?;
        let kill_switch = KillSwitchConfig::from_raw(required_section(&raw, "kill_switch")?)?;
        let signing = SigningConfig::from_raw(required_section(&raw, "signing")?)?;

        reject_unknown_top_fields(&raw)?;

        let mut config = Self {
            version,
            hash: ConfigHash(String::new()),
            execution,
            kill_switch,
            signing,
        };
        config.validate()?;
        config.hash = ConfigHash::for_config(&config);
        Ok(config)
    }

    /// 从文件读取配置。
    ///
    /// 中文说明：该函数只读文件内容，不写文件、不访问网络、不改变账户状态。
    pub fn from_path(path: impl AsRef<Path>) -> ConfigResult<Self> {
        let path = path.as_ref();
        let input = fs::read_to_string(path).map_err(|error| ConfigError::Io {
            path: path.to_path_buf(),
            message: error.to_string(),
        })?;
        Self::from_yaml_str(&input)
    }

    /// 返回配置版本。
    pub fn version(&self) -> &ConfigVersion {
        &self.version
    }

    /// 返回稳定配置哈希。
    pub fn hash(&self) -> &ConfigHash {
        &self.hash
    }

    /// 返回执行模式配置。
    pub fn execution(&self) -> &ExecutionConfig {
        &self.execution
    }

    /// 返回熔断开关配置。
    pub fn kill_switch(&self) -> &KillSwitchConfig {
        &self.kill_switch
    }

    /// 返回签名策略引用配置。
    pub fn signing(&self) -> &SigningConfig {
        &self.signing
    }

    /// 判断当前配置是否允许真实账户变化。
    ///
    /// 中文说明：配置读取本身不会执行任何动作；该方法只是给后续运行时读取权限边界。
    pub fn allows_account_changes(&self) -> bool {
        self.execution.mode.requires_live_permission()
            && self.execution.live_execution_enabled
            && !self.kill_switch.blocks_execution_mode(self.execution.mode)
    }

    fn validate(&self) -> ConfigResult<()> {
        if self.version.as_str() != SUPPORTED_CONFIG_VERSION {
            return Err(invalid_value(
                "$.config_version",
                format!("expected `{SUPPORTED_CONFIG_VERSION}`"),
            ));
        }

        self.execution.validate()?;
        self.signing.validate_against_execution(&self.execution)?;
        Ok(())
    }

    fn canonical_body(&self) -> String {
        let kill = &self.kill_switch;
        [
            format!("config_version={}", self.version.as_str()),
            format!(
                "execution.auto_live_enabled={}",
                self.execution.auto_live_enabled
            ),
            format!(
                "execution.live_execution_enabled={}",
                self.execution.live_execution_enabled
            ),
            format!("execution.mode={}", self.execution.mode.as_str()),
            format!("kill_switch.accounts={}", canonical_list(kill.accounts())),
            format!("kill_switch.assets={}", canonical_list(kill.assets())),
            format!("kill_switch.chains={}", canonical_list(kill.chains())),
            format!("kill_switch.execution={}", kill.execution()),
            format!(
                "kill_switch.execution_modes={}",
                canonical_modes(kill.execution_modes())
            ),
            format!("kill_switch.global={}", kill.global()),
            format!(
                "kill_switch.instruments={}",
                canonical_list(kill.instruments())
            ),
            format!(
                "kill_switch.strategies={}",
                canonical_list(kill.strategies())
            ),
            format!("kill_switch.venues={}", canonical_list(kill.venues())),
            format!("signing.policy_ref={}", self.signing.policy_ref().as_str()),
            format!(
                "signing.real_signing_enabled={}",
                self.signing.real_signing_enabled()
            ),
        ]
        .join("\n")
    }
}

/// 配置版本。
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ConfigVersion(String);

impl ConfigVersion {
    fn new(value: String) -> ConfigResult<Self> {
        validate_reference("$.config_version", &value)?;
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// 稳定配置哈希。
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ConfigHash(String);

impl ConfigHash {
    fn for_config(config: &ArbConfig) -> Self {
        Self(format!(
            "sha256:{}",
            sha256_hex(config.canonical_body().as_bytes())
        ))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// 执行权限模式。
///
/// 中文说明：执行模式是权限策略，不是架构分支。配置 crate 只读取模式，不执行动作。
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum ExecutionMode {
    ReadOnly,
    Simulated,
    ManualApproval,
    GuardedLive,
    AutonomousLive,
}

impl ExecutionMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ReadOnly => "ReadOnly",
            Self::Simulated => "Simulated",
            Self::ManualApproval => "ManualApproval",
            Self::GuardedLive => "GuardedLive",
            Self::AutonomousLive => "AutonomousLive",
        }
    }

    pub fn requires_live_permission(self) -> bool {
        matches!(self, Self::GuardedLive | Self::AutonomousLive)
    }

    fn from_str(path: &str, value: &str) -> ConfigResult<Self> {
        match value {
            "ReadOnly" => Ok(Self::ReadOnly),
            "Simulated" => Ok(Self::Simulated),
            "ManualApproval" => Ok(Self::ManualApproval),
            "GuardedLive" => Ok(Self::GuardedLive),
            "AutonomousLive" => Ok(Self::AutonomousLive),
            other => Err(invalid_value(
                path,
                format!("unsupported execution mode `{other}`"),
            )),
        }
    }
}

impl fmt::Display for ExecutionMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// 执行配置。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExecutionConfig {
    mode: ExecutionMode,
    live_execution_enabled: bool,
    auto_live_enabled: bool,
}

impl ExecutionConfig {
    fn from_raw(raw: &RawSection) -> ConfigResult<Self> {
        reject_unknown_section_fields(
            "$.execution",
            raw,
            &["mode", "live_execution_enabled", "auto_live_enabled"],
        )?;

        let mode = ExecutionMode::from_str(
            "$.execution.mode",
            &required_section_string(raw, "$.execution", "mode")?,
        )?;
        let live_execution_enabled =
            required_section_bool(raw, "$.execution", "live_execution_enabled")?;
        let auto_live_enabled = required_section_bool(raw, "$.execution", "auto_live_enabled")?;

        Ok(Self {
            mode,
            live_execution_enabled,
            auto_live_enabled,
        })
    }

    pub fn mode(&self) -> ExecutionMode {
        self.mode
    }

    pub fn live_execution_enabled(&self) -> bool {
        self.live_execution_enabled
    }

    pub fn auto_live_enabled(&self) -> bool {
        self.auto_live_enabled
    }

    pub fn live_execution_default_closed(&self) -> bool {
        !self.live_execution_enabled
            && !self.auto_live_enabled
            && self.mode == ExecutionMode::ReadOnly
    }

    fn validate(&self) -> ConfigResult<()> {
        if self.auto_live_enabled && !self.live_execution_enabled {
            return Err(invalid_value(
                "$.execution.auto_live_enabled",
                "auto live cannot be enabled unless live execution is enabled",
            ));
        }

        match self.mode {
            ExecutionMode::ReadOnly | ExecutionMode::Simulated | ExecutionMode::ManualApproval => {
                if self.live_execution_enabled || self.auto_live_enabled {
                    return Err(invalid_value(
                        "$.execution",
                        "non-live execution modes must not enable live execution flags",
                    ));
                }
            }
            ExecutionMode::GuardedLive => {
                if !self.live_execution_enabled {
                    return Err(invalid_value(
                        "$.execution.live_execution_enabled",
                        "GuardedLive requires explicit live_execution_enabled=true",
                    ));
                }
                if self.auto_live_enabled {
                    return Err(invalid_value(
                        "$.execution.auto_live_enabled",
                        "GuardedLive must not enable autonomous live execution",
                    ));
                }
            }
            ExecutionMode::AutonomousLive => {
                if !self.live_execution_enabled || !self.auto_live_enabled {
                    return Err(invalid_value(
                        "$.execution",
                        "AutonomousLive requires both live_execution_enabled=true and auto_live_enabled=true",
                    ));
                }
            }
        }

        Ok(())
    }
}

/// 熔断开关配置。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct KillSwitchConfig {
    global: bool,
    execution: bool,
    strategies: Vec<String>,
    venues: Vec<String>,
    accounts: Vec<String>,
    instruments: Vec<String>,
    assets: Vec<String>,
    chains: Vec<String>,
    execution_modes: Vec<ExecutionMode>,
}

impl KillSwitchConfig {
    fn from_raw(raw: &RawSection) -> ConfigResult<Self> {
        reject_unknown_section_fields(
            "$.kill_switch",
            raw,
            &[
                "global",
                "execution",
                "strategies",
                "venues",
                "accounts",
                "instruments",
                "assets",
                "chains",
                "execution_modes",
            ],
        )?;

        let execution_mode_values = required_section_list(raw, "$.kill_switch", "execution_modes")?;
        let mut execution_modes = Vec::with_capacity(execution_mode_values.len());
        for value in execution_mode_values {
            execution_modes.push(ExecutionMode::from_str(
                "$.kill_switch.execution_modes",
                &value,
            )?);
        }

        let config = Self {
            global: required_section_bool(raw, "$.kill_switch", "global")?,
            execution: required_section_bool(raw, "$.kill_switch", "execution")?,
            strategies: required_reference_list(raw, "$.kill_switch", "strategies")?,
            venues: required_reference_list(raw, "$.kill_switch", "venues")?,
            accounts: required_reference_list(raw, "$.kill_switch", "accounts")?,
            instruments: required_reference_list(raw, "$.kill_switch", "instruments")?,
            assets: required_reference_list(raw, "$.kill_switch", "assets")?,
            chains: required_reference_list(raw, "$.kill_switch", "chains")?,
            execution_modes,
        };

        reject_duplicate_list("$.kill_switch.execution_modes", config.execution_modes())?;
        Ok(config)
    }

    pub fn global(&self) -> bool {
        self.global
    }

    pub fn execution(&self) -> bool {
        self.execution
    }

    pub fn strategies(&self) -> &[String] {
        &self.strategies
    }

    pub fn venues(&self) -> &[String] {
        &self.venues
    }

    pub fn accounts(&self) -> &[String] {
        &self.accounts
    }

    pub fn instruments(&self) -> &[String] {
        &self.instruments
    }

    pub fn assets(&self) -> &[String] {
        &self.assets
    }

    pub fn chains(&self) -> &[String] {
        &self.chains
    }

    pub fn execution_modes(&self) -> &[ExecutionMode] {
        &self.execution_modes
    }

    pub fn is_triggered(&self) -> bool {
        self.global
            || self.execution
            || !self.strategies.is_empty()
            || !self.venues.is_empty()
            || !self.accounts.is_empty()
            || !self.instruments.is_empty()
            || !self.assets.is_empty()
            || !self.chains.is_empty()
            || !self.execution_modes.is_empty()
    }

    pub fn blocks_execution_mode(&self, mode: ExecutionMode) -> bool {
        self.global || self.execution || self.execution_modes.contains(&mode)
    }

    pub fn blocks_strategy(&self, strategy_id: &str) -> bool {
        self.global || self.strategies.iter().any(|value| value == strategy_id)
    }

    pub fn blocks_venue(&self, venue_id: &str) -> bool {
        self.global || self.venues.iter().any(|value| value == venue_id)
    }

    pub fn blocks_account(&self, account_id: &str) -> bool {
        self.global || self.accounts.iter().any(|value| value == account_id)
    }

    pub fn blocks_instrument(&self, instrument_id: &str) -> bool {
        self.global || self.instruments.iter().any(|value| value == instrument_id)
    }

    pub fn blocks_asset(&self, asset_id: &str) -> bool {
        self.global || self.assets.iter().any(|value| value == asset_id)
    }

    pub fn blocks_chain(&self, chain_id: &str) -> bool {
        self.global || self.chains.iter().any(|value| value == chain_id)
    }
}

/// 签名策略引用配置。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SigningConfig {
    policy_ref: SigningPolicyRef,
    real_signing_enabled: bool,
}

impl SigningConfig {
    fn from_raw(raw: &RawSection) -> ConfigResult<Self> {
        reject_unknown_section_fields("$.signing", raw, &["policy_ref", "real_signing_enabled"])?;

        Ok(Self {
            policy_ref: SigningPolicyRef::new(required_section_string(
                raw,
                "$.signing",
                "policy_ref",
            )?)?,
            real_signing_enabled: required_section_bool(raw, "$.signing", "real_signing_enabled")?,
        })
    }

    pub fn policy_ref(&self) -> &SigningPolicyRef {
        &self.policy_ref
    }

    pub fn real_signing_enabled(&self) -> bool {
        self.real_signing_enabled
    }

    fn validate_against_execution(&self, execution: &ExecutionConfig) -> ConfigResult<()> {
        if self.real_signing_enabled && !execution.live_execution_enabled {
            return Err(invalid_value(
                "$.signing.real_signing_enabled",
                "real signing requires explicit live execution permission",
            ));
        }
        Ok(())
    }
}

/// 签名策略引用。
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct SigningPolicyRef(String);

impl SigningPolicyRef {
    fn new(value: String) -> ConfigResult<Self> {
        validate_reference("$.signing.policy_ref", &value)?;
        let allowed_prefix = value.starts_with("signing-policy/")
            || value.starts_with("mock-policy/")
            || value.starts_with("hardware-policy/")
            || value.starts_with("kms-policy/");
        if !allowed_prefix {
            return Err(invalid_value(
                "$.signing.policy_ref",
                "must be a signing policy reference, not key material",
            ));
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum RawValue {
    Bool(bool),
    String(String),
    List(Vec<String>),
}

type RawSection = BTreeMap<String, RawValue>;

#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct RawConfig {
    top_scalars: BTreeMap<String, RawValue>,
    sections: BTreeMap<String, RawSection>,
}

fn parse_config_yaml(input: &str) -> ConfigResult<RawConfig> {
    let mut raw = RawConfig::default();
    let mut current_section: Option<String> = None;

    for (index, original_line) in input.lines().enumerate() {
        let line_number = index + 1;
        if original_line.contains('\t') {
            return Err(parse_error(line_number, "tabs are not allowed"));
        }

        let without_comment = strip_yaml_comment(original_line);
        let trimmed_end = without_comment.trim_end();
        if trimmed_end.trim().is_empty() {
            continue;
        }

        let indent = trimmed_end.len() - trimmed_end.trim_start_matches(' ').len();
        if indent != 0 && indent != 2 {
            return Err(parse_error(
                line_number,
                "only top-level and two-space sections are supported",
            ));
        }

        let content = trimmed_end.trim_start();
        let Some((raw_key, raw_value)) = content.split_once(':') else {
            return Err(parse_error(line_number, "expected `key: value`"));
        };

        let key = raw_key.trim();
        if key.is_empty() || key.contains(' ') {
            return Err(parse_error(line_number, "invalid key"));
        }
        reject_sensitive_key(line_number, key)?;

        let value = raw_value.trim();
        if indent == 0 {
            if value.is_empty() {
                insert_section(&mut raw, line_number, key)?;
                current_section = Some(key.to_owned());
            } else {
                insert_top_scalar(
                    &mut raw,
                    line_number,
                    key,
                    parse_raw_value(line_number, value)?,
                )?;
                current_section = None;
            }
        } else {
            let Some(section) = current_section.as_ref() else {
                return Err(parse_error(
                    line_number,
                    "nested field must follow a section",
                ));
            };
            if value.is_empty() {
                return Err(parse_error(
                    line_number,
                    "nested sections are not supported",
                ));
            }
            insert_section_scalar(
                &mut raw,
                line_number,
                section,
                key,
                parse_raw_value(line_number, value)?,
            )?;
        }
    }

    Ok(raw)
}

fn insert_section(raw: &mut RawConfig, line: usize, key: &str) -> ConfigResult<()> {
    if raw.top_scalars.contains_key(key) || raw.sections.contains_key(key) {
        return Err(duplicate_line(line, key));
    }
    raw.sections.insert(key.to_owned(), BTreeMap::new());
    Ok(())
}

fn insert_top_scalar(
    raw: &mut RawConfig,
    line: usize,
    key: &str,
    value: RawValue,
) -> ConfigResult<()> {
    if raw.top_scalars.contains_key(key) || raw.sections.contains_key(key) {
        return Err(duplicate_line(line, key));
    }
    raw.top_scalars.insert(key.to_owned(), value);
    Ok(())
}

fn insert_section_scalar(
    raw: &mut RawConfig,
    line: usize,
    section: &str,
    key: &str,
    value: RawValue,
) -> ConfigResult<()> {
    let Some(fields) = raw.sections.get_mut(section) else {
        return Err(parse_error(line, "unknown parser section"));
    };
    if fields.insert(key.to_owned(), value).is_some() {
        return Err(duplicate_line(line, &format!("{section}.{key}")));
    }
    Ok(())
}

fn parse_raw_value(line: usize, value: &str) -> ConfigResult<RawValue> {
    match value {
        "true" => Ok(RawValue::Bool(true)),
        "false" => Ok(RawValue::Bool(false)),
        _ if value.starts_with('[') => parse_flow_list(line, value).map(RawValue::List),
        _ if value == "{}" || value.starts_with('{') => Err(parse_error(
            line,
            "object values are not supported in arb-config v1",
        )),
        _ => parse_string_scalar(line, value).map(RawValue::String),
    }
}

fn parse_flow_list(line: usize, value: &str) -> ConfigResult<Vec<String>> {
    if !value.ends_with(']') {
        return Err(parse_error(line, "list must end with `]`"));
    }

    let inner = value[1..value.len() - 1].trim();
    if inner.is_empty() {
        return Ok(Vec::new());
    }

    let mut output = Vec::new();
    for item in inner.split(',') {
        let item = item.trim();
        if item.is_empty() {
            return Err(parse_error(line, "list item cannot be empty"));
        }
        output.push(parse_string_scalar(line, item)?);
    }
    Ok(output)
}

fn parse_string_scalar(line: usize, value: &str) -> ConfigResult<String> {
    let Some(first) = value.chars().next() else {
        return Err(parse_error(line, "empty scalar"));
    };

    if first == '"' || first == '\'' {
        if !value.ends_with(first) || value.len() < 2 {
            return Err(parse_error(line, "unterminated quoted string"));
        }
        let inner = &value[1..value.len() - 1];
        if inner.contains(first) {
            return Err(parse_error(line, "quote escaping is not supported"));
        }
        Ok(inner.to_owned())
    } else {
        Ok(value.to_owned())
    }
}

fn strip_yaml_comment(line: &str) -> &str {
    let mut in_single = false;
    let mut in_double = false;
    for (index, ch) in line.char_indices() {
        match ch {
            '\'' if !in_double => in_single = !in_single,
            '"' if !in_single => in_double = !in_double,
            '#' if !in_single && !in_double => return &line[..index],
            _ => {}
        }
    }
    line
}

fn reject_unknown_top_fields(raw: &RawConfig) -> ConfigResult<()> {
    for key in raw.top_scalars.keys() {
        reject_sensitive_path(&format!("$.{key}"))?;
        if key != "config_version" {
            return Err(ConfigError::UnknownField {
                path: format!("$.{key}"),
            });
        }
    }

    for key in raw.sections.keys() {
        reject_sensitive_path(&format!("$.{key}"))?;
        if !["execution", "kill_switch", "signing"].contains(&key.as_str()) {
            return Err(ConfigError::UnknownField {
                path: format!("$.{key}"),
            });
        }
    }

    Ok(())
}

fn reject_unknown_section_fields(
    path: &str,
    raw: &RawSection,
    allowed: &[&str],
) -> ConfigResult<()> {
    for key in raw.keys() {
        let field_path = format!("{path}.{key}");
        reject_sensitive_path(&field_path)?;
        if !allowed.contains(&key.as_str()) {
            return Err(ConfigError::UnknownField { path: field_path });
        }
    }
    Ok(())
}

fn required_top_string(raw: &RawConfig, key: &str) -> ConfigResult<String> {
    let path = format!("$.{key}");
    match raw.top_scalars.get(key) {
        Some(RawValue::String(value)) => Ok(value.clone()),
        Some(_) => Err(invalid_value(&path, "expected string")),
        None => Err(ConfigError::MissingField { path }),
    }
}

fn required_section<'a>(raw: &'a RawConfig, section: &str) -> ConfigResult<&'a RawSection> {
    raw.sections
        .get(section)
        .ok_or_else(|| ConfigError::MissingField {
            path: format!("$.{section}"),
        })
}

fn required_section_string(
    raw: &RawSection,
    section_path: &str,
    key: &str,
) -> ConfigResult<String> {
    let path = format!("{section_path}.{key}");
    match raw.get(key) {
        Some(RawValue::String(value)) => Ok(value.clone()),
        Some(_) => Err(invalid_value(&path, "expected string")),
        None => Err(ConfigError::MissingField { path }),
    }
}

fn required_section_bool(raw: &RawSection, section_path: &str, key: &str) -> ConfigResult<bool> {
    let path = format!("{section_path}.{key}");
    match raw.get(key) {
        Some(RawValue::Bool(value)) => Ok(*value),
        Some(_) => Err(invalid_value(&path, "expected bool")),
        None => Err(ConfigError::MissingField { path }),
    }
}

fn required_section_list(
    raw: &RawSection,
    section_path: &str,
    key: &str,
) -> ConfigResult<Vec<String>> {
    let path = format!("{section_path}.{key}");
    match raw.get(key) {
        Some(RawValue::List(value)) => Ok(value.clone()),
        Some(_) => Err(invalid_value(&path, "expected list")),
        None => Err(ConfigError::MissingField { path }),
    }
}

fn required_reference_list(
    raw: &RawSection,
    section_path: &str,
    key: &str,
) -> ConfigResult<Vec<String>> {
    let path = format!("{section_path}.{key}");
    let values = required_section_list(raw, section_path, key)?;
    for value in &values {
        validate_reference(&path, value)?;
    }
    reject_duplicate_list(&path, &values)?;
    Ok(values)
}

fn validate_reference(path: &str, value: &str) -> ConfigResult<()> {
    if value.is_empty() {
        return Err(invalid_value(path, "reference cannot be empty"));
    }
    if value.len() > 160 {
        return Err(invalid_value(path, "reference is too long"));
    }
    if value.bytes().any(|byte| {
        !(byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.' | b':' | b'/' | b'@'))
    }) {
        return Err(invalid_value(
            path,
            "reference must be ASCII letters, digits, `_`, `-`, `.`, `:`, `/` or `@`",
        ));
    }
    Ok(())
}

fn reject_duplicate_list<T>(path: &str, values: &[T]) -> ConfigResult<()>
where
    T: Ord,
{
    let mut seen = BTreeSet::new();
    for value in values {
        if !seen.insert(value) {
            return Err(ConfigError::DuplicateField {
                path: path.to_owned(),
            });
        }
    }
    Ok(())
}

fn reject_sensitive_key(line: usize, key: &str) -> ConfigResult<()> {
    if is_sensitive_name(key) {
        return Err(ConfigError::SensitiveField {
            path: format!("line {line}: {key}"),
        });
    }
    Ok(())
}

fn reject_sensitive_path(path: &str) -> ConfigResult<()> {
    if path.split('.').any(is_sensitive_name) {
        return Err(ConfigError::SensitiveField {
            path: path.to_owned(),
        });
    }
    Ok(())
}

fn is_sensitive_name(name: &str) -> bool {
    let normalized = name.to_ascii_lowercase();
    [
        "api_key",
        "secret",
        "private_key",
        "mnemonic",
        "password",
        "token",
        "credential",
        "seed_phrase",
    ]
    .iter()
    .any(|needle| normalized.contains(needle))
}

fn canonical_list(values: &[String]) -> String {
    format!(
        "[{}]",
        values
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>()
            .join(",")
    )
}

fn canonical_modes(values: &[ExecutionMode]) -> String {
    format!(
        "[{}]",
        values
            .iter()
            .map(|mode| mode.as_str())
            .collect::<Vec<_>>()
            .join(",")
    )
}

fn invalid_value(path: impl Into<String>, message: impl Into<String>) -> ConfigError {
    ConfigError::InvalidValue {
        path: path.into(),
        message: message.into(),
    }
}

fn parse_error(line: usize, message: impl Into<String>) -> ConfigError {
    ConfigError::Parse {
        line,
        message: message.into(),
    }
}

fn duplicate_line(line: usize, key: &str) -> ConfigError {
    ConfigError::Parse {
        line,
        message: format!("duplicate field `{key}`"),
    }
}

fn sha256_hex(input: &[u8]) -> String {
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

    let mut state = H0;
    let bit_len = (input.len() as u64) * 8;
    let mut padded = Vec::with_capacity(input.len() + 72);
    padded.extend_from_slice(input);
    padded.push(0x80);
    while padded.len() % 64 != 56 {
        padded.push(0);
    }
    padded.extend_from_slice(&bit_len.to_be_bytes());

    for chunk in padded.chunks_exact(64) {
        let mut words = [0_u32; 64];
        for (index, word) in words.iter_mut().take(16).enumerate() {
            let offset = index * 4;
            *word = u32::from_be_bytes([
                chunk[offset],
                chunk[offset + 1],
                chunk[offset + 2],
                chunk[offset + 3],
            ]);
        }
        for index in 16..64 {
            let s0 = words[index - 15].rotate_right(7)
                ^ words[index - 15].rotate_right(18)
                ^ (words[index - 15] >> 3);
            let s1 = words[index - 2].rotate_right(17)
                ^ words[index - 2].rotate_right(19)
                ^ (words[index - 2] >> 10);
            words[index] = words[index - 16]
                .wrapping_add(s0)
                .wrapping_add(words[index - 7])
                .wrapping_add(s1);
        }

        let mut a = state[0];
        let mut b = state[1];
        let mut c = state[2];
        let mut d = state[3];
        let mut e = state[4];
        let mut f = state[5];
        let mut g = state[6];
        let mut h = state[7];

        for index in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ ((!e) & g);
            let temp1 = h
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(K[index])
                .wrapping_add(words[index]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let temp2 = s0.wrapping_add(maj);

            h = g;
            g = f;
            f = e;
            e = d.wrapping_add(temp1);
            d = c;
            c = b;
            b = a;
            a = temp1.wrapping_add(temp2);
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

    let mut output = String::with_capacity(64);
    for word in state {
        push_hex_u32(word, &mut output);
    }
    output
}

fn push_hex_u32(word: u32, output: &mut String) {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    for byte in word.to_be_bytes() {
        output.push(HEX[(byte >> 4) as usize] as char);
        output.push(HEX[(byte & 0x0f) as usize] as char);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEMPLATE: &str = include_str!("../../../templates/config.template.yaml");

    fn base_config() -> &'static str {
        r#"
config_version: "arb-config-v1"
execution:
  mode: "ReadOnly"
  live_execution_enabled: false
  auto_live_enabled: false
kill_switch:
  global: false
  execution: false
  strategies: []
  venues: []
  accounts: []
  instruments: []
  assets: []
  chains: []
  execution_modes: []
signing:
  policy_ref: "signing-policy/null-signer-v1"
  real_signing_enabled: false
"#
    }

    #[test]
    fn template_defaults_keep_live_closed() {
        let config = ArbConfig::from_yaml_str(TEMPLATE).expect("template config should load");

        assert_eq!(config.version().as_str(), SUPPORTED_CONFIG_VERSION);
        assert_eq!(config.execution().mode(), ExecutionMode::ReadOnly);
        assert!(config.execution().live_execution_default_closed());
        assert!(!config.allows_account_changes());
        assert!(!config.signing().real_signing_enabled());
        assert_eq!(
            config.signing().policy_ref().as_str(),
            "signing-policy/null-signer-v1"
        );
        assert!(config.hash().as_str().starts_with("sha256:"));
    }

    #[test]
    fn illegal_config_is_rejected() {
        let input = base_config().replace(
            "live_execution_enabled: false",
            "live_execution_enabled: true",
        );

        let error = ArbConfig::from_yaml_str(&input).expect_err("live flag in ReadOnly must fail");
        assert!(matches!(error, ConfigError::InvalidValue { .. }));
    }

    #[test]
    fn unknown_and_sensitive_fields_are_rejected() {
        let unknown = base_config().replace(
            "  real_signing_enabled: false",
            "  real_signing_enabled: false\n  api_secret: \"do-not-store\"",
        );
        let error = ArbConfig::from_yaml_str(&unknown).expect_err("secret field must fail");
        assert!(matches!(error, ConfigError::SensitiveField { .. }));
    }

    #[test]
    fn kill_switch_blocks_live_account_changes() {
        let input = r#"
config_version: "arb-config-v1"
execution:
  mode: "GuardedLive"
  live_execution_enabled: true
  auto_live_enabled: false
kill_switch:
  global: true
  execution: false
  strategies: ["strategy/main"]
  venues: ["venue/binance"]
  accounts: ["account/main"]
  instruments: ["instrument/main"]
  assets: ["asset/main"]
  chains: ["chain/main"]
  execution_modes: ["GuardedLive"]
signing:
  policy_ref: "signing-policy/null-signer-v1"
  real_signing_enabled: false
"#;

        let config = ArbConfig::from_yaml_str(input).expect("kill switch config should load");
        assert!(config.kill_switch().is_triggered());
        assert!(config
            .kill_switch()
            .blocks_execution_mode(ExecutionMode::GuardedLive));
        assert!(config.kill_switch().blocks_strategy("strategy/main"));
        assert!(config.kill_switch().blocks_venue("venue/binance"));
        assert!(config.kill_switch().blocks_account("account/main"));
        assert!(config.kill_switch().blocks_instrument("instrument/main"));
        assert!(config.kill_switch().blocks_asset("asset/main"));
        assert!(config.kill_switch().blocks_chain("chain/main"));
        assert!(!config.allows_account_changes());
    }

    #[test]
    fn config_hash_is_stable_across_field_order_and_comments() {
        let reordered = r#"
# same effective config, different order
signing:
  real_signing_enabled: false
  policy_ref: "signing-policy/null-signer-v1"
kill_switch:
  execution_modes: []
  chains: []
  assets: []
  instruments: []
  accounts: []
  venues: []
  strategies: []
  execution: false
  global: false
execution:
  auto_live_enabled: false
  live_execution_enabled: false
  mode: "ReadOnly"
config_version: "arb-config-v1"
"#;

        let left = ArbConfig::from_yaml_str(base_config()).expect("base config should load");
        let right = ArbConfig::from_yaml_str(reordered).expect("reordered config should load");

        assert_eq!(left.hash(), right.hash());
    }

    #[test]
    fn config_hash_changes_when_permission_changes() {
        let left = ArbConfig::from_yaml_str(base_config()).expect("base config should load");
        let changed = base_config().replace("mode: \"ReadOnly\"", "mode: \"Simulated\"");
        let right = ArbConfig::from_yaml_str(&changed).expect("simulated config should load");

        assert_ne!(left.hash(), right.hash());
        assert!(!right.allows_account_changes());
    }

    #[test]
    fn sha256_implementation_matches_known_vector() {
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }
}
