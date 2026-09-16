//! Explicit source-fault binding is provenance, never approval or automatic dispatch.
use super::{AUTH, call, config};
use crate::recovery::incidents::{
    IncidentKind, IncidentSignal, IncidentStore, IncidentStoreConfig, MonitorCommit,
    SignalCondition,
};
use crate::server::{Console, router, timestamp};
use serde_json::json;

#[cfg(windows)]
#[tokio::test]
async fn explicit_repair_source_is_scope_checked_durable_and_visible_from_both_details() {
    use crate::recovery::approval::{
        ApprovalDecision, ApprovalPolicy, ApprovalStore, ApprovalStoreConfig, ExecutionOutcome,
        ProposedOperation, ReviewerConfig,
    };
    use crate::recovery::workflow::{RepairConfig, RepairSession, now};
    let mut cfg = config("incident-repair-binding");
    let directory = cfg.data_dir.clone();
    cfg.permissions.extend([
        "monitor.read".into(),
        "incident.read".into(),
        "repair.run".into(),
    ]);
    for name in ["monitoring", "target", "review", "approval-state"] {
        std::fs::create_dir(directory.join(name)).unwrap();
    }
    std::fs::write(directory.join("target/example.txt"), "before").unwrap();
    let journal_path = directory.join("monitoring/incidents.jsonl");
    let mut incidents = IncidentStore::open(&journal_path, IncidentStoreConfig::default()).unwrap();
    for (monitor, target) in [
        ("monitor-a", "configured-target"),
        ("monitor-b", "other-target"),
    ] {
        incidents
            .commit(MonitorCommit {
                monitor_id: monitor.into(),
                sequence: 1,
                checkpoint: json!({}),
                now_ms: timestamp(),
                signals: vec![IncidentSignal {
                    monitor_id: monitor.into(),
                    target_id: target.into(),
                    rule_id: "health".into(),
                    kind: IncidentKind::Target,
                    condition: SignalCondition::Active,
                    summary: "Captured target error".into(),
                    evidence: json!({"untrusted":"must not enlarge target scope"}),
                }],
            })
            .unwrap();
    }
    let source = incidents
        .list()
        .into_iter()
        .find(|record| record.monitor_id == "monitor-a")
        .unwrap();
    let other = incidents
        .list()
        .into_iter()
        .find(|record| record.monitor_id == "monitor-b")
        .unwrap();
    drop(incidents);
    let monitors_path = directory.join("monitors.json");
    std::fs::write(&monitors_path, r#"{"schema_version":1,"monitors":[]}"#).unwrap();
    cfg.monitors_config = Some(monitors_path);
    let repair_config = RepairConfig {
        schema_version: 1,
        harness_config: directory.join("unused.json"),
        extensions_config: None,
        execution_workspace: None,
        reviewer_workspace: None,
        execution_harness: "unused".into(),
        target_id: "configured-target".into(),
        target_root: directory.join("target"),
        reviewer_directory: directory.join("review"),
        data_dir: directory.join("approval-state"),
        allowed_files: vec!["example.txt".into()],
        policy: ApprovalPolicy {
            id: "bound-policy".into(),
            version: 1,
            reviewer: ReviewerConfig::Human,
            delegation: "bounded fixture file only".into(),
            allowed_targets: vec!["configured-target".into()],
            allowed_action_kinds: vec!["replace_text".into()],
            ttl_secs: 600,
        },
        timeout_secs: 30,
        max_tool_calls: 4,
    };
    // A historical CLI/store task has no console operation receipt.
    {
        let mut approvals = ApprovalStore::open(
            &repair_config.data_dir,
            ApprovalStoreConfig::default(),
            now().unwrap(),
        )
        .unwrap();
        let operation = ProposedOperation {
            task_id: "cli-existing-task".into(),
            task_revision: 1,
            operation_id: "cli-old-operation".into(),
            target: "configured-target".into(),
            action: json!({"kind":"replace_text","path":"example.txt","expected":"before","replacement":"after"}),
        };
        let record = approvals
            .request(
                operation.clone(),
                repair_config.policy.clone(),
                now().unwrap(),
            )
            .unwrap();
        approvals
            .decide_human(
                &record.request.request_id,
                ApprovalDecision::Approve,
                "old fixture decision".into(),
                "fixture-cli".into(),
                &repair_config.policy,
                now().unwrap(),
            )
            .unwrap();
        let permit = approvals
            .consume(
                &record.request.request_id,
                &operation,
                &repair_config.policy,
                now().unwrap(),
            )
            .unwrap();
        approvals
            .complete(
                permit,
                ExecutionOutcome::Executed,
                "historical fixture execution fact".into(),
                now().unwrap(),
            )
            .unwrap();
    }
    let session = RepairSession::open(repair_config.clone(), None, &[]).unwrap();
    let (mut state, engine) = Console::open(cfg.clone()).await.unwrap();
    let unique = std::sync::Arc::get_mut(&mut state).unwrap();
    unique.repair = Some(session.clone());
    unique.repair_config = Some(repair_config.clone());
    let listener = tokio::net::TcpListener::bind(cfg.listen).await.unwrap();
    let address = listener.local_addr().unwrap();
    let app = router(state.clone());
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let request = |operation: &str, id: &str, revision: u64| {
        json!({"operation_id":operation,"task_id":operation,"prompt":"Diagnose captured fault within configured scope",
        "incident_id":id,"incident_revision":revision}).to_string()
    };
    let old_before = call(
        address,
        "GET",
        "/api/v1/repairs/cli-existing-task",
        AUTH,
        "",
    )
    .await
    .1;
    assert_eq!(old_before["status"], "completed");
    assert!(old_before["sourceIncident"].is_null());
    let collision = json!({"operation_id":"collision-source-operation","task_id":"cli-existing-task","prompt":"do not relabel an old repair",
        "incident_id":source.id,"incident_revision":source.revision}).to_string();
    assert_eq!(
        call(address, "POST", "/api/v1/repairs/runs", AUTH, &collision)
            .await
            .0,
        409
    );
    assert!(
        state.journal.lock().unwrap().records.is_empty(),
        "existing approval task must be rejected before accepting a source operation"
    );
    let old_after = call(
        address,
        "GET",
        "/api/v1/repairs/cli-existing-task",
        AUTH,
        "",
    )
    .await
    .1;
    assert_eq!(old_after["status"], "completed");
    assert!(
        old_after["sourceIncident"].is_null(),
        "a new diagnosis cannot attach its source to the completed historical task"
    );
    assert_eq!(
        call(
            address,
            "GET",
            &format!("/api/v1/incidents/{}", source.id),
            AUTH,
            ""
        )
        .await
        .1["related_repairs"]["total"],
        0
    );
    assert_eq!(
        call(
            address,
            "POST",
            "/api/v1/repairs/runs",
            AUTH,
            &request("stale-source", &source.id, source.revision + 1)
        )
        .await
        .0,
        409
    );
    assert_eq!(
        call(
            address,
            "POST",
            "/api/v1/repairs/runs",
            AUTH,
            &request("wrong-target", &other.id, other.revision)
        )
        .await
        .0,
        409
    );
    assert_eq!(
        call(
            address,
            "POST",
            "/api/v1/repairs/runs",
            AUTH,
            &request("missing-source", "missing-incident", 1)
        )
        .await
        .0,
        404
    );
    let partial=json!({"operation_id":"partial-source","task_id":"partial-source","prompt":"diagnose","incident_id":source.id}).to_string();
    assert_eq!(
        call(address, "POST", "/api/v1/repairs/runs", AUTH, &partial)
            .await
            .0,
        400
    );
    assert!(
        state.journal.lock().unwrap().records.is_empty(),
        "invalid fault binding must not persist or dispatch an operation"
    );
    let body = request("linked-task", &source.id, source.revision);
    assert_eq!(
        call(address, "POST", "/api/v1/repairs/runs", AUTH, &body)
            .await
            .0,
        202
    );
    let detail = call(address, "GET", "/api/v1/repairs/linked-task", AUTH, "")
        .await
        .1;
    assert_eq!(detail["sourceIncident"]["id"], source.id);
    assert_eq!(detail["sourceIncident"]["revision"], source.revision);
    assert_eq!(detail["target"], "configured-target");
    assert!(
        detail["sourceIncident"].get("evidence").is_none(),
        "link summary cannot promote arbitrary fault payloads into authority"
    );
    let reverse = call(
        address,
        "GET",
        &format!("/api/v1/incidents/{}", source.id),
        AUTH,
        "",
    )
    .await
    .1;
    assert_eq!(reverse["related_repairs"]["total"], 1);
    assert_eq!(reverse["related_repairs"]["items"][0]["id"], "linked-task");
    let duplicate=json!({"operation_id":"different-operation","task_id":"linked-task","prompt":"do not replay","incident_id":source.id,"incident_revision":source.revision}).to_string();
    assert_eq!(
        call(address, "POST", "/api/v1/repairs/runs", AUTH, &duplicate)
            .await
            .0,
        409
    );
    assert_eq!(
        session.records().unwrap().len(),
        1,
        "source linkage must not create approvals"
    );
    assert_eq!(
        std::fs::read_to_string(directory.join("target/example.txt")).unwrap(),
        "before"
    );
    server.abort();
    let _ = server.await;
    state.shutdown().await.unwrap();
    engine.shutdown().await.unwrap();
    drop(state);
    drop(session);

    // The fixture has no execution registry; provenance survives its failed run and restart.
    let (state, engine) = Console::open(cfg.clone()).await.unwrap();
    let persistent = crate::server::history::repair_detail(&state, "linked-task").unwrap();
    assert_eq!(persistent["sourceIncident"]["id"], source.id);
    assert_eq!(persistent["sourceIncident"]["revision"], source.revision);
    assert_eq!(
        persistent["target"], "configured-target",
        "historical scope must come from its accepted record"
    );
    assert_eq!(
        crate::server::history::incident_repairs(&state, &source.id).unwrap()["total"],
        1
    );
    state.shutdown().await.unwrap();
    engine.shutdown().await.unwrap();
    drop(state);

    cfg.permissions
        .retain(|permission| permission != "incident.read");
    let session = RepairSession::open(repair_config.clone(), None, &[]).unwrap();
    let (mut state, engine) = Console::open(cfg.clone()).await.unwrap();
    let unique = std::sync::Arc::get_mut(&mut state).unwrap();
    unique.repair = Some(session.clone());
    unique.repair_config = Some(repair_config);
    let listener = tokio::net::TcpListener::bind(cfg.listen).await.unwrap();
    let address = listener.local_addr().unwrap();
    let app = router(state.clone());
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    assert_eq!(
        call(
            address,
            "POST",
            "/api/v1/repairs/runs",
            AUTH,
            &request("no-read-permission", &source.id, source.revision)
        )
        .await
        .0,
        403
    );
    server.abort();
    let _ = server.await;
    state.shutdown().await.unwrap();
    engine.shutdown().await.unwrap();
    drop(state);
    drop(session);
    assert_eq!(
        directory.parent(),
        Some(
            std::path::PathBuf::from(std::env::var_os("RECUVORA_TEST_TEMP").unwrap())
                .canonicalize()
                .unwrap()
                .as_path()
        )
    );
    std::fs::remove_dir_all(directory).unwrap();
}
