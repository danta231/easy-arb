//! `arb-replay` 确定性回放输入加载。
//!
//! 中文说明：本 crate 只从离线 fixture 读取事件、配置、固定时间源和固定随机
//! 种子。它不访问外部 API、不触发签名、不写真实账户，也不把系统当前时间作为
//! 回放输入。

#![forbid(unsafe_code)]

use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use arb_config::ArbConfig;
use arb_contracts::{from_json_strict, NormalizedEvent};
use arb_eventstore::canonical_event_hash;

/// 当前 replay fixture 元数据版本。
pub const SUPPORTED_REPLAY_VERSION: &str = "arb-replay-fixture-v1";

/// 回放模块统一返回类型。
pub type ReplayResult<T> = Result<T, ReplayError>;

/// 回放输入加载错误。
///
/// 中文说明：fixture 损坏、配置非法、排序不确定或哈希不匹配时必须显式失败，
/// 不能用默认值或当前系统状态继续回放。
#[derive(Debug)]
pub enum ReplayError {
    Io {
        path: PathBuf,
        message: String,
    },
    Config {
        path: PathBuf,
        message: String,
    },
    Contract {
        path: PathBuf,
        line: usize,
        message: String,
    },
    Metadata {
        path: PathBuf,
        line: Option<usize>,
        message: String,
    },
    InvalidFixture {
        path: PathBuf,
        line: Option<usize>,
        message: String,
    },
}

impl fmt::Display for ReplayError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { path, message } => write!(f, "{}: {message}", path.display()),
            Self::Config { path, message } => {
                write!(f, "{}: invalid config fixture: {message}", path.display())
            }
            Self::Contract {
                path,
                line,
                message,
            } => write!(
                f,
                "{} line {line}: invalid event fixture: {message}",
                path.display()
            ),
            Self::Metadata {
                path,
                line: Some(line),
                message,
            } => write!(
                f,
                "{} line {line}: invalid replay metadata: {message}",
                path.display()
            ),
            Self::Metadata {
                path,
                line: None,
                message,
            } => write!(f, "{}: invalid replay metadata: {message}", path.display()),
            Self::InvalidFixture {
                path,
                line: Some(line),
                message,
            } => write!(
                f,
                "{} line {line}: invalid replay fixture: {message}",
                path.display()
            ),
            Self::InvalidFixture {
                path,
                line: None,
                message,
            } => write!(f, "{}: invalid replay fixture: {message}", path.display()),
        }
    }
}

impl Error for ReplayError {}

/// 已加载的回放输入。
///
/// 中文说明：所有字段都来自 fixture 目录；调用方通过 getter 读取，避免在回放
/// 过程中悄悄补充系统时间、环境变量或外部状态。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReplayInput {
    fixture_root: PathBuf,
    events: Vec<NormalizedEvent>,
    config: ArbConfig,
    metadata: ReplayFixtureMetadata,
}

impl ReplayInput {
    /// 返回 fixture 根目录。
    pub fn fixture_root(&self) -> &Path {
        &self.fixture_root
    }

    /// 返回按回放规则排序后的事件。
    pub fn events(&self) -> &[NormalizedEvent] {
        &self.events
    }

    /// 返回只读配置 fixture。
    pub fn config(&self) -> &ArbConfig {
        &self.config
    }

    /// 返回固定时间源。
    pub fn time_source(&self) -> FixedTimeSource {
        FixedTimeSource::new_unchecked(self.metadata.fixed_time.clone())
    }

    /// 返回固定随机种子。
    pub fn random_seed(&self) -> ReplayRandomSeed {
        ReplayRandomSeed(self.metadata.random_seed)
    }

    /// 用固定种子创建一个确定性随机数发生器。
    pub fn seeded_rng(&self) -> SeededRandom {
        self.random_seed().into_rng()
    }

    /// 生成一个最小 smoke 回放摘要。
    ///
    /// 中文说明：该摘要只使用 fixture 中的事件、配置哈希、固定时间和固定种子，
    /// 可用于证明同一输入多次运行结果一致。
    pub fn run_smoke_replay(&self) -> ReplaySmokeResult {
        let mut rng = self.seeded_rng();
        ReplaySmokeResult {
            fixture_name: self
                .fixture_root
                .file_name()
                .and_then(|value| value.to_str())
                .unwrap_or("")
                .to_owned(),
            fixed_time: self.time_source().now().to_owned(),
            random_seed: self.random_seed().as_u64(),
            config_hash: self.config.hash().as_str().to_owned(),
            event_count: self.events.len(),
            ordered_event_ids: self
                .events
                .iter()
                .map(|event| event.event_id.as_str().to_owned())
                .collect(),
            event_hashes: self
                .events
                .iter()
                .map(|event| event.checksum.as_str().to_owned())
                .collect(),
            deterministic_random_draws: vec![rng.next_u64(), rng.next_u64(), rng.next_u64()],
        }
    }
}

/// replay fixture 元数据。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReplayFixtureMetadata {
    replay_version: String,
    fixed_time: String,
    random_seed: u64,
}

impl ReplayFixtureMetadata {
    pub fn replay_version(&self) -> &str {
        &self.replay_version
    }

    pub fn fixed_time(&self) -> &str {
        &self.fixed_time
    }

    pub fn random_seed(&self) -> u64 {
        self.random_seed
    }
}

/// 固定时间源接口。
pub trait TimeSource {
    fn now(&self) -> &str;
}

/// 从 fixture 注入的固定 UTC 时间。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FixedTimeSource {
    now_utc: String,
}

impl FixedTimeSource {
    /// 创建固定时间源并校验 UTC 秒级格式。
    pub fn new(now_utc: impl Into<String>) -> ReplayResult<Self> {
        let now_utc = now_utc.into();
        validate_utc_second(&now_utc).map_err(|message| ReplayError::Metadata {
            path: PathBuf::from("<fixed-time>"),
            line: None,
            message,
        })?;
        Ok(Self { now_utc })
    }

    fn new_unchecked(now_utc: String) -> Self {
        Self { now_utc }
    }
}

impl TimeSource for FixedTimeSource {
    fn now(&self) -> &str {
        &self.now_utc
    }
}

/// fixture 记录的固定随机种子。
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ReplayRandomSeed(u64);

impl ReplayRandomSeed {
    pub fn as_u64(self) -> u64 {
        self.0
    }

    pub fn into_rng(self) -> SeededRandom {
        SeededRandom::new(self.0)
    }
}

/// 确定性伪随机数发生器。
///
/// 中文说明：用于回放中需要随机性的路径；同一 seed 必须得到完全一致的序列。
/// 这里不依赖操作系统随机源。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SeededRandom {
    state: u64,
}

impl SeededRandom {
    pub fn new(seed: u64) -> Self {
        Self { state: seed }
    }

    pub fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9e3779b97f4a7c15);
        let mut value = self.state;
        value = (value ^ (value >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
        value = (value ^ (value >> 27)).wrapping_mul(0x94d049bb133111eb);
        value ^ (value >> 31)
    }
}

/// 最小 smoke 回放输出。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReplaySmokeResult {
    pub fixture_name: String,
    pub fixed_time: String,
    pub random_seed: u64,
    pub config_hash: String,
    pub event_count: usize,
    pub ordered_event_ids: Vec<String>,
    pub event_hashes: Vec<String>,
    pub deterministic_random_draws: Vec<u64>,
}

impl ReplaySmokeResult {
    /// 输出稳定文本，供黄金 fixture 或人工比对使用。
    pub fn to_stable_text(&self) -> String {
        format!(
            concat!(
                "fixture_name={}\n",
                "fixed_time={}\n",
                "random_seed={}\n",
                "config_hash={}\n",
                "event_count={}\n",
                "ordered_event_ids={}\n",
                "event_hashes={}\n",
                "deterministic_random_draws={}\n",
            ),
            self.fixture_name,
            self.fixed_time,
            self.random_seed,
            self.config_hash,
            self.event_count,
            self.ordered_event_ids.join(","),
            self.event_hashes.join(","),
            self.deterministic_random_draws
                .iter()
                .map(u64::to_string)
                .collect::<Vec<_>>()
                .join(","),
        )
    }
}

/// 加载一个完整 replay fixture 目录。
///
/// 中文说明：目录内必须包含 `events.jsonl`、`config.yaml` 和 `replay.yaml`。
pub fn load_fixture(root: impl AsRef<Path>) -> ReplayResult<ReplayInput> {
    let root = root.as_ref();
    let events_path = root.join("events.jsonl");
    let config_path = root.join("config.yaml");
    let metadata_path = root.join("replay.yaml");

    Ok(ReplayInput {
        fixture_root: root.to_path_buf(),
        events: load_events_fixture(&events_path)?,
        config: load_config_fixture(&config_path)?,
        metadata: load_replay_metadata(&metadata_path)?,
    })
}

/// 加载事件 JSONL fixture。
pub fn load_events_fixture(path: impl AsRef<Path>) -> ReplayResult<Vec<NormalizedEvent>> {
    let path = path.as_ref();
    let input = read_to_string(path)?;
    let mut events = Vec::new();

    for (index, line) in input.lines().enumerate() {
        let line_number = index + 1;
        if line.trim().is_empty() {
            return Err(ReplayError::InvalidFixture {
                path: path.to_path_buf(),
                line: Some(line_number),
                message: "blank JSONL line is not allowed".to_owned(),
            });
        }

        let event =
            from_json_strict::<NormalizedEvent>(line).map_err(|error| ReplayError::Contract {
                path: path.to_path_buf(),
                line: line_number,
                message: error.to_string(),
            })?;
        validate_event_hash(path, line_number, &event)?;
        events.push(event);
    }

    if events.is_empty() {
        return Err(ReplayError::InvalidFixture {
            path: path.to_path_buf(),
            line: None,
            message: "events.jsonl must contain at least one event".to_owned(),
        });
    }

    order_events(path, events)
}

/// 加载配置 fixture。
pub fn load_config_fixture(path: impl AsRef<Path>) -> ReplayResult<ArbConfig> {
    let path = path.as_ref();
    ArbConfig::from_path(path).map_err(|error| ReplayError::Config {
        path: path.to_path_buf(),
        message: error.to_string(),
    })
}

/// 加载 replay 元数据 fixture。
pub fn load_replay_metadata(path: impl AsRef<Path>) -> ReplayResult<ReplayFixtureMetadata> {
    let path = path.as_ref();
    let fields = parse_metadata_yaml(path, &read_to_string(path)?)?;

    let replay_version = required_metadata_string(path, &fields, "replay_version")?;
    if replay_version != SUPPORTED_REPLAY_VERSION {
        return Err(metadata_error(
            path,
            None,
            format!("expected replay_version `{SUPPORTED_REPLAY_VERSION}`"),
        ));
    }

    let fixed_time = required_metadata_string(path, &fields, "fixed_time")?;
    validate_utc_second(&fixed_time).map_err(|message| metadata_error(path, None, message))?;

    let random_seed = required_metadata_string(path, &fields, "random_seed")?
        .parse::<u64>()
        .map_err(|_| metadata_error(path, None, "random_seed must be an unsigned integer"))?;

    Ok(ReplayFixtureMetadata {
        replay_version,
        fixed_time,
        random_seed,
    })
}

fn read_to_string(path: &Path) -> ReplayResult<String> {
    fs::read_to_string(path).map_err(|error| ReplayError::Io {
        path: path.to_path_buf(),
        message: error.to_string(),
    })
}

fn validate_event_hash(path: &Path, line: usize, event: &NormalizedEvent) -> ReplayResult<()> {
    let expected = canonical_event_hash(event);
    let actual = event.checksum.as_str();
    if actual != expected {
        return Err(ReplayError::InvalidFixture {
            path: path.to_path_buf(),
            line: Some(line),
            message: format!("checksum mismatch; expected {expected}, found {actual}"),
        });
    }
    Ok(())
}

fn order_events(
    path: &Path,
    mut events: Vec<NormalizedEvent>,
) -> ReplayResult<Vec<NormalizedEvent>> {
    let sequenced_count = events
        .iter()
        .filter(|event| event.sequence.is_some())
        .count();

    if sequenced_count == events.len() {
        events.sort_by_key(|event| event.sequence.expect("checked above"));
        reject_duplicate_sequences(path, &events)?;
        return Ok(events);
    }

    if sequenced_count > 0 {
        return Err(ReplayError::InvalidFixture {
            path: path.to_path_buf(),
            line: None,
            message: "events must either all contain sequence or all omit sequence".to_owned(),
        });
    }

    for event in &events {
        if event.source_sequence.as_deref().unwrap_or("").is_empty() {
            return Err(ReplayError::InvalidFixture {
                path: path.to_path_buf(),
                line: None,
                message: format!(
                    "event `{}` is missing source_sequence for import ordering",
                    event.event_id.as_str()
                ),
            });
        }
    }

    events.sort_by_key(import_order_key);
    reject_ambiguous_import_order(path, &events)?;
    Ok(events)
}

fn reject_duplicate_sequences(path: &Path, events: &[NormalizedEvent]) -> ReplayResult<()> {
    let mut seen = BTreeSet::new();
    for event in events {
        let sequence = event.sequence.expect("only called for sequenced events");
        if !seen.insert(sequence) {
            return Err(ReplayError::InvalidFixture {
                path: path.to_path_buf(),
                line: None,
                message: format!("duplicate event sequence {sequence}"),
            });
        }
    }
    Ok(())
}

fn import_order_key(event: &NormalizedEvent) -> (String, String, String, String) {
    (
        event.source.clone(),
        event.source_sequence.clone().unwrap_or_default(),
        event.timestamp_event.as_str().to_owned(),
        event.event_id.as_str().to_owned(),
    )
}

fn reject_ambiguous_import_order(path: &Path, events: &[NormalizedEvent]) -> ReplayResult<()> {
    let mut seen = BTreeSet::new();
    for event in events {
        let key = (
            event.source.as_str(),
            event.source_sequence.as_deref().unwrap_or(""),
            event.timestamp_event.as_str(),
        );
        if !seen.insert(key) {
            return Err(ReplayError::InvalidFixture {
                path: path.to_path_buf(),
                line: None,
                message: format!(
                    "events share source/source_sequence/timestamp ordering key near `{}`",
                    event.event_id.as_str()
                ),
            });
        }
    }
    Ok(())
}

fn parse_metadata_yaml(path: &Path, input: &str) -> ReplayResult<BTreeMap<String, String>> {
    let mut fields = BTreeMap::new();

    for (index, original_line) in input.lines().enumerate() {
        let line_number = index + 1;
        if original_line.contains('\t') {
            return Err(metadata_error(
                path,
                Some(line_number),
                "tabs are not allowed",
            ));
        }

        let without_comment = strip_yaml_comment(original_line);
        let trimmed = without_comment.trim();
        if trimmed.is_empty() {
            continue;
        }
        if original_line.len() - original_line.trim_start_matches(' ').len() != 0 {
            return Err(metadata_error(
                path,
                Some(line_number),
                "replay metadata only supports top-level scalar fields",
            ));
        }

        let Some((raw_key, raw_value)) = trimmed.split_once(':') else {
            return Err(metadata_error(
                path,
                Some(line_number),
                "expected `key: value`",
            ));
        };
        let key = raw_key.trim();
        if !matches!(key, "replay_version" | "fixed_time" | "random_seed") {
            return Err(metadata_error(
                path,
                Some(line_number),
                format!("unknown metadata field `{key}`"),
            ));
        }
        reject_sensitive_key(path, Some(line_number), key)?;

        let value = parse_metadata_scalar(path, Some(line_number), raw_value.trim())?;
        if fields.insert(key.to_owned(), value).is_some() {
            return Err(metadata_error(
                path,
                Some(line_number),
                format!("duplicate metadata field `{key}`"),
            ));
        }
    }

    Ok(fields)
}

fn required_metadata_string(
    path: &Path,
    fields: &BTreeMap<String, String>,
    key: &str,
) -> ReplayResult<String> {
    fields
        .get(key)
        .cloned()
        .ok_or_else(|| metadata_error(path, None, format!("missing metadata field `{key}`")))
}

fn parse_metadata_scalar(path: &Path, line: Option<usize>, value: &str) -> ReplayResult<String> {
    if value.is_empty() {
        return Err(metadata_error(path, line, "metadata value cannot be empty"));
    }
    let first = value
        .chars()
        .next()
        .expect("empty checked before reading first char");
    if first == '"' || first == '\'' {
        if !value.ends_with(first) || value.len() < 2 {
            return Err(metadata_error(path, line, "unterminated quoted string"));
        }
        let inner = &value[1..value.len() - 1];
        if inner.contains(first) {
            return Err(metadata_error(
                path,
                line,
                "quote escaping is not supported",
            ));
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

fn reject_sensitive_key(path: &Path, line: Option<usize>, key: &str) -> ReplayResult<()> {
    let normalized = key.to_ascii_lowercase();
    let sensitive = [
        "secret",
        "private",
        "token",
        "credential",
        "password",
        "mnemonic",
        "api_key",
    ];
    if sensitive.iter().any(|needle| normalized.contains(needle)) {
        return Err(metadata_error(
            path,
            line,
            format!("sensitive metadata field `{key}` is not allowed"),
        ));
    }
    Ok(())
}

fn validate_utc_second(value: &str) -> Result<(), String> {
    let bytes = value.as_bytes();
    if bytes.len() != 20
        || bytes[4] != b'-'
        || bytes[7] != b'-'
        || bytes[10] != b'T'
        || bytes[13] != b':'
        || bytes[16] != b':'
        || bytes[19] != b'Z'
    {
        return Err("fixed_time must use UTC second format YYYY-MM-DDTHH:MM:SSZ".to_owned());
    }

    for index in [0, 1, 2, 3, 5, 6, 8, 9, 11, 12, 14, 15, 17, 18] {
        if !bytes[index].is_ascii_digit() {
            return Err("fixed_time contains a non-digit timestamp component".to_owned());
        }
    }

    let year = parse_two_or_four_digits(&bytes[0..4]);
    let month = parse_two_or_four_digits(&bytes[5..7]);
    let day = parse_two_or_four_digits(&bytes[8..10]);
    let hour = parse_two_or_four_digits(&bytes[11..13]);
    let minute = parse_two_or_four_digits(&bytes[14..16]);
    let second = parse_two_or_four_digits(&bytes[17..19]);

    if year == 0 {
        return Err("fixed_time year must be greater than zero".to_owned());
    }
    if !(1..=12).contains(&month) {
        return Err("fixed_time month must be 01..=12".to_owned());
    }
    let max_day = days_in_month(year, month);
    if day == 0 || day > max_day {
        return Err(format!("fixed_time day must be 01..={max_day:02}"));
    }
    if hour > 23 || minute > 59 || second > 59 {
        return Err("fixed_time time must be within 00:00:00..23:59:59".to_owned());
    }
    Ok(())
}

fn parse_two_or_four_digits(bytes: &[u8]) -> u32 {
    bytes
        .iter()
        .fold(0_u32, |acc, byte| acc * 10 + u32::from(byte - b'0'))
}

fn days_in_month(year: u32, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if is_leap_year(year) => 29,
        2 => 28,
        _ => 0,
    }
}

fn is_leap_year(year: u32) -> bool {
    (year % 4 == 0 && year % 100 != 0) || year % 400 == 0
}

fn metadata_error(path: &Path, line: Option<usize>, message: impl Into<String>) -> ReplayError {
    ReplayError::Metadata {
        path: path.to_path_buf(),
        line,
        message: message.into(),
    }
}

#[cfg(test)]
mod tests {
    use std::thread;
    use std::time::Duration;

    use super::*;
    use arb_contracts::{to_canonical_json, CandidatePortfolioTransition};

    #[test]
    fn loads_events_config_fixed_time_and_seed_from_fixture() {
        let input = load_fixture(minimal_fixture_dir()).expect("minimal fixture should load");

        assert_eq!(input.events().len(), 2);
        assert_eq!(
            input
                .events()
                .iter()
                .map(|event| event.event_id.as_str())
                .collect::<Vec<_>>(),
            vec!["event:smoke:01", "event:smoke:02"]
        );
        assert_eq!(input.time_source().now(), "2026-01-01T00:00:05Z");
        assert_eq!(input.random_seed().as_u64(), 424242);
        assert!(input.config().execution().live_execution_default_closed());
        assert!(!input.config().allows_account_changes());
        assert!(input
            .events()
            .iter()
            .all(|event| canonical_event_hash(event) == event.checksum.as_str()));
    }

    #[test]
    fn repeated_fixture_runs_are_identical() {
        let left = load_fixture(minimal_fixture_dir())
            .expect("first load should succeed")
            .run_smoke_replay();
        let right = load_fixture(minimal_fixture_dir())
            .expect("second load should succeed")
            .run_smoke_replay();

        assert_eq!(left, right);
        assert_eq!(left.to_stable_text(), right.to_stable_text());
    }

    #[test]
    fn strategy_smoke_fixture_loads_with_expected_candidate_output() {
        let input = load_fixture(strategy_fixture_dir()).expect("strategy fixture should load");

        assert_eq!(input.events().len(), 1);
        assert_eq!(
            input.events()[0].event_id.as_str(),
            "event:strategy-smoke-01"
        );
        assert_eq!(input.time_source().now(), "2026-01-01T00:00:02Z");
        assert_eq!(input.random_seed().as_u64(), 4004);
        assert_eq!(input.config().execution().mode().as_str(), "ReadOnly");
        assert!(!input.config().allows_account_changes());

        let expected_path = strategy_fixture_dir().join("expected/candidate_transitions.jsonl");
        let expected = fs::read_to_string(&expected_path).expect("expected candidate JSONL");
        let expected_line = expected
            .lines()
            .next()
            .expect("candidate transition fixture line");
        let candidate = from_json_strict::<CandidatePortfolioTransition>(expected_line)
            .expect("expected candidate should satisfy contract");
        assert_eq!(to_canonical_json(&candidate), expected_line);
        assert!(strategy_fixture_dir()
            .join("strategy_manifest.yaml")
            .is_file());
    }

    #[test]
    fn manual_approval_fixtures_are_replay_loadable_and_ordered() {
        let approved =
            load_fixture(manual_approval_approved_fixture_dir()).expect("approved fixture");
        assert_eq!(approved.events().len(), 1);
        assert_eq!(
            approved.events()[0].event_id.as_str(),
            "event:approval:approved:01"
        );
        assert_eq!(approved.events()[0].event_type.as_str(), "ApprovalEvent");
        assert_eq!(approved.time_source().now(), "2026-01-01T00:01:30Z");
        assert_eq!(approved.random_seed().as_u64(), 10001);
        assert_eq!(
            approved.config().execution().mode().as_str(),
            "ManualApproval"
        );
        assert!(!approved.config().allows_account_changes());

        let rejected =
            load_fixture(manual_approval_rejected_fixture_dir()).expect("rejected fixture");
        assert_eq!(
            rejected
                .events()
                .iter()
                .map(|event| event.event_id.as_str())
                .collect::<Vec<_>>(),
            vec![
                "event:approval:rejected:01",
                "event:approval:expired:01",
                "event:approval:duplicate:01"
            ]
        );
        assert_eq!(rejected.time_source().now(), "2026-01-01T00:06:30Z");
        assert_eq!(rejected.random_seed().as_u64(), 10002);
        assert_eq!(
            rejected.config().execution().mode().as_str(),
            "ManualApproval"
        );
        assert!(rejected
            .events()
            .iter()
            .all(|event| canonical_event_hash(event) == event.checksum.as_str()));
    }

    #[test]
    fn replay_result_does_not_depend_on_system_current_time() {
        let input = load_fixture(minimal_fixture_dir()).expect("fixture should load");
        let before = input.run_smoke_replay();

        thread::sleep(Duration::from_millis(2));

        let after = input.run_smoke_replay();
        assert_eq!(before, after);
        assert_eq!(after.fixed_time, "2026-01-01T00:00:05Z");
    }

    #[test]
    fn seeded_random_is_reproducible() {
        let mut left = ReplayRandomSeed(7).into_rng();
        let mut right = ReplayRandomSeed(7).into_rng();
        let mut different = ReplayRandomSeed(8).into_rng();

        let left_values = [left.next_u64(), left.next_u64(), left.next_u64()];
        let right_values = [right.next_u64(), right.next_u64(), right.next_u64()];
        let different_values = [
            different.next_u64(),
            different.next_u64(),
            different.next_u64(),
        ];

        assert_eq!(left_values, right_values);
        assert_ne!(left_values, different_values);
    }

    #[test]
    fn fixed_time_rejects_non_utc_or_invalid_dates() {
        assert!(FixedTimeSource::new("2026-01-01T00:00:00Z").is_ok());
        assert!(FixedTimeSource::new("2026-01-01T00:00:00+08:00").is_err());
        assert!(FixedTimeSource::new("2026-02-30T00:00:00Z").is_err());
    }

    #[test]
    fn damaged_event_checksum_fails_closed() {
        let path = minimal_fixture_dir().join("events.jsonl");
        let input = fs::read_to_string(&path).expect("fixture should be readable");
        let damaged = input.replacen("sha256:", "sha257:", 1);
        let temp_dir =
            std::env::temp_dir().join(format!("easy-arb-replay-damaged-{}", std::process::id()));
        fs::create_dir_all(&temp_dir).expect("temp dir should be created");
        let damaged_path = temp_dir.join("events.jsonl");
        fs::write(&damaged_path, damaged).expect("test fixture should be writable");

        let error = load_events_fixture(&damaged_path).expect_err("checksum mismatch must fail");
        assert!(matches!(error, ReplayError::InvalidFixture { .. }));
    }

    fn minimal_fixture_dir() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join("fixtures/replay/minimal_smoke")
    }

    fn strategy_fixture_dir() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join("fixtures/replay/strategy_smoke")
    }

    fn manual_approval_approved_fixture_dir() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join("fixtures/replay/manual_approval_approved")
    }

    fn manual_approval_rejected_fixture_dir() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join("fixtures/replay/manual_approval_rejected")
    }
}
