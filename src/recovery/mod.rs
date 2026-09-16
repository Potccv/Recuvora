//! Recovery business modules. The engine in this file remains a closed,
//! side-effect-free simulation; approval and workflow have separate contracts.

pub mod incidents;
pub mod workflow;

#[cfg(test)]
#[path = "../../tests/workflow_support.rs"]
pub(crate) mod workflow_test_support;

pub mod approval;

use std::{
    collections::{BTreeMap, HashMap, HashSet, VecDeque},
    fs::{File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    path::Path,
    sync::{Arc, Mutex},
    time::Duration,
};

use fs2::FileExt;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::{
    sync::{mpsc, oneshot},
    task::{AbortHandle, Id, JoinHandle, JoinSet},
};

const MAX_TEXT_BYTES: usize = 128;
const MAX_TIMEOUT_MS: u64 = 3_600_000;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Simulation {
    Succeed,
    Fail,
    Hang,
    /// Simulates a missing executor receipt; never terminates the host process.
    Exit,
    VerificationFailed,
    Delay {
        millis: u64,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskSpec {
    pub id: String,
    pub target: String,
    pub simulation: Simulation,
    /// Only authorizes the closed, side-effect-free Simulation variants above.
    pub simulation_authorized: bool,
    pub timeout_ms: u64,
}

impl TaskSpec {
    pub fn simulated(
        id: impl Into<String>,
        target: impl Into<String>,
        simulation: Simulation,
    ) -> Self {
        Self {
            id: id.into(),
            target: target.into(),
            simulation,
            simulation_authorized: false,
            timeout_ms: 30_000,
        }
    }

    /// This is NOT identity verification or authorization for any real action.
    pub fn authorize_simulation(mut self) -> Self {
        self.simulation_authorized = true;
        self
    }

    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout_ms = u64::try_from(timeout.as_millis()).unwrap_or(u64::MAX);
        self
    }

    fn validate(&self) -> Result<(), EngineError> {
        for value in [&self.id, &self.target] {
            if value.is_empty()
                || value.len() > MAX_TEXT_BYTES
                || value.chars().any(char::is_control)
            {
                return Err(EngineError::Invalid(
                    "ID and target must contain 1..128 printable bytes",
                ));
            }
        }
        if self.timeout_ms == 0 || self.timeout_ms > MAX_TIMEOUT_MS {
            return Err(EngineError::Invalid(
                "timeout must be between 1 ms and one hour",
            ));
        }
        if matches!(self.simulation, Simulation::Delay { millis } if millis > MAX_TIMEOUT_MS) {
            return Err(EngineError::Invalid("simulation delay exceeds one hour"));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TaskState {
    Queued,
    Diagnosing,
    Executing,
    Verifying,
    Succeeded,
    Failed,
    Denied,
    Canceled,
    TimedOut,
    Unknown,
}

impl TaskState {
    pub fn is_terminal(self) -> bool {
        !matches!(
            self,
            Self::Queued | Self::Diagnosing | Self::Executing | Self::Verifying
        )
    }

    fn interrupted(self, timeout: bool) -> Self {
        match self {
            Self::Executing | Self::Verifying => Self::Unknown,
            _ if timeout => Self::TimedOut,
            _ => Self::Canceled,
        }
    }

    fn allows(self, next: Self) -> bool {
        match self {
            Self::Queued => matches!(next, Self::Diagnosing | Self::Canceled),
            Self::Diagnosing => matches!(
                next,
                Self::Executing | Self::Canceled | Self::TimedOut | Self::Failed
            ),
            Self::Executing => matches!(next, Self::Verifying | Self::Failed | Self::Unknown),
            Self::Verifying => matches!(next, Self::Succeeded | Self::Failed | Self::Unknown),
            _ => false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskSnapshot {
    pub spec: TaskSpec,
    pub state: TaskState,
    pub revision: u64,
}

#[derive(Debug, Clone)]
pub struct EngineConfig {
    pub max_concurrency: usize,
    pub max_queued: usize,
    pub max_tasks: usize,
    pub max_journal_bytes: u64,
}

impl Default for EngineConfig {
    fn default() -> Self {
        Self {
            max_concurrency: 4,
            max_queued: 64,
            max_tasks: 10_000,
            max_journal_bytes: 16 * 1024 * 1024,
        }
    }
}

impl EngineConfig {
    fn validate(&self) -> Result<(), EngineError> {
        if !(1..=256).contains(&self.max_concurrency)
            || !(1..=100_000).contains(&self.max_queued)
            || !(1..=1_000_000).contains(&self.max_tasks)
            || !(128..=1024 * 1024 * 1024).contains(&self.max_journal_bytes)
        {
            return Err(EngineError::Invalid("invalid engine capacity limits"));
        }
        Ok(())
    }
}

#[derive(Debug, Error)]
pub enum EngineError {
    #[error("invalid configuration or task: {0}")]
    Invalid(&'static str),
    #[error("task capacity or submission queue is full")]
    Capacity,
    #[error("task ID already exists with different content")]
    Conflict,
    #[error("engine is stopped or its durable store is unavailable")]
    Unavailable,
    #[error("another engine owns the runtime directory: {0}")]
    Locked(std::io::Error),
    #[error("journal error: {0}")]
    Storage(#[from] std::io::Error),
    #[error("invalid journal: {0}")]
    Corrupt(String),
    #[error("journal size limit reached")]
    JournalFull,
    #[error("internal task failed: {0}")]
    Internal(String),
}

type Reply<T> = oneshot::Sender<Result<T, EngineError>>;

enum Command {
    Submit(TaskSpec, Reply<TaskSnapshot>),
    Query(String, Reply<Option<TaskSnapshot>>),
    Cancel(String, Reply<Option<TaskSnapshot>>),
    Stage(String, TaskState, Reply<()>),
    Shutdown(Reply<()>),
}

/// Owns a single scheduler. Dropping it aborts the scheduler and its simulations.
pub struct Engine {
    tx: mpsc::Sender<Command>,
    actor: Option<JoinHandle<Result<(), EngineError>>>,
}

/// Cloneable service contract without ownership of engine shutdown.
#[derive(Clone)]
pub struct EngineHandle {
    tx: mpsc::Sender<Command>,
}

impl Engine {
    pub async fn open(
        data_dir: impl AsRef<Path>,
        config: EngineConfig,
    ) -> Result<Self, EngineError> {
        config.validate()?;
        let dir = data_dir.as_ref().to_owned();
        let limits = config.clone();
        let (mut journal, mut records) =
            tokio::task::spawn_blocking(move || Journal::open(&dir, &limits))
                .await
                .map_err(|error| EngineError::Internal(error.to_string()))??;
        // Do not replay accepted work or uncertain operations after a host restart.
        for record in records.values_mut() {
            if !record.state.is_terminal() {
                record.state = record.state.interrupted(false);
                record.revision += 1;
                journal.append(record).await?;
            }
        }
        let (tx, rx) = mpsc::channel(
            config
                .max_queued
                .saturating_add(config.max_concurrency)
                .min(4096),
        );
        let targets = records
            .values()
            .filter(|record| record.state == TaskState::Unknown)
            .map(|record| record.spec.target.clone())
            .collect();
        let actor = Actor {
            config,
            journal,
            records,
            queue: VecDeque::new(),
            workers: JoinSet::new(),
            active: HashMap::new(),
            worker_ids: HashMap::new(),
            targets,
            cancel_replies: HashMap::new(),
            tx: tx.clone(),
            rx,
        };
        Ok(Self {
            tx,
            actor: Some(tokio::spawn(actor.run())),
        })
    }

    pub fn handle(&self) -> EngineHandle {
        EngineHandle {
            tx: self.tx.clone(),
        }
    }

    pub async fn submit(&self, spec: TaskSpec) -> Result<TaskSnapshot, EngineError> {
        self.handle().submit(spec).await
    }

    pub async fn query(&self, id: &str) -> Result<Option<TaskSnapshot>, EngineError> {
        self.handle().query(id).await
    }

    pub async fn cancel(&self, id: &str) -> Result<Option<TaskSnapshot>, EngineError> {
        self.handle().cancel(id).await
    }

    pub async fn shutdown(mut self) -> Result<(), EngineError> {
        let (reply, result) = oneshot::channel();
        let sent = self.tx.send(Command::Shutdown(reply)).await.is_ok();
        if sent {
            result.await.map_err(|_| EngineError::Unavailable)??;
        }
        let outcome = match self.actor.as_mut() {
            Some(actor) => actor
                .await
                .map_err(|error| EngineError::Internal(error.to_string()))?,
            None => Err(EngineError::Unavailable),
        };
        self.actor.take();
        outcome
    }
}

impl EngineHandle {
    /// Returns only after durable acceptance (or a durable duplicate/denial).
    pub async fn submit(&self, spec: TaskSpec) -> Result<TaskSnapshot, EngineError> {
        spec.validate()?;
        let (reply, result) = oneshot::channel();
        self.tx
            .try_send(Command::Submit(spec, reply))
            .map_err(|error| match error {
                mpsc::error::TrySendError::Full(_) => EngineError::Capacity,
                mpsc::error::TrySendError::Closed(_) => EngineError::Unavailable,
            })?;
        result.await.map_err(|_| EngineError::Unavailable)?
    }

    pub async fn query(&self, id: &str) -> Result<Option<TaskSnapshot>, EngineError> {
        validate_lookup_id(id)?;
        let (reply, result) = oneshot::channel();
        self.tx
            .send(Command::Query(id.into(), reply))
            .await
            .map_err(|_| EngineError::Unavailable)?;
        result.await.map_err(|_| EngineError::Unavailable)?
    }

    /// Waits for the canceled simulation to be reaped before returning its state.
    pub async fn cancel(&self, id: &str) -> Result<Option<TaskSnapshot>, EngineError> {
        validate_lookup_id(id)?;
        let (reply, result) = oneshot::channel();
        self.tx
            .send(Command::Cancel(id.into(), reply))
            .await
            .map_err(|_| EngineError::Unavailable)?;
        result.await.map_err(|_| EngineError::Unavailable)?
    }
}

fn validate_lookup_id(id: &str) -> Result<(), EngineError> {
    if id.is_empty() || id.len() > MAX_TEXT_BYTES || id.chars().any(char::is_control) {
        return Err(EngineError::Invalid(
            "ID must contain 1..128 printable bytes",
        ));
    }
    Ok(())
}

impl Drop for Engine {
    fn drop(&mut self) {
        if let Some(actor) = &self.actor {
            actor.abort();
        }
    }
}

#[derive(Serialize, Deserialize)]
struct JournalRecord {
    format: u32,
    sequence: u64,
    snapshot: TaskSnapshot,
}

struct Journal {
    file: Arc<Mutex<File>>,
    lock: Arc<File>,
    bytes: u64,
    max_bytes: u64,
    sequence: u64,
}

impl Journal {
    fn open(
        dir: &Path,
        config: &EngineConfig,
    ) -> Result<(Self, BTreeMap<String, TaskSnapshot>), EngineError> {
        std::fs::create_dir_all(dir)?;
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(dir.join("writer.lock"))?;
        lock.try_lock_exclusive().map_err(EngineError::Locked)?;
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(dir.join("tasks.jsonl"))?;
        if file.metadata()?.len() > config.max_journal_bytes {
            return Err(EngineError::JournalFull);
        }
        let mut bytes = Vec::new();
        Read::by_ref(&mut file)
            .take(config.max_journal_bytes + 1)
            .read_to_end(&mut bytes)?;
        if bytes.len() as u64 > config.max_journal_bytes {
            return Err(EngineError::JournalFull);
        }
        let complete = bytes
            .iter()
            .rposition(|byte| *byte == b'\n')
            .map_or(0, |index| index + 1);
        let mut records: BTreeMap<String, TaskSnapshot> = BTreeMap::new();
        let mut sequence = 0;
        for complete_line in bytes[..complete].split_inclusive(|byte| *byte == b'\n') {
            let line = &complete_line[..complete_line.len() - 1];
            let entry: JournalRecord = serde_json::from_slice(line)
                .map_err(|error| EngineError::Corrupt(error.to_string()))?;
            if entry.format != 1 || entry.sequence != sequence + 1 {
                return Err(EngineError::Corrupt(
                    "unsupported format or nonsequential record".into(),
                ));
            }
            entry
                .snapshot
                .spec
                .validate()
                .map_err(|error| EngineError::Corrupt(error.to_string()))?;
            match records.get(&entry.snapshot.spec.id) {
                None if entry.snapshot.revision == 0
                    && entry.snapshot.state
                        == if entry.snapshot.spec.simulation_authorized {
                            TaskState::Queued
                        } else {
                            TaskState::Denied
                        } => {}
                Some(previous)
                    if previous.spec == entry.snapshot.spec
                        && previous.revision.checked_add(1) == Some(entry.snapshot.revision)
                        && previous.state.allows(entry.snapshot.state) => {}
                _ => {
                    return Err(EngineError::Corrupt(
                        "invalid task revision or transition".into(),
                    ));
                }
            }
            sequence = entry.sequence;
            records.insert(entry.snapshot.spec.id.clone(), entry.snapshot);
            if records.len() > config.max_tasks {
                return Err(EngineError::Capacity);
            }
        }
        if complete != bytes.len() {
            file.set_len(complete as u64)?;
            file.sync_all()?;
        }
        file.seek(SeekFrom::End(0))?;
        Ok((
            Self {
                file: Arc::new(Mutex::new(file)),
                lock: Arc::new(lock),
                bytes: complete as u64,
                max_bytes: config.max_journal_bytes,
                sequence,
            },
            records,
        ))
    }

    async fn append(&mut self, snapshot: &TaskSnapshot) -> Result<(), EngineError> {
        let record = JournalRecord {
            format: 1,
            sequence: self.sequence + 1,
            snapshot: snapshot.clone(),
        };
        let mut bytes =
            serde_json::to_vec(&record).map_err(|error| EngineError::Corrupt(error.to_string()))?;
        bytes.push(b'\n');
        if self.bytes.saturating_add(bytes.len() as u64) > self.max_bytes {
            return Err(EngineError::JournalFull);
        }
        let appended_bytes = bytes.len() as u64;
        let file = self.file.clone();
        let lock = self.lock.clone();
        // A canceled actor must not release the writer lock while a blocking
        // filesystem operation is still running. The I/O closure owns both Arcs.
        tokio::task::spawn_blocking(move || -> Result<(), EngineError> {
            let _write_lock = lock;
            let mut file = file
                .lock()
                .map_err(|_| EngineError::Internal("journal mutex poisoned".into()))?;
            file.write_all(&bytes)?;
            file.sync_all()?;
            Ok(())
        })
        .await
        .map_err(|error| EngineError::Internal(error.to_string()))??;
        self.bytes += appended_bytes;
        self.sequence += 1;
        Ok(())
    }
}

enum Outcome {
    Succeeded,
    Failed,
    MissingReceipt,
    TimedOut,
}

struct Actor {
    config: EngineConfig,
    journal: Journal,
    records: BTreeMap<String, TaskSnapshot>,
    queue: VecDeque<String>,
    workers: JoinSet<Outcome>,
    active: HashMap<String, AbortHandle>,
    worker_ids: HashMap<Id, String>,
    targets: HashSet<String>,
    cancel_replies: HashMap<String, Vec<Reply<Option<TaskSnapshot>>>>,
    tx: mpsc::Sender<Command>,
    rx: mpsc::Receiver<Command>,
}

impl Actor {
    async fn transition(&mut self, id: &str, state: TaskState) -> Result<(), EngineError> {
        let previous = self
            .records
            .get(id)
            .ok_or_else(|| EngineError::Internal("missing task".into()))?;
        if !previous.state.allows(state) {
            return Err(EngineError::Internal("invalid state transition".into()));
        }
        let mut next = previous.clone();
        next.state = state;
        next.revision += 1;
        self.journal.append(&next).await?;
        self.records.insert(id.into(), next);
        Ok(())
    }

    async fn dispatch(&mut self) -> Result<(), EngineError> {
        while self.active.len() < self.config.max_concurrency {
            let position = self.queue.iter().position(|id| {
                self.records
                    .get(id)
                    .is_some_and(|record| !self.targets.contains(&record.spec.target))
            });
            let Some(position) = position else { break };
            let Some(id) = self.queue.remove(position) else {
                break;
            };
            self.transition(&id, TaskState::Diagnosing).await?;
            let spec = self
                .records
                .get(&id)
                .ok_or_else(|| EngineError::Internal("missing dispatched task".into()))?
                .spec
                .clone();
            self.targets.insert(spec.target.clone());
            let sender = self.tx.clone();
            let timeout = Duration::from_millis(spec.timeout_ms);
            let worker = self.workers.spawn(async move {
                match tokio::time::timeout(timeout, simulate(spec, sender)).await {
                    Ok(outcome) => outcome,
                    Err(_) => Outcome::TimedOut,
                }
            });
            self.worker_ids.insert(worker.id(), id.clone());
            self.active.insert(id, worker);
        }
        Ok(())
    }

    async fn submit(
        &mut self,
        spec: TaskSpec,
        reply: Reply<TaskSnapshot>,
    ) -> Result<(), EngineError> {
        if let Some(previous) = self.records.get(&spec.id) {
            let _ = reply.send(if previous.spec == spec {
                Ok(previous.clone())
            } else {
                Err(EngineError::Conflict)
            });
            return Ok(());
        }
        if self.records.len() >= self.config.max_tasks
            || (spec.simulation_authorized && self.queue.len() >= self.config.max_queued)
        {
            let _ = reply.send(Err(EngineError::Capacity));
            return Ok(());
        }
        let snapshot = TaskSnapshot {
            state: if spec.simulation_authorized {
                TaskState::Queued
            } else {
                TaskState::Denied
            },
            spec,
            revision: 0,
        };
        if let Err(error) = self.journal.append(&snapshot).await {
            let _ = reply.send(Err(error));
            return Err(EngineError::Unavailable);
        }
        self.records
            .insert(snapshot.spec.id.clone(), snapshot.clone());
        if snapshot.state == TaskState::Queued {
            self.queue.push_back(snapshot.spec.id.clone());
        }
        let _ = reply.send(Ok(snapshot));
        Ok(())
    }

    async fn cancel(
        &mut self,
        id: String,
        reply: Reply<Option<TaskSnapshot>>,
    ) -> Result<(), EngineError> {
        if let Some(worker) = self.active.get(&id) {
            // Multiple outstanding cancel requests are bounded by the command channel
            // only until drained, so refuse excess waiters for this task explicitly.
            let waiters = self.cancel_replies.entry(id).or_default();
            if waiters.len() >= 64 {
                let _ = reply.send(Err(EngineError::Capacity));
            } else {
                waiters.push(reply);
                worker.abort();
            }
            return Ok(());
        }
        if self
            .records
            .get(&id)
            .is_some_and(|record| record.state == TaskState::Queued)
        {
            self.queue.retain(|queued| queued != &id);
            self.transition(&id, TaskState::Canceled).await?;
        }
        let _ = reply.send(Ok(self.records.get(&id).cloned()));
        Ok(())
    }

    async fn complete(
        &mut self,
        completed: Result<(Id, Outcome), tokio::task::JoinError>,
    ) -> Result<(), EngineError> {
        let worker_id = match &completed {
            Ok((id, _)) => *id,
            Err(error) => error.id(),
        };
        let id = self
            .worker_ids
            .remove(&worker_id)
            .ok_or_else(|| EngineError::Internal("unknown completed worker".into()))?;
        let previous = self
            .records
            .get(&id)
            .ok_or_else(|| EngineError::Internal("missing completed task".into()))?;
        let target = previous.spec.target.clone();
        let state = match completed {
            Ok((_, Outcome::Succeeded)) => TaskState::Succeeded,
            Ok((_, Outcome::Failed)) => TaskState::Failed,
            Ok((_, Outcome::MissingReceipt)) => previous.state.interrupted(false),
            Ok((_, Outcome::TimedOut)) => previous.state.interrupted(true),
            Err(_) => previous.state.interrupted(false),
        };
        self.transition(&id, state).await?;
        self.active.remove(&id);
        // Unknown work must be reconciled before ANY conflicting follow-up work.
        // There is no real reconciliation/authorization API in this prototype.
        if state != TaskState::Unknown {
            self.targets.remove(&target);
        }
        if let Some(waiters) = self.cancel_replies.remove(&id) {
            for reply in waiters {
                let _ = reply.send(Ok(self.records.get(&id).cloned()));
            }
        }
        Ok(())
    }

    async fn stop(&mut self) -> Result<(), EngineError> {
        self.rx.close();
        for worker in self.active.values() {
            worker.abort();
        }
        while let Some(completed) = self.workers.join_next_with_id().await {
            self.complete(completed).await?;
        }
        while let Some(id) = self.queue.pop_front() {
            self.transition(&id, TaskState::Canceled).await?;
        }
        // A received but unhandled command was never acknowledged as durable.
        while self.rx.try_recv().is_ok() {}
        Ok(())
    }

    async fn run(mut self) -> Result<(), EngineError> {
        loop {
            self.dispatch().await?;
            tokio::select! {
                completed = self.workers.join_next_with_id(), if !self.workers.is_empty() => {
                    if let Some(completed) = completed { self.complete(completed).await?; }
                }
                command = self.rx.recv() => {
                    match command {
                        Some(Command::Submit(spec, reply)) => self.submit(spec, reply).await?,
                        Some(Command::Query(id, reply)) => { let _ = reply.send(Ok(self.records.get(&id).cloned())); }
                        Some(Command::Cancel(id, reply)) => self.cancel(id, reply).await?,
                        Some(Command::Stage(id, state, reply)) => {
                            // Timed-out or canceled simulations cannot advance a finished task.
                            if self.active.contains_key(&id) && !self.cancel_replies.contains_key(&id) && !reply.is_closed() {
                                self.transition(&id, state).await?;
                                let _ = reply.send(Ok(()));
                            } else {
                                let _ = reply.send(Err(EngineError::Unavailable));
                            }
                        }
                        Some(Command::Shutdown(reply)) => {
                            match self.stop().await {
                                Ok(()) => { let _ = reply.send(Ok(())); return Ok(()); }
                                Err(error) => { let _ = reply.send(Err(error)); return Err(EngineError::Unavailable); }
                            }
                        }
                        None => return self.stop().await,
                    }
                }
            }
        }
    }
}

async fn stage(
    sender: &mpsc::Sender<Command>,
    id: &str,
    state: TaskState,
) -> Result<(), EngineError> {
    let (reply, result) = oneshot::channel();
    sender
        .send(Command::Stage(id.into(), state, reply))
        .await
        .map_err(|_| EngineError::Unavailable)?;
    result.await.map_err(|_| EngineError::Unavailable)?
}

async fn simulate(spec: TaskSpec, sender: mpsc::Sender<Command>) -> Outcome {
    // Yielding is part of the mock diagnosis; there is no real target inspection.
    tokio::task::yield_now().await;
    if !spec.simulation_authorized
        || stage(&sender, &spec.id, TaskState::Executing)
            .await
            .is_err()
    {
        return Outcome::MissingReceipt;
    }
    match spec.simulation {
        Simulation::Fail => return Outcome::Failed,
        Simulation::Exit => return Outcome::MissingReceipt,
        Simulation::Hang => std::future::pending::<()>().await,
        Simulation::Delay { millis } => tokio::time::sleep(Duration::from_millis(millis)).await,
        Simulation::Succeed | Simulation::VerificationFailed => {}
    }
    if stage(&sender, &spec.id, TaskState::Verifying)
        .await
        .is_err()
    {
        return Outcome::MissingReceipt;
    }
    tokio::task::yield_now().await;
    if matches!(spec.simulation, Simulation::VerificationFailed) {
        Outcome::Failed
    } else {
        Outcome::Succeeded
    }
}
