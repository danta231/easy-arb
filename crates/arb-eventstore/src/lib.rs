//! `arb-eventstore` 追加式事件存储。
//!
//! 中文说明：本 crate 只负责事件事实的追加、排序、哈希和只读查询。
//! 它不解释业务含义、不产生风控结论、不写账本，也不提供改写历史事件的接口。

#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::error::Error;
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use arb_contracts::{
    from_json_strict, to_canonical_json, CanonicalJson, JsonValue, NormalizedEvent,
};

/// 事件存储统一返回类型。
pub type EventStoreResult<T> = Result<T, EventStoreError>;

/// 事件存储错误。
///
/// 中文说明：读取时重新校验 JSON、序号和哈希；任何损坏或未知状态都不能被当作成功。
#[derive(Debug)]
pub enum EventStoreError {
    Io {
        path: PathBuf,
        message: String,
    },
    Contract {
        line: Option<usize>,
        message: String,
    },
    InvalidStore {
        line: Option<usize>,
        message: String,
    },
    LockPoisoned,
}

impl fmt::Display for EventStoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { path, message } => write!(f, "{}: {message}", path.display()),
            Self::Contract {
                line: Some(line),
                message,
            } => {
                write!(f, "line {line}: contract error: {message}")
            }
            Self::Contract {
                line: None,
                message,
            } => {
                write!(f, "contract error: {message}")
            }
            Self::InvalidStore {
                line: Some(line),
                message,
            } => {
                write!(f, "line {line}: invalid event store: {message}")
            }
            Self::InvalidStore {
                line: None,
                message,
            } => {
                write!(f, "invalid event store: {message}")
            }
            Self::LockPoisoned => f.write_str("event store lock is poisoned"),
        }
    }
}

impl Error for EventStoreError {}

/// 追加写入结果。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AppendReceipt {
    pub sequence: u64,
    pub event_hash: String,
}

/// 已落盘事件。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StoredEvent {
    pub sequence: u64,
    pub event_hash: String,
    pub event: NormalizedEvent,
    pub canonical_json: String,
}

/// 事件追加接口。
pub trait EventWriter {
    fn append(&self, event: &NormalizedEvent) -> EventStoreResult<AppendReceipt>;
}

/// 事件只读接口。
pub trait EventReader {
    fn read_all_ordered(&self) -> EventStoreResult<Vec<StoredEvent>>;
    fn read_by_sequence(&self, sequence: u64) -> EventStoreResult<Option<StoredEvent>>;
    fn read_range(
        &self,
        from_sequence: u64,
        to_sequence: u64,
    ) -> EventStoreResult<Vec<StoredEvent>>;
    fn read_correlation_chain(&self, correlation_id: &str) -> EventStoreResult<Vec<StoredEvent>>;
}

/// 追加式 JSONL 文件事件存储。
///
/// 中文说明：文件使用 `append` 模式打开，写入路径只追加一行规范 JSON，不提供覆盖、
/// 截断、删除或原地更新能力。
#[derive(Debug)]
pub struct JsonlEventStore {
    path: PathBuf,
    write_lock: Mutex<()>,
}

impl JsonlEventStore {
    /// 创建指向指定 JSONL 文件的事件存储句柄。
    pub fn open(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            write_lock: Mutex::new(()),
        }
    }

    /// 返回底层 JSONL 文件路径。
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// 从原始 JSON 解析事件后追加。
    pub fn append_json(&self, json: &str) -> EventStoreResult<AppendReceipt> {
        let event = parse_event(json, None)?;
        self.append(&event)
    }

    fn append_locked(&self, event: &NormalizedEvent) -> EventStoreResult<AppendReceipt> {
        let existing = self.read_all_ordered()?;
        let next_sequence = match existing.last() {
            Some(record) => {
                record
                    .sequence
                    .checked_add(1)
                    .ok_or_else(|| EventStoreError::InvalidStore {
                        line: None,
                        message: "sequence overflow".to_owned(),
                    })?
            }
            None => 1,
        };

        ensure_parent_dir(&self.path)?;
        ensure_appendable_file(&self.path)?;

        let (canonical_json, event_hash) =
            canonical_event_json_with_assigned_sequence(event, next_sequence)?;

        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .map_err(|error| io_error(&self.path, error))?;
        writeln!(file, "{canonical_json}").map_err(|error| io_error(&self.path, error))?;
        file.flush().map_err(|error| io_error(&self.path, error))?;

        Ok(AppendReceipt {
            sequence: next_sequence,
            event_hash,
        })
    }

    fn load_records(&self) -> EventStoreResult<Vec<StoredEvent>> {
        if !self.path.exists() {
            return Ok(Vec::new());
        }

        let file = File::open(&self.path).map_err(|error| io_error(&self.path, error))?;
        let reader = BufReader::new(file);
        let mut records = Vec::new();
        let mut expected_sequence = 1_u64;

        for (index, line) in reader.lines().enumerate() {
            let line_number = index + 1;
            let line = line.map_err(|error| io_error(&self.path, error))?;
            if line.is_empty() {
                return Err(EventStoreError::InvalidStore {
                    line: Some(line_number),
                    message: "blank JSONL line is not a valid event".to_owned(),
                });
            }

            let event = parse_event(&line, Some(line_number))?;
            let sequence = event
                .sequence
                .ok_or_else(|| EventStoreError::InvalidStore {
                    line: Some(line_number),
                    message: "stored event is missing assigned sequence".to_owned(),
                })?;

            if sequence != expected_sequence {
                return Err(EventStoreError::InvalidStore {
                    line: Some(line_number),
                    message: format!(
                        "sequence must be strictly increasing from 1; expected {expected_sequence}, found {sequence}"
                    ),
                });
            }

            let expected_hash = canonical_event_hash(&event);
            let stored_hash = event.checksum.as_str();
            if stored_hash != expected_hash {
                return Err(EventStoreError::InvalidStore {
                    line: Some(line_number),
                    message: format!(
                        "checksum mismatch; expected {expected_hash}, found {stored_hash}"
                    ),
                });
            }

            let canonical_json = to_canonical_json(&event);
            if line != canonical_json {
                return Err(EventStoreError::InvalidStore {
                    line: Some(line_number),
                    message: "stored line is not canonical JSON".to_owned(),
                });
            }

            records.push(StoredEvent {
                sequence,
                event_hash: expected_hash,
                event,
                canonical_json,
            });
            expected_sequence =
                expected_sequence
                    .checked_add(1)
                    .ok_or_else(|| EventStoreError::InvalidStore {
                        line: Some(line_number),
                        message: "sequence overflow".to_owned(),
                    })?;
        }

        Ok(records)
    }
}

impl EventWriter for JsonlEventStore {
    fn append(&self, event: &NormalizedEvent) -> EventStoreResult<AppendReceipt> {
        let _guard = self
            .write_lock
            .lock()
            .map_err(|_| EventStoreError::LockPoisoned)?;
        self.append_locked(event)
    }
}

impl EventReader for JsonlEventStore {
    fn read_all_ordered(&self) -> EventStoreResult<Vec<StoredEvent>> {
        self.load_records()
    }

    fn read_by_sequence(&self, sequence: u64) -> EventStoreResult<Option<StoredEvent>> {
        Ok(self
            .load_records()?
            .into_iter()
            .find(|record| record.sequence == sequence))
    }

    fn read_range(
        &self,
        from_sequence: u64,
        to_sequence: u64,
    ) -> EventStoreResult<Vec<StoredEvent>> {
        if from_sequence > to_sequence {
            return Err(EventStoreError::InvalidStore {
                line: None,
                message: format!(
                    "invalid sequence range: from {from_sequence} is greater than to {to_sequence}"
                ),
            });
        }

        Ok(self
            .load_records()?
            .into_iter()
            .filter(|record| record.sequence >= from_sequence && record.sequence <= to_sequence)
            .collect())
    }

    fn read_correlation_chain(&self, correlation_id: &str) -> EventStoreResult<Vec<StoredEvent>> {
        if correlation_id.is_empty() {
            return Err(EventStoreError::InvalidStore {
                line: None,
                message: "correlation_id cannot be empty".to_owned(),
            });
        }

        Ok(self
            .load_records()?
            .into_iter()
            .filter(|record| record.event.correlation_id.as_str() == correlation_id)
            .collect())
    }
}

/// 计算规范事件哈希。
///
/// 中文说明：哈希输入是已规范化的事件 JSON，包含事件存储分配的 `sequence`，
/// 但不包含自引用的 `checksum` 字段。
pub fn canonical_event_hash(event: &NormalizedEvent) -> String {
    let value = event.to_json_value();
    canonical_event_hash_from_value(&value)
}

fn canonical_event_json_with_assigned_sequence(
    event: &NormalizedEvent,
    sequence: u64,
) -> EventStoreResult<(String, String)> {
    let mut value = event.to_json_value();
    let JsonValue::Object(fields) = &mut value else {
        return Err(EventStoreError::InvalidStore {
            line: None,
            message: "normalized event did not serialize to an object".to_owned(),
        });
    };

    fields.insert("sequence".to_owned(), sequence.to_json_value());
    let event_hash = canonical_event_hash_from_fields(fields);
    fields.insert("checksum".to_owned(), JsonValue::String(event_hash.clone()));

    let canonical_json = value.to_canonical_json();
    parse_event(&canonical_json, None)?;

    Ok((canonical_json, event_hash))
}

fn canonical_event_hash_from_value(value: &JsonValue) -> String {
    match value {
        JsonValue::Object(fields) => canonical_event_hash_from_fields(fields),
        _ => hash_canonical_bytes(value.to_canonical_json().as_bytes()),
    }
}

fn canonical_event_hash_from_fields(fields: &BTreeMap<String, JsonValue>) -> String {
    let mut hash_fields = fields.clone();
    hash_fields.remove("checksum");
    let canonical_without_checksum = JsonValue::Object(hash_fields).to_canonical_json();
    hash_canonical_bytes(canonical_without_checksum.as_bytes())
}

fn hash_canonical_bytes(bytes: &[u8]) -> String {
    format!("sha256:{}", sha256_hex(bytes))
}

fn parse_event(json: &str, line: Option<usize>) -> EventStoreResult<NormalizedEvent> {
    from_json_strict::<NormalizedEvent>(json).map_err(|error| EventStoreError::Contract {
        line,
        message: error.to_string(),
    })
}

fn ensure_parent_dir(path: &Path) -> EventStoreResult<()> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent).map_err(|error| io_error(parent, error))?;
        }
    }
    Ok(())
}

fn ensure_appendable_file(path: &Path) -> EventStoreResult<()> {
    if !path.exists() {
        return Ok(());
    }

    let mut file = File::open(path).map_err(|error| io_error(path, error))?;
    let len = file
        .metadata()
        .map_err(|error| io_error(path, error))?
        .len();
    if len == 0 {
        return Ok(());
    }

    file.seek(SeekFrom::End(-1))
        .map_err(|error| io_error(path, error))?;
    let mut last_byte = [0_u8; 1];
    file.read_exact(&mut last_byte)
        .map_err(|error| io_error(path, error))?;
    if last_byte[0] != b'\n' {
        return Err(EventStoreError::InvalidStore {
            line: None,
            message: "existing JSONL file does not end with a newline".to_owned(),
        });
    }
    Ok(())
}

fn io_error(path: &Path, error: std::io::Error) -> EventStoreError {
    EventStoreError::Io {
        path: path.to_path_buf(),
        message: error.to_string(),
    }
}

fn sha256_hex(input: &[u8]) -> String {
    let digest = sha256(input);
    let mut out = String::with_capacity(64);
    for byte in digest {
        use std::fmt::Write as _;
        write!(&mut out, "{byte:02x}").expect("writing to String cannot fail");
    }
    out
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
    digest
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::*;

    static TEST_COUNTER: AtomicU64 = AtomicU64::new(0);

    #[test]
    fn appends_jsonl_without_overwriting_history() {
        let store = test_store("append_only");

        let first = store
            .append_json(&event_json("event:01", "corr:append", "fixture-1", "first"))
            .expect("first append should succeed");
        let after_first = fs::read(store.path()).expect("event file should exist");

        let second = store
            .append_json(&event_json(
                "event:02",
                "corr:append",
                "fixture-2",
                "second",
            ))
            .expect("second append should succeed");
        let after_second = fs::read(store.path()).expect("event file should exist");

        assert_eq!(first.sequence, 1);
        assert_eq!(second.sequence, 2);
        assert!(after_second.starts_with(&after_first));
        assert!(after_second.len() > after_first.len());
        assert_eq!(
            2,
            store.read_all_ordered().expect("read should succeed").len()
        );
    }

    #[test]
    fn sequences_are_strictly_monotonic_and_read_by_sequence_is_stable() {
        let store = test_store("sequence_read");
        store
            .append_json(&event_json("event:01", "corr:seq", "fixture-1", "first"))
            .expect("first append should succeed");
        store
            .append_json(&event_json("event:02", "corr:seq", "fixture-2", "second"))
            .expect("second append should succeed");

        let ordered = store.read_range(1, 2).expect("range read should succeed");
        assert_eq!(
            ordered
                .iter()
                .map(|event| event.sequence)
                .collect::<Vec<_>>(),
            vec![1, 2]
        );

        let first_read = store
            .read_by_sequence(2)
            .expect("read should succeed")
            .expect("sequence 2 should exist");
        let second_read = store
            .read_by_sequence(2)
            .expect("read should succeed")
            .expect("sequence 2 should exist");

        assert_eq!(first_read, second_read);
        assert_eq!("event:02", first_read.event.event_id.as_str());
        assert!(store
            .read_by_sequence(3)
            .expect("read should succeed")
            .is_none());
    }

    #[test]
    fn canonical_hash_is_stable_and_rechecked_on_read() {
        let store = test_store("hash_stable");
        let receipt = store
            .append_json(&event_json("event:01", "corr:hash", "fixture-1", "first"))
            .expect("append should succeed");

        let stored = store
            .read_by_sequence(1)
            .expect("read should succeed")
            .expect("sequence 1 should exist");
        let reopened = JsonlEventStore::open(store.path().to_path_buf());
        let reread = reopened
            .read_by_sequence(1)
            .expect("reread should succeed")
            .expect("sequence 1 should exist");

        assert_eq!(receipt.event_hash, stored.event_hash);
        assert_eq!(stored.event_hash, canonical_event_hash(&stored.event));
        assert_eq!(stored.event_hash, reread.event_hash);
        assert_eq!(stored.canonical_json, reread.canonical_json);
        assert_eq!(stored.event.checksum.as_str(), stored.event_hash);
    }

    #[test]
    fn reads_correlation_chain_in_sequence_order() {
        let store = test_store("correlation_chain");
        store
            .append_json(&event_json("event:01", "corr:chain", "fixture-1", "first"))
            .expect("first append should succeed");
        store
            .append_json(&event_json("event:02", "corr:other", "fixture-2", "other"))
            .expect("second append should succeed");
        store
            .append_json(&event_json("event:03", "corr:chain", "fixture-3", "third"))
            .expect("third append should succeed");

        let chain = store
            .read_correlation_chain("corr:chain")
            .expect("chain read should succeed");

        assert_eq!(chain.len(), 2);
        assert_eq!(
            chain.iter().map(|event| event.sequence).collect::<Vec<_>>(),
            vec![1, 3]
        );
        assert_eq!("event:01", chain[0].event.event_id.as_str());
        assert_eq!("event:03", chain[1].event.event_id.as_str());
    }

    #[test]
    fn rejects_rewritten_or_corrupted_history() {
        let store = test_store("corrupt_history");
        store
            .append_json(&event_json(
                "event:01",
                "corr:corrupt",
                "fixture-1",
                "first",
            ))
            .expect("append should succeed");

        let stored = fs::read_to_string(store.path()).expect("event file should exist");
        let corrupted = stored.replacen("sha256:", "sha257:", 1);
        fs::write(store.path(), corrupted).expect("test should be able to corrupt its fixture");

        assert!(store.read_all_ordered().is_err());
    }

    #[test]
    fn sha256_matches_known_vector() {
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    fn test_store(name: &str) -> JsonlEventStore {
        let counter = TEST_COUNTER.fetch_add(1, Ordering::SeqCst);
        let mut dir = std::env::temp_dir();
        dir.push(format!(
            "easy-arb-eventstore-test-{}-{name}-{counter}",
            std::process::id()
        ));
        fs::create_dir_all(&dir).expect("test temp dir should be created");
        JsonlEventStore::open(dir.join("events.jsonl"))
    }

    fn event_json(
        event_id: &str,
        correlation_id: &str,
        source_sequence: &str,
        note: &str,
    ) -> String {
        format!(
            r#"{{
  "event_id": "{event_id}",
  "event_type": "AuditEvent",
  "event_version": "1.0.0",
  "timestamp_event": "2026-01-01T00:00:00Z",
  "timestamp_ingested": "2026-01-01T00:00:01Z",
  "source": "fixture",
  "sequence": 999,
  "source_sequence": "{source_sequence}",
  "correlation_id": "{correlation_id}",
  "causation_id": null,
  "schema_version": "1.0.0",
  "venue_id": null,
  "instrument_id": null,
  "strategy_id": null,
  "portfolio_state_ref": "state:01",
  "payload": {{
    "note": "{note}"
  }},
  "checksum": "sha256:placeholder"
}}"#
        )
    }
}
