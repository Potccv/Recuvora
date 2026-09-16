//! Bounded CLI syntax for the trusted approval workflow.
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::PathBuf;

/// The shared CLI/HTTP representation of typed workflow facts.
pub(crate) fn result_json(result: &crate::recovery::workflow::RepairResult) -> serde_json::Value {
    use crate::recovery::workflow::RepairStatus;
    use serde_json::json;
    let status = match result.status {
        RepairStatus::Unknown => "unknown",
        RepairStatus::WaitingHuman => "waiting_human",
        RepairStatus::Blocked => "blocked",
        RepairStatus::Canceled => "canceled",
        RepairStatus::Failed => "failed",
        RepairStatus::Completed => "completed",
    };
    let context = result.execution_context.as_ref().map(|context| {
        json!({"harness_id":context.harness_id,"thread_id":context.thread_id,"session_id":context.session_id})
    });
    json!({
        "status":status,"task_id":result.task_id,"final_response":result.final_response,
        "harness_error":result.harness_error.as_ref().map(super::error_json),
        "operations":result.operations,"execution_context":context,
        "business_verified":false,"auto_retry":false
    })
}

pub(crate) const HELP: &str = "Approval-controlled repair (JSON output):
  recuvora repair run --config PATH --task ID --prompt TEXT
  recuvora repair inspect --config PATH [--request ID]
  recuvora repair approve --config PATH --request ID --reason TEXT
  recuvora repair deny --config PATH --request ID --reason TEXT
  recuvora repair revoke --config PATH --request ID --reason TEXT
  recuvora repair apply --config PATH --request ID
  recuvora repair reconcile --config PATH --request ID

Use an external repair configuration with an explicit target, file allowlist,
delegated approval policy, and stable state directory. Run uses hidden execution
and approval conversations. Approve records a decision; apply executes that exact
stored operation once. Reconcile checks current content and never repeats writes.
Only existing UTF-8 text files up to 16 KiB are supported, on native Windows.
No shell, publishing, desktop operation, or business-health verification.
";

pub(crate) struct Arguments {
    pub command: String,
    pub config: PathBuf,
    pub task: Option<String>,
    pub prompt: Option<String>,
    pub request: Option<String>,
    pub reason: Option<String>,
}

pub(crate) fn parse(args: impl IntoIterator<Item = OsString>) -> Result<Option<Arguments>, String> {
    let args: Vec<_> = args.into_iter().take(20).collect();
    if args.is_empty() || (args.len() == 1 && args[0] == "--help") {
        return Ok(None);
    }
    let command = args[0].to_str().ok_or("command must be UTF-8")?.to_owned();
    if !matches!(
        command.as_str(),
        "run" | "inspect" | "approve" | "deny" | "revoke" | "apply" | "reconcile"
    ) || args.len() % 2 != 1
    {
        return Err("invalid repair command or option pairs; see repair --help".into());
    }
    let mut options = BTreeMap::new();
    for pair in args[1..].as_chunks::<2>().0 {
        let key = pair[0].to_str().ok_or("option must be UTF-8")?.to_owned();
        if options.insert(key, pair[1].clone()).is_some() {
            return Err("duplicate option".into());
        }
    }
    let config = PathBuf::from(options.remove("--config").ok_or("--config required")?);
    let mut text = |flag: &str, required: bool, limit: usize| -> Result<Option<String>, String> {
        let value = options
            .remove(flag)
            .map(|v| v.into_string().map_err(|_| format!("{flag} must be UTF-8")))
            .transpose()?;
        if required && value.is_none() {
            return Err(format!("{flag} required"));
        }
        if value
            .as_ref()
            .is_some_and(|v| v.trim().is_empty() || v.len() > limit || v.contains('\0'))
        {
            return Err(format!(
                "{flag} must contain 1..{limit} nonempty bytes without NUL"
            ));
        }
        Ok(value)
    };
    let task = if command == "run" {
        text("--task", true, 128)?
    } else {
        None
    };
    let prompt = if command == "run" {
        text("--prompt", true, 8192)?
    } else {
        None
    };
    let request = if command != "run" {
        text("--request", command != "inspect", 512)?
    } else {
        None
    };
    let reason = if matches!(command.as_str(), "approve" | "deny" | "revoke") {
        text("--reason", true, 8192)?
    } else {
        None
    };
    if !options.is_empty() {
        return Err(format!(
            "unsupported option(s): {:?}",
            options.keys().collect::<Vec<_>>()
        ));
    }
    Ok(Some(Arguments {
        command,
        config,
        task,
        prompt,
        request,
        reason,
    }))
}
