use super::*;
use crate::recovery::approval::{ApprovalRecord, ApprovalState, AssessmentSource, ReviewerConfig};

pub(super) fn bootstrap(state: &Console) -> Result<Value, ApiError> {
    let approvals = history::approval_page(
        state,
        &history::ListQuery {
            state: Some("attention".into()),
            ..Default::default()
        },
    )?;
    let repairs = history::repair_page(state, &history::ListQuery::default())?;
    let operations = history::operation_page(state, &history::ListQuery::default())?;
    let simulations = history::simulation_page(state, &history::ListQuery::default())?;
    let extension_statuses = state
        .extensions
        .as_ref()
        .map(|registry| registry.statuses())
        .unwrap_or_default();
    let harnesses=state.registry.as_ref().map(|registry|registry.definitions().iter().map(|h| {
        let node_status=h.address.strip_prefix("node://").and_then(|id|extension_statuses.iter().find(|status|status.id==id));
        json!({
        "id":h.id,"adapter":h.adapter,"address":h.address,"enabled":h.enabled,"isDefault":registry.default_harness()==Some(&h.id),
        "availability":if !h.enabled {"disabled"} else if node_status.is_some_and(|status|!status.available) {"unavailable"} else {"configured"},"authentication":"not_checked","runtimeStatus":"not_probed",
        "reason":node_status.and_then(|status|status.error.as_ref()),
        "workspaceRoots":h.workspace_roots,"projects":null})}).collect::<Vec<_>>()).unwrap_or_default();
    let capability = |id: &str,
                      name: &str,
                      available: bool,
                      actions: Vec<&str>,
                      limitation: &str| {
        json!({"id":id,"name":name,"group":"运行时能力",
        "state":match id { "harness" | "text-repair" | "external-plugins" => "limited", "simulation" => "prototype", _ => "implemented" },"availability":if available {"available"} else {"unconfigured"},
        "actions":actions,"summary":name,"limitation":limitation,"availableNow":if available {"已配置"} else {"未配置"}})
    };
    let caps = vec![
        capability(
            "monitoring",
            "监控与故障",
            state.monitoring.is_some(),
            vec!["monitor.read", "incident.read", "incident.acknowledge"],
            "规则判定、观测覆盖与持久故障；确认收到不解除故障，不自动执行修复。",
        ),
        capability(
            "harness",
            "Harness 会话",
            state.registry.is_some(),
            vec!["harness.run", "harness.projects"],
            "提供方登录状态按调用结果确认，客户端归组另行核验。",
        ),
        capability(
            "text-repair",
            "审批与文本修复",
            state.repair.is_some(),
            vec![
                "repair.run",
                "approval.decide",
                "approval.apply",
                "approval.reconcile",
            ],
            "真实动作仅为 Windows 白名单文本替换；读回一致不等于业务恢复。",
        ),
        capability(
            "simulation",
            "模拟实验室",
            true,
            vec!["simulation.run"],
            "封闭模拟，不调用真实目标。",
        ),
        capability(
            "logs",
            "运行记录",
            true,
            vec!["logs.read"],
            "宿主持久操作记录与按可信配置绑定的观测记录；记录原文范围由只读提供方决定。",
        ),
        capability(
            "external-plugins",
            "外部节点与契约插件",
            extension_statuses.iter().any(|status| status.available),
            vec!["extension.read"],
            "只允许已登记的契约与方法；新领域写动作不因注册而获权。",
        ),
    ];
    let active=state.repair_config.as_ref().map(|c|json!({"name":"当前服务配置","repairConfig":state.config.repair_config,"harnessConfig":c.harness_config,
        "targetId":c.target_id,"targetRoot":c.target_root,"reviewerDirectory":c.reviewer_directory,"dataDirectory":c.data_dir,"allowedFiles":c.allowed_files,
        "executionHarness":c.execution_harness,"executionWorkspace":c.execution_workspace,"reviewerWorkspace":c.reviewer_workspace,
        "reviewer":reviewer(&c.policy.reviewer),"delegation":c.policy.delegation,"policyId":c.policy.id,"policyVersion":c.policy.version,
        "ttlSeconds":c.policy.ttl_secs,"timeoutSeconds":c.timeout_secs,"maxToolCalls":c.max_tool_calls}));
    Ok(
        json!({"schema_version":1,"mode":"live","runtime":{"status":"connected","updatedAt":timestamp(),"operator":state.config.operator,"permissions":state.config.permissions},
        "capabilities":caps,"harnesses":harnesses,"repairs":repairs["items"],"approvals":approvals["items"],"simulation_tasks":simulations["items"],
        "logs":[],"active_configuration":active,"operations":operations["items"],"extension_statuses":extension_statuses,"monitoring":monitoring::bootstrap(state)?,
        "pages":{"repairs":history::metadata(&repairs),"approvals":history::metadata(&approvals),"operations":history::metadata(&operations),"simulation_tasks":history::metadata(&simulations)},"approval_counts":history::approval_counts(state)?}),
    )
}

fn reviewer(value: &ReviewerConfig) -> String {
    match value {
        ReviewerConfig::Human => "人工".into(),
        ReviewerConfig::Harness { harness_id } => format!("Harness · {harness_id}"),
    }
}
pub(super) fn approval(state: &Console, r: &ApprovalRecord) -> Value {
    let operation = &r.request.operation;
    let action = &operation.action;
    let policy = &r.request.policy;
    let execution_context = &action["execution_context"];
    let receipt = r
        .note
        .as_ref()
        .and_then(|note| serde_json::from_str::<Value>(note).ok())
        .map(|raw| {
            json!({
        "contentVerified":raw.get("content_verified").and_then(Value::as_bool)==Some(true),
        "businessVerified":false,"raw":raw})
        });
    let mut actions = Vec::new();
    let unexpired = r.request.expires_at > timestamp() / 1000;
    if state.require("approval.decide").is_ok() {
        if unexpired
            && matches!(
                r.state,
                ApprovalState::Pending | ApprovalState::WaitingHuman
            )
        {
            actions.extend(["approve", "deny"]);
        }
        if matches!(
            r.state,
            ApprovalState::Pending | ApprovalState::WaitingHuman | ApprovalState::Approved
        ) {
            actions.push("revoke");
        }
    }
    if state.require("approval.apply").is_ok() && r.state == ApprovalState::Approved && unexpired {
        actions.push("apply");
    }
    if state.require("approval.reconcile").is_ok() && r.state == ApprovalState::Unknown {
        actions.push("reconcile");
    }
    let assessment=r.assessment.as_ref().map(|a|{let (source,name,session)=match &a.reviewer {
        AssessmentSource::Human{actor}=>("human",actor.clone(),None),
        AssessmentSource::Harness{harness_id,session_id}=>("harness",harness_id.clone(),Some(session_id.clone())),
    };json!({"decision":a.decision,"reason":a.reason,"source":source,"reviewer":name,"sessionId":session})});
    json!({"requestId":r.request.request_id,"state":r.state,"revision":r.revision,"createdAt":r.request.created_at*1000,"updatedAt":r.updated_at*1000,"expiresAt":r.request.expires_at*1000,
        "taskId":operation.task_id,"taskRevision":operation.task_revision,"operationId":operation.operation_id,"target":operation.target,
        "actionKind":action["kind"],"targetRoot":action["target_root"],"path":action["path"],"expected":action["expected"],"replacement":action["replacement"],
        "userRequest":action["user_request"],"assessment":assessment,"note":r.note,"allowedActions":actions,"normalizedOperation":operation,
        "policy":{"id":policy.id,"version":policy.version,"reviewer":reviewer(&policy.reviewer),"delegation":policy.delegation,
            "allowedTargets":policy.allowed_targets,"allowedActions":policy.allowed_action_kinds,"ttlSeconds":policy.ttl_secs},
        "executionContext":{"harnessId":execution_context["harness_id"],"threadId":execution_context["thread_id"],"turnId":execution_context["turn_id"],"callId":execution_context["call_id"]},
        "receipt":receipt})
}
