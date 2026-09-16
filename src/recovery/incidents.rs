//! Durable monitoring facts. Acknowledgement is attribution, never repair authority.
mod paths;

use fs2::FileExt;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::BTreeMap,
    fs::File,
    io::{BufRead, BufReader, Write},
    path::{Path, PathBuf},
};
use thiserror::Error;

const MAX_RECORD_BYTES: usize = 524_288;
const MAX_CHECKPOINT_BYTES: usize = 131_072;
const MAX_EVIDENCE_BYTES: usize = 16_384;
pub const MAX_ACK_NOTE_BYTES: usize = 4096;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IncidentKind {
    Target,
    Coverage,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SignalCondition {
    Active,
    Clear,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IncidentStatus {
    Open,
    Acknowledged,
    Resolved,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IncidentSignal {
    pub monitor_id: String,
    pub target_id: String,
    pub rule_id: String,
    pub kind: IncidentKind,
    pub condition: SignalCondition,
    pub summary: String,
    pub evidence: Value,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IncidentAcknowledgement {
    pub actor: String,
    pub note: String,
    pub at_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IncidentRecord {
    pub id: String,
    pub revision: u64,
    pub monitor_id: String,
    pub target_id: String,
    pub rule_id: String,
    pub kind: IncidentKind,
    pub status: IncidentStatus,
    pub condition: SignalCondition,
    pub summary: String,
    pub evidence: Value,
    pub first_seen: u64,
    pub last_seen: u64,
    pub resolved_at: Option<u64>,
    pub acknowledgement: Option<IncidentAcknowledgement>,
    pub occurrences: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Checkpoint {
    pub sequence: u64,
    pub value: Value,
    pub updated_at_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MonitorCommit {
    pub monitor_id: String,
    pub sequence: u64,
    pub checkpoint: Value,
    pub signals: Vec<IncidentSignal>,
    pub now_ms: u64,
}

#[derive(Debug, Clone)]
pub struct IncidentStoreConfig {
    pub max_incidents: usize,
    pub max_monitors: usize,
    pub max_journal_bytes: u64,
}

impl Default for IncidentStoreConfig {
    fn default() -> Self {
        Self {
            max_incidents: 10_000,
            max_monitors: 256,
            max_journal_bytes: 64 * 1024 * 1024,
        }
    }
}

#[derive(Debug, Error)]
pub enum IncidentError {
    #[error("invalid incident input: {0}")]
    Invalid(String),
    #[error("incident state conflict: {0}")]
    Conflict(String),
    #[error("incident not found: {0}")]
    NotFound(String),
    #[error("incident capacity exhausted: {0}")]
    Capacity(String),
    #[error("corrupt incident journal: {0}")]
    Corrupt(String),
    #[error("incident storage I/O: {0}")]
    Io(#[from] std::io::Error),
    #[error("incident storage unavailable: {0}")]
    Unavailable(String),
}

type IncidentKey = (String, String, String, IncidentKind);

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum Event {
    Monitor {
        commit: MonitorCommit,
    },
    Acknowledge {
        id: String,
        expected_revision: u64,
        actor: String,
        note: String,
        now_ms: u64,
    },
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct JournalEntry {
    format: u32,
    sequence: u64,
    event: Event,
}

struct Prepared {
    records: Vec<IncidentRecord>,
    monitor: Option<MonitorState>,
}

struct MonitorState {
    commit: MonitorCommit,
    updated_at_ms: u64,
}

/// Single synchronous writer. The monitor engine owns this through one mutex;
/// read projections and acknowledgement use the same authority and lock.
pub struct IncidentStore {
    journal: Option<File>,
    path: PathBuf,
    _directories: Vec<File>,
    config: IncidentStoreConfig,
    bytes: u64,
    sequence: u64,
    poisoned: bool,
    records: BTreeMap<String, IncidentRecord>,
    active: BTreeMap<IncidentKey, String>,
    monitors: BTreeMap<String, MonitorState>,
}

impl IncidentStore {
    /// `path` is an absolute journal file path outside the project source tree.
    pub fn open(
        path: impl AsRef<Path>,
        config: IncidentStoreConfig,
    ) -> Result<Self, IncidentError> {
        if !(1..=100_000).contains(&config.max_incidents)
            || !(1..=4096).contains(&config.max_monitors)
            || !(1..=1024 * 1024 * 1024).contains(&config.max_journal_bytes)
        {
            return Err(IncidentError::Invalid("store limits out of range".into()));
        }
        let path = path.as_ref().to_path_buf();
        let (journal, directories) = paths::open(&path)?;
        journal
            .try_lock_exclusive()
            .map_err(|error| IncidentError::Unavailable(format!("journal writer lock: {error}")))?;
        let bytes = journal.metadata()?.len();
        if bytes > config.max_journal_bytes {
            return Err(IncidentError::Capacity("journal bytes".into()));
        }
        let mut store = Self {
            journal: Some(journal),
            path,
            _directories: directories,
            config,
            bytes,
            sequence: 0,
            poisoned: false,
            records: BTreeMap::new(),
            active: BTreeMap::new(),
            monitors: BTreeMap::new(),
        };
        store.replay()?;
        Ok(store)
    }

    pub fn checkpoint(&self, monitor_id: &str) -> Option<Checkpoint> {
        self.monitors.get(monitor_id).map(|state| Checkpoint {
            sequence: state.commit.sequence,
            value: state.commit.checkpoint.clone(),
            updated_at_ms: state.updated_at_ms,
        })
    }

    /// Starts at one and advances exactly one sequence per monitor. Only the
    /// latest identical full commit can be retried without another journal write.
    pub fn commit(&mut self, commit: MonitorCommit) -> Result<(), IncidentError> {
        self.append(Event::Monitor { commit })
    }

    pub fn get(&self, id: &str) -> Option<IncidentRecord> {
        self.records.get(id).cloned()
    }

    pub fn list(&self) -> Vec<IncidentRecord> {
        self.map_records(Clone::clone)
    }

    /// Projects summaries without cloning every stored evidence object.
    pub fn map_records<T>(&self, projection: impl FnMut(&IncidentRecord) -> T) -> Vec<T> {
        self.records.values().map(projection).collect()
    }

    /// Releases storage ownership while keeping the final in-memory read view.
    /// All accepted events have already been synced; closing never accepts more.
    pub fn close(&mut self) -> Result<(), IncidentError> {
        self.poisoned = true;
        self.journal.take();
        self._directories.clear();
        Ok(())
    }

    /// The caller supplies its authenticated host identity. Acknowledgement
    /// records attention only; it does not clear the condition or grant actions.
    pub fn acknowledge(
        &mut self,
        id: &str,
        expected_revision: u64,
        actor: &str,
        note: &str,
        now_ms: u64,
    ) -> Result<IncidentRecord, IncidentError> {
        self.append(Event::Acknowledge {
            id: id.into(),
            expected_revision,
            actor: actor.into(),
            note: note.into(),
            now_ms,
        })?;
        self.get(id)
            .ok_or_else(|| IncidentError::NotFound(id.into()))
    }

    fn append(&mut self, event: Event) -> Result<(), IncidentError> {
        if self.poisoned {
            return Err(IncidentError::Unavailable(
                "previous write failed; reopen after checking storage".into(),
            ));
        }
        let sequence = next(self.sequence)?;
        let Some(prepared) = self.prepare(&event, sequence)? else {
            return Ok(());
        };
        let entry = JournalEntry {
            format: 1,
            sequence,
            event,
        };
        let mut bytes = serde_json::to_vec(&entry)
            .map_err(|error| IncidentError::Invalid(error.to_string()))?;
        bytes.push(b'\n');
        if bytes.len() > MAX_RECORD_BYTES {
            return Err(IncidentError::Capacity(
                "single journal record bytes".into(),
            ));
        }
        let total = self
            .bytes
            .checked_add(bytes.len() as u64)
            .filter(|bytes| *bytes <= self.config.max_journal_bytes)
            .ok_or_else(|| IncidentError::Capacity("journal bytes".into()))?;
        // A failed append may have written a partial record. Stop all subsequent
        // mutations; never advance in-memory facts or the monitor checkpoint.
        self.poisoned = true;
        let journal = self
            .journal
            .as_mut()
            .ok_or_else(|| IncidentError::Unavailable("store closed".into()))?;
        paths::validate_current(&self.path, journal)?;
        if journal.metadata()?.len() != self.bytes {
            return Err(IncidentError::Unavailable(
                "journal length changed externally".into(),
            ));
        }
        journal.write_all(&bytes)?;
        journal.sync_data()?;
        self.install(prepared);
        self.sequence = sequence;
        self.bytes = total;
        self.poisoned = false;
        Ok(())
    }

    fn replay(&mut self) -> Result<(), IncidentError> {
        // read_until is bounded by Take, so a malicious oversized line cannot
        // allocate up to the total journal limit before rejection.
        use std::io::Read;
        let journal = self
            .journal
            .as_ref()
            .ok_or_else(|| IncidentError::Unavailable("store closed".into()))?;
        let mut reader = BufReader::new(journal.try_clone()?);
        let mut read_bytes = 0u64;
        loop {
            let mut bytes = Vec::new();
            let count = reader
                .by_ref()
                .take((MAX_RECORD_BYTES + 1) as u64)
                .read_until(b'\n', &mut bytes)?;
            if count == 0 {
                break;
            }
            if count > MAX_RECORD_BYTES || bytes.last() != Some(&b'\n') {
                return Err(IncidentError::Corrupt(
                    "oversized or incomplete record".into(),
                ));
            }
            read_bytes += count as u64;
            if read_bytes > self.config.max_journal_bytes {
                return Err(IncidentError::Capacity(
                    "journal bytes during replay".into(),
                ));
            }
            let entry: JournalEntry = serde_json::from_slice(&bytes)
                .map_err(|error| IncidentError::Corrupt(error.to_string()))?;
            if entry.format != 1 || entry.sequence != next(self.sequence)? {
                return Err(IncidentError::Corrupt("format or journal sequence".into()));
            }
            let prepared = self
                .prepare(&entry.event, entry.sequence)
                .map_err(|error| IncidentError::Corrupt(error.to_string()))?
                .ok_or_else(|| IncidentError::Corrupt("duplicate committed event".into()))?;
            self.install(prepared);
            self.sequence = entry.sequence;
        }
        if read_bytes != self.bytes {
            return Err(IncidentError::Corrupt(
                "journal length changed during replay".into(),
            ));
        }
        Ok(())
    }

    fn prepare(&self, event: &Event, sequence: u64) -> Result<Option<Prepared>, IncidentError> {
        match event {
            Event::Monitor { commit } => self.prepare_monitor(commit, sequence),
            Event::Acknowledge {
                id,
                expected_revision,
                actor,
                note,
                now_ms,
            } => {
                bounded_text(id, 256, "incident id", false)?;
                bounded_text(actor, 256, "actor", false)?;
                bounded_text(note, MAX_ACK_NOTE_BYTES, "acknowledgement note", true)?;
                let mut record = self
                    .records
                    .get(id)
                    .cloned()
                    .ok_or_else(|| IncidentError::NotFound(id.clone()))?;
                if record.revision != *expected_revision || record.status != IncidentStatus::Open {
                    return Err(IncidentError::Conflict(
                        "revision changed or incident is not open".into(),
                    ));
                }
                record.revision = next(record.revision)?;
                record.status = IncidentStatus::Acknowledged;
                record.acknowledgement = Some(IncidentAcknowledgement {
                    actor: actor.clone(),
                    note: note.clone(),
                    at_ms: (*now_ms).max(record.last_seen),
                });
                Ok(Some(Prepared {
                    records: vec![record],
                    monitor: None,
                }))
            }
        }
    }

    fn prepare_monitor(
        &self,
        commit: &MonitorCommit,
        sequence: u64,
    ) -> Result<Option<Prepared>, IncidentError> {
        bounded_text(&commit.monitor_id, 256, "monitor id", false)?;
        bounded_object(&commit.checkpoint, MAX_CHECKPOINT_BYTES, "checkpoint")?;
        if commit.signals.len() > 128 {
            return Err(IncidentError::Capacity("signals per commit".into()));
        }
        match self.monitors.get(&commit.monitor_id) {
            Some(previous) if previous.commit == *commit => return Ok(None),
            Some(previous) if commit.sequence != next(previous.commit.sequence)? => {
                return Err(IncidentError::Conflict("monitor sequence".into()));
            }
            None if commit.sequence != 1 => {
                return Err(IncidentError::Conflict(
                    "first monitor sequence must be one".into(),
                ));
            }
            None if self.monitors.len() >= self.config.max_monitors => {
                return Err(IncidentError::Capacity("monitors".into()));
            }
            _ => {}
        }
        let mut timestamp = self
            .monitors
            .get(&commit.monitor_id)
            .map_or(commit.now_ms, |previous| {
                commit.now_ms.max(previous.updated_at_ms)
            });
        let mut records: Vec<IncidentRecord> = Vec::new();
        let mut additions = 0;
        for (index, signal) in commit.signals.iter().enumerate() {
            if signal.monitor_id != commit.monitor_id {
                return Err(IncidentError::Invalid(
                    "signal monitor differs from commit".into(),
                ));
            }
            bounded_text(&signal.target_id, 256, "target id", false)?;
            bounded_text(&signal.rule_id, 256, "rule id", false)?;
            bounded_text(&signal.summary, 2048, "summary", false)?;
            bounded_object(&signal.evidence, MAX_EVIDENCE_BYTES, "evidence")?;
            let key = (
                signal.monitor_id.clone(),
                signal.target_id.clone(),
                signal.rule_id.clone(),
                signal.kind,
            );
            // Process every observation in order, including repeated keys in a
            // single batch. A later clear must not erase an earlier episode.
            let current = records
                .iter()
                .rev()
                .find(|record| {
                    record.monitor_id == signal.monitor_id
                        && record.target_id == signal.target_id
                        && record.rule_id == signal.rule_id
                        && record.kind == signal.kind
                })
                .or_else(|| self.active.get(&key).and_then(|id| self.records.get(id)))
                .filter(|record| record.status != IncidentStatus::Resolved);
            let mut record = match current {
                Some(current) => {
                    timestamp = timestamp
                        .max(current.last_seen)
                        .max(current.acknowledgement.as_ref().map_or(0, |ack| ack.at_ms));
                    let mut record = current.clone();
                    record.revision = next(record.revision)?;
                    record
                }
                None if signal.condition != SignalCondition::Active => continue,
                None => {
                    additions += 1;
                    IncidentRecord {
                        id: format!("incident-{sequence:016x}-{index:04x}"),
                        revision: 1,
                        monitor_id: signal.monitor_id.clone(),
                        target_id: signal.target_id.clone(),
                        rule_id: signal.rule_id.clone(),
                        kind: signal.kind,
                        status: IncidentStatus::Open,
                        condition: signal.condition,
                        summary: signal.summary.clone(),
                        evidence: signal.evidence.clone(),
                        first_seen: timestamp,
                        last_seen: timestamp,
                        resolved_at: None,
                        acknowledgement: None,
                        occurrences: 0,
                    }
                }
            };
            record.condition = signal.condition;
            record.summary.clone_from(&signal.summary);
            record.evidence.clone_from(&signal.evidence);
            record.last_seen = timestamp;
            match signal.condition {
                SignalCondition::Active => record.occurrences = next(record.occurrences)?,
                SignalCondition::Clear => {
                    record.status = IncidentStatus::Resolved;
                    record.resolved_at = Some(timestamp);
                }
                SignalCondition::Unknown => {}
            }
            records.push(record);
        }
        if self.records.len().saturating_add(additions) > self.config.max_incidents {
            return Err(IncidentError::Capacity("incident episodes".into()));
        }
        Ok(Some(Prepared {
            records,
            monitor: Some(MonitorState {
                commit: commit.clone(),
                updated_at_ms: timestamp,
            }),
        }))
    }

    fn install(&mut self, prepared: Prepared) {
        for record in prepared.records {
            let key = (
                record.monitor_id.clone(),
                record.target_id.clone(),
                record.rule_id.clone(),
                record.kind,
            );
            if record.status == IncidentStatus::Resolved {
                self.active.remove(&key);
            } else {
                self.active.insert(key, record.id.clone());
            }
            self.records.insert(record.id.clone(), record);
        }
        if let Some(state) = prepared.monitor {
            self.monitors.insert(state.commit.monitor_id.clone(), state);
        }
    }
}

fn next(value: u64) -> Result<u64, IncidentError> {
    value
        .checked_add(1)
        .ok_or_else(|| IncidentError::Capacity("sequence overflow".into()))
}

fn bounded_text(value: &str, max: usize, field: &str, empty: bool) -> Result<(), IncidentError> {
    if value.len() > max || (!empty && value.trim().is_empty()) || value.contains('\0') {
        return Err(IncidentError::Invalid(format!(
            "{field} is empty, contains NUL or exceeds {max} bytes"
        )));
    }
    Ok(())
}

fn bounded_object(value: &Value, max: usize, field: &str) -> Result<(), IncidentError> {
    if !value.is_object() {
        return Err(IncidentError::Invalid(format!(
            "{field} must be a JSON object"
        )));
    }
    // Bound nesting before recursive serialization, and count encoded bytes
    // without allocating a second potentially unbounded copy of caller data.
    let mut pending = vec![(value, 0usize)];
    let mut nodes = 0usize;
    while let Some((value, depth)) = pending.pop() {
        nodes += 1;
        if depth > 32 || nodes > max {
            return Err(IncidentError::Capacity(format!("{field} JSON complexity")));
        }
        match value {
            Value::Array(values) => {
                if values.len() > max.saturating_sub(pending.len()) {
                    return Err(IncidentError::Capacity(format!("{field} JSON complexity")));
                }
                pending.extend(values.iter().map(|value| (value, depth + 1)));
            }
            Value::Object(values) => {
                if values.len() > max.saturating_sub(pending.len())
                    || values.keys().any(|key| key.len() > max)
                {
                    return Err(IncidentError::Capacity(format!("{field} JSON complexity")));
                }
                pending.extend(values.values().map(|value| (value, depth + 1)));
            }
            Value::String(value) if value.len() > max => {
                return Err(IncidentError::Capacity(format!(
                    "{field} exceeds {max} bytes"
                )));
            }
            _ => {}
        }
    }
    serde_json::to_writer(ByteLimit { remaining: max }, value)
        .map_err(|_| IncidentError::Capacity(format!("{field} exceeds {max} bytes")))?;
    Ok(())
}

struct ByteLimit {
    remaining: usize,
}

impl Write for ByteLimit {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if bytes.len() > self.remaining {
            return Err(std::io::Error::other("JSON byte limit"));
        }
        self.remaining -= bytes.len();
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
#[path = "../../tests/incident_storage.rs"]
mod storage_tests;
