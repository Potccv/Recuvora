//! Single-writer journal opening, recovery and durable append.
use super::paths::{reject_nonregular, validate_external_dir, validate_open_file};
use super::*;
use fs2::FileExt;
use std::{
    fs::OpenOptions,
    io::{Read, Write},
    path::Path,
};

impl ApprovalStore {
    pub fn open(
        data_dir: impl AsRef<Path>,
        config: ApprovalStoreConfig,
        now: u64,
    ) -> Result<Self, ApprovalError> {
        if config.max_requests == 0
            || config.max_requests > 100_000
            || config.max_journal_bytes == 0
            || config.max_journal_bytes > 128 * 1024 * 1024
        {
            return Err(ApprovalError::Invalid("store limits"));
        }
        let dir = data_dir.as_ref();
        validate_external_dir(dir)?;
        std::fs::create_dir_all(dir)?;
        validate_external_dir(dir)?;
        let lock_path = dir.join("approvals.lock");
        let journal_path = dir.join("approvals.jsonl");
        reject_nonregular(&lock_path)?;
        reject_nonregular(&journal_path)?;
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(lock_path)?;
        validate_open_file(&lock)?;
        lock.try_lock_exclusive().map_err(ApprovalError::Locked)?;
        let mut file = OpenOptions::new()
            .read(true)
            .append(true)
            .create(true)
            .open(journal_path)?;
        validate_open_file(&file)?;
        if file.metadata()?.len() > config.max_journal_bytes {
            return Err(ApprovalError::Capacity);
        }
        let mut bytes = Vec::new();
        Read::by_ref(&mut file)
            .take(config.max_journal_bytes + 1)
            .read_to_end(&mut bytes)?;
        if bytes.len() as u64 > config.max_journal_bytes {
            return Err(ApprovalError::Capacity);
        }
        // An incomplete append may be an interrupted execution intent. Do not
        // discard it and then allow an old approval to be used again.
        if !bytes.is_empty() && bytes.last() != Some(&b'\n') {
            return Err(ApprovalError::Corrupt("incomplete final record".into()));
        }
        let mut store = Self {
            file,
            _lock: lock,
            config,
            sequence: 0,
            bytes: bytes.len() as u64,
            poisoned: false,
            records: BTreeMap::new(),
            identity: Arc::new(()),
        };
        for complete in bytes.split_inclusive(|byte| *byte == b'\n') {
            if complete.len() > MAX_RECORD {
                return Err(ApprovalError::Corrupt("oversized record".into()));
            }
            let entry: JournalEntry = serde_json::from_slice(&complete[..complete.len() - 1])
                .map_err(|error| ApprovalError::Corrupt(error.to_string()))?;
            if entry.format != 1 || store.sequence.checked_add(1) != Some(entry.sequence) {
                return Err(ApprovalError::Corrupt("format or sequence".into()));
            }
            let record = store
                .apply(&entry.event, entry.now, entry.sequence)
                .map_err(|error| ApprovalError::Corrupt(error.to_string()))?;
            store.sequence = entry.sequence;
            store
                .records
                .insert(record.request.request_id.clone(), record);
        }
        let interrupted: Vec<String> = store
            .records
            .values()
            .filter(|record| record.state == ApprovalState::Executing)
            .map(|record| record.request.request_id.clone())
            .collect();
        for id in interrupted {
            store.change(&id, Change::RecoverUnknown, now)?;
        }
        Ok(store)
    }

    pub(super) fn append(
        &mut self,
        event: Event,
        now: u64,
    ) -> Result<ApprovalRecord, ApprovalError> {
        self.available()?;
        let sequence = self
            .sequence
            .checked_add(1)
            .ok_or(ApprovalError::Capacity)?;
        let next = self.apply(&event, now, sequence)?;
        let mut bytes = serde_json::to_vec(&JournalEntry {
            format: 1,
            sequence,
            now,
            event,
        })?;
        bytes.push(b'\n');
        if bytes.len() > MAX_RECORD
            || self.bytes.saturating_add(bytes.len() as u64) > self.config.max_journal_bytes
        {
            return Err(ApprovalError::Capacity);
        }
        self.poisoned = true;
        self.file.write_all(&bytes)?;
        self.file.sync_data()?;
        self.poisoned = false;
        self.bytes += bytes.len() as u64;
        self.sequence = sequence;
        self.records
            .insert(next.request.request_id.clone(), next.clone());
        Ok(next)
    }
}
