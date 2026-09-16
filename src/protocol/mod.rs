//! Bounded versioned JSONL sessions for explicitly trusted external programs.
mod schema;
pub use schema::{validate_schema, validate_value};

use crate::framework::Cancellation;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use thiserror::Error;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::mpsc;

pub const PROTOCOL_VERSION: u32 = 1;
pub const MAX_FRAME_BYTES: usize = 1024 * 1024;
const GRACE: Duration = Duration::from_secs(10);

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ExtensionKind {
    Node,
    Plugin,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct MethodDeclaration {
    pub name: String,
    pub read_only: bool,
    pub input_schema: Value,
    pub output_schema: Value,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ContractDeclaration {
    pub id: String,
    pub version: u32,
    pub methods: Vec<MethodDeclaration>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ExtensionMetadata {
    pub protocol_version: u32,
    pub id: String,
    pub kind: ExtensionKind,
    pub contracts: Vec<ContractDeclaration>,
    #[serde(default)]
    pub capabilities: Vec<String>,
    #[serde(default)]
    pub workspaces: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Message {
    Hello {
        protocol_version: u32,
        expected_id: String,
        kind: ExtensionKind,
    },
    Ready {
        #[serde(flatten)]
        metadata: ExtensionMetadata,
    },
    Call {
        id: String,
        contract: String,
        version: u32,
        method: String,
        params: Value,
        timeout_ms: u64,
    },
    Result {
        id: String,
        result: Value,
    },
    Error {
        id: String,
        code: String,
        message: String,
        outcome: Outcome,
    },
    Callback {
        id: String,
        parent_id: String,
        method: String,
        params: Value,
    },
    Cancel {
        id: String,
    },
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    Rejected,
    Unknown,
    Cancelled,
}

#[derive(Debug, Error)]
pub enum ExtensionError {
    #[error("invalid extension configuration: {0}")]
    Configuration(String),
    #[error("extension unavailable: {0}")]
    Unavailable(String),
    #[error("extension protocol violation: {0}")]
    Protocol(String),
    #[error("extension rejected request: {0}")]
    Rejected(String),
    #[error("extension call {call_id} outcome is unknown: {message}")]
    Unknown { call_id: String, message: String },
    #[error("extension call cancelled before dispatch")]
    Cancelled,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CommandSpec {
    pub program: PathBuf,
    #[serde(default)]
    pub args: Vec<String>,
    pub cwd: PathBuf,
    #[serde(default)]
    pub env: std::collections::BTreeMap<String, String>,
}

impl CommandSpec {
    pub fn validate(&self) -> Result<(), ExtensionError> {
        if !self.program.is_absolute()
            || !self.program.is_file()
            || !self.cwd.is_absolute()
            || !self.cwd.is_dir()
        {
            return Err(ExtensionError::Configuration(
                "program and cwd must be existing absolute local paths".into(),
            ));
        }
        if self.env.len() > 32
            || self.env.iter().any(|(k, v)| {
                k.is_empty()
                    || k.len() > 128
                    || k.contains(['=', '\0'])
                    || v.len() > 8192
                    || v.contains('\0')
            })
        {
            return Err(ExtensionError::Configuration(
                "invalid environment overrides".into(),
            ));
        }
        if self.args.len() > 64 || self.args.iter().any(|s| s.len() > 8192 || s.contains('\0')) {
            return Err(ExtensionError::Configuration(
                "invalid command arguments".into(),
            ));
        }
        Ok(())
    }
}

pub type CallbackFuture<'a> =
    Pin<Box<dyn Future<Output = Result<Value, ExtensionError>> + Send + 'a>>;
pub trait CallbackHandler: Send + Sync {
    fn call<'a>(
        &'a self,
        method: String,
        params: Value,
        cancellation: Cancellation,
    ) -> CallbackFuture<'a>;
}

#[derive(Clone)]
pub struct ExtensionClient {
    pub id: String,
    pub kind: ExtensionKind,
    pub command: CommandSpec,
}

#[derive(Clone, Debug)]
pub struct ExtensionCall {
    pub contract: String,
    pub version: u32,
    pub method: String,
    pub params: Value,
    pub timeout: Duration,
}

impl ExtensionClient {
    pub async fn probe(&self) -> Result<ExtensionMetadata, ExtensionError> {
        let mut session = self.open().await?;
        let metadata = session.metadata.clone();
        if !session.close().await {
            return Err(ExtensionError::Protocol(
                "probe transport did not close cleanly".into(),
            ));
        }
        Ok(metadata)
    }

    /// A supervisor outlives a dropped caller, cancels work, awaits any trusted
    /// callback, and reaps the transport process. No call is automatically replayed.
    pub async fn call(
        &self,
        call: ExtensionCall,
        expected: ExtensionMetadata,
        cancellation: Cancellation,
        handler: Option<Arc<dyn CallbackHandler>>,
        permit: Option<tokio::sync::OwnedSemaphorePermit>,
    ) -> Result<Value, ExtensionError> {
        if call.timeout.is_zero() || call.timeout > Duration::from_secs(1800) {
            return Err(ExtensionError::Rejected(
                "deadline must be 1ns..1800s".into(),
            ));
        }
        let mut guard = CancelOnDrop(Some(cancellation.clone()));
        let client = self.clone();
        let task = tokio::spawn(async move {
            let _permit = permit;
            client
                .call_inner(call, expected, cancellation, handler)
                .await
        });
        let result = task
            .await
            .map_err(|e| ExtensionError::Unavailable(format!("supervisor failed: {e}")))?;
        guard.0 = None;
        result
    }

    async fn call_inner(
        &self,
        call: ExtensionCall,
        expected: ExtensionMetadata,
        cancellation: Cancellation,
        handler: Option<Arc<dyn CallbackHandler>>,
    ) -> Result<Value, ExtensionError> {
        if cancellation.is_cancelled() {
            return Err(ExtensionError::Cancelled);
        }
        let mut session = self.open().await?;
        if session.metadata != expected {
            session.close().await;
            return Err(ExtensionError::Protocol(
                "metadata changed since registration; re-register explicitly".into(),
            ));
        }
        if cancellation.is_cancelled() {
            session.close().await;
            return Err(ExtensionError::Cancelled);
        }
        let id = call_id();
        let message = Message::Call {
            id: id.clone(),
            contract: call.contract,
            version: call.version,
            method: call.method,
            params: call.params,
            timeout_ms: u64::try_from(call.timeout.as_millis())
                .unwrap_or(1800000)
                .max(1),
        };
        let result = async {
            send(&mut session.stdin, &message).await?;
            let deadline = tokio::time::sleep(call.timeout);
            tokio::pin!(deadline);
            let mut callback: Option<tokio::task::JoinHandle<(String, Result<Value, ExtensionError>)>> = None;
            let mut seen = std::collections::BTreeSet::new();
            let mut count = 0usize;
            let mut cancelled = false;
            let cancel_grace = tokio::time::sleep(Duration::from_secs(1802));
            tokio::pin!(cancel_grace);
            let outcome = loop {
                tokio::select! {
                    _ = cancellation.cancelled(), if !cancelled => {
                        cancelled = true;
                        cancel_grace.as_mut().reset(tokio::time::Instant::now() + GRACE);
                        if let Err(e) = send(&mut session.stdin, &Message::Cancel {id:id.clone()}).await { break Err(e); }
                    }
                    _ = &mut deadline, if !cancelled => {
                        cancellation.cancel();
                    }
                    _ = &mut cancel_grace, if cancelled => {
                        break Err(ExtensionError::Unknown {call_id:id.clone(), message:"cancel acknowledgement deadline elapsed".into()});
                    }
                    done = async { match &mut callback { Some(task) => task.await, None => std::future::pending().await } } => {
                        callback = None;
                        let (callback_id, result) = match done { Ok(v) => v, Err(e) => break Err(ExtensionError::Protocol(format!("trusted callback failed: {e}"))) };
                        let reply = match result { Ok(result) => Message::Result {id:callback_id,result}, Err(e) => Message::Error {id:callback_id,code:"tool_rejected".into(),message:e.to_string(),outcome:Outcome::Rejected} };
                        if let Err(e) = send(&mut session.stdin, &reply).await { break Err(e); }
                    }
                    message = session.incoming.recv() => {
                        count += 1;
                        if count > 130 { break Err(ExtensionError::Protocol("message count exceeded".into())); }
                        let message = match message { Some(Ok(m)) => m, Some(Err(e)) => break Err(e), None => break Err(ExtensionError::Unavailable("transport closed".into())) };
                        match message {
                            Message::Result {id:result_id,result} if result_id == id && callback.is_none() => { break Ok(result); }
                            Message::Error {id:result_id,code,message,outcome} if result_id == id && callback.is_none() => {
                                break Err(match outcome { Outcome::Rejected if seen.is_empty() => ExtensionError::Rejected(format!("{code}: {message}")), _ => ExtensionError::Unknown {call_id:id.clone(),message:format!("{code}: {message}")} });
                            }
                            Message::Callback {id:callback_id,parent_id,method,params} if parent_id == id && !cancelled && callback.is_none() && valid_id(&callback_id) && callback_id != id && seen.len() < 64 && seen.insert(callback_id.clone()) => {
                                let Some(handler) = handler.clone() else { break Err(ExtensionError::Protocol("unsolicited callback".into())); };
                                let token = cancellation.clone();
                                callback = Some(tokio::spawn(async move { (callback_id,handler.call(method,params,token).await) }));
                            }
                            _ => break Err(ExtensionError::Protocol("unexpected, duplicate or uncorrelated message".into())),
                        }
                    }
                }
            };
            if callback.is_some() { cancellation.cancel(); }
            if let Some(task) = callback { let _ = task.await; }
            outcome
        }.await;
        let exited = session.close().await;
        let result = if exited {
            result
        } else {
            Err(ExtensionError::Unknown {
                call_id: id.clone(),
                message: "transport did not exit during drain; executor shutdown unconfirmed"
                    .into(),
            })
        };
        result.map_err(|error| match error {
            ExtensionError::Rejected(_) | ExtensionError::Unknown { .. } => error,
            other => ExtensionError::Unknown {
                call_id: id,
                message: other.to_string(),
            },
        })
    }

    async fn open(&self) -> Result<Session, ExtensionError> {
        self.command.validate()?;
        let mut command = Command::new(&self.command.program);
        command
            .args(&self.command.args)
            .envs(&self.command.env)
            .current_dir(&self.command.cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        #[cfg(windows)]
        command.creation_flags(0x08000000);
        let mut child = command
            .spawn()
            .map_err(|e| ExtensionError::Unavailable(e.to_string()))?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| ExtensionError::Unavailable("missing stdin".into()))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| ExtensionError::Unavailable("missing stdout".into()))?;
        let (sender, incoming) = mpsc::channel(8);
        let reader = tokio::spawn(async move {
            let mut reader = BufReader::new(stdout);
            loop {
                let message = read_message(&mut reader).await;
                let failed = message.is_err();
                if sender.send(message).await.is_err() || failed {
                    break;
                }
            }
        });
        let mut session = Session {
            child,
            stdin: Some(stdin),
            incoming,
            reader,
            metadata: ExtensionMetadata {
                protocol_version: 0,
                id: String::new(),
                kind: self.kind,
                contracts: vec![],
                capabilities: vec![],
                workspaces: vec![],
            },
        };
        let handshake = tokio::time::timeout(Duration::from_secs(5), async {
            send(
                &mut session.stdin,
                &Message::Hello {
                    protocol_version: PROTOCOL_VERSION,
                    expected_id: self.id.clone(),
                    kind: self.kind,
                },
            )
            .await?;
            match session.incoming.recv().await {
                Some(Ok(Message::Ready { metadata }))
                    if metadata.protocol_version == PROTOCOL_VERSION
                        && metadata.id == self.id
                        && metadata.kind == self.kind =>
                {
                    Ok(metadata)
                }
                Some(Err(e)) => Err(e),
                _ => Err(ExtensionError::Protocol(
                    "identity, kind or protocol version mismatch".into(),
                )),
            }
        })
        .await;
        match handshake {
            Ok(Ok(metadata)) => {
                session.metadata = metadata;
                Ok(session)
            }
            error => {
                session.close().await;
                Err(match error {
                    Ok(Err(e)) => e,
                    _ => ExtensionError::Unavailable("handshake deadline elapsed".into()),
                })
            }
        }
    }
}

struct CancelOnDrop(Option<Cancellation>);
impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        if let Some(token) = &self.0 {
            token.cancel();
        }
    }
}
struct Session {
    child: Child,
    stdin: Option<ChildStdin>,
    incoming: mpsc::Receiver<Result<Message, ExtensionError>>,
    reader: tokio::task::JoinHandle<()>,
    metadata: ExtensionMetadata,
}
impl Session {
    async fn close(&mut self) -> bool {
        // Tokio pipe shutdown does not close the owned handle on all platforms.
        // Drop stdin to deliver actual EOF before waiting for the external process.
        drop(self.stdin.take());
        let exited = matches!(tokio::time::timeout(GRACE,self.child.wait()).await, Ok(Ok(status)) if status.success());
        if !exited {
            let _ = self.child.start_kill();
            let _ = self.child.wait().await;
        }
        self.reader.abort();
        let _ = (&mut self.reader).await;
        let mut clean = true;
        while let Ok(message) = self.incoming.try_recv() {
            if message.is_ok() {
                clean = false;
            }
        }
        exited && clean
    }
}
impl Drop for Session {
    fn drop(&mut self) {
        self.reader.abort();
        let _ = self.child.start_kill();
    }
}

pub async fn read_message<R: AsyncBufReadExt + Unpin>(
    reader: &mut R,
) -> Result<Message, ExtensionError> {
    let mut bytes = Vec::new();
    loop {
        let available = reader
            .fill_buf()
            .await
            .map_err(|e| ExtensionError::Unavailable(e.to_string()))?;
        if available.is_empty() {
            return Err(ExtensionError::Unavailable("unexpected EOF".into()));
        }
        let end = available.iter().position(|b| *b == b'\n').map(|n| n + 1);
        let length = end.unwrap_or(available.len());
        if bytes.len() + length > MAX_FRAME_BYTES {
            return Err(ExtensionError::Protocol("frame exceeds 1 MiB".into()));
        }
        bytes.extend_from_slice(&available[..length]);
        reader.consume(length);
        if end.is_some() {
            return serde_json::from_slice(&bytes)
                .map_err(|e| ExtensionError::Protocol(e.to_string()));
        }
    }
}

async fn send(stdin: &mut Option<ChildStdin>, message: &Message) -> Result<(), ExtensionError> {
    let stdin = stdin
        .as_mut()
        .ok_or_else(|| ExtensionError::Unavailable("transport stdin closed".into()))?;
    let mut bytes =
        serde_json::to_vec(message).map_err(|e| ExtensionError::Protocol(e.to_string()))?;
    if bytes.len() + 1 > MAX_FRAME_BYTES {
        return Err(ExtensionError::Rejected(
            "outgoing frame exceeds 1 MiB".into(),
        ));
    }
    bytes.push(b'\n');
    tokio::time::timeout(Duration::from_secs(5), async {
        stdin.write_all(&bytes).await?;
        stdin.flush().await
    })
    .await
    .map_err(|_| ExtensionError::Unavailable("write deadline elapsed".into()))?
    .map_err(|e| ExtensionError::Unavailable(e.to_string()))
}
pub fn valid_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}
pub fn call_id() -> String {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!(
        "{}-{now}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    )
}
