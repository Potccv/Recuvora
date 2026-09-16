use recuvora::recovery::incidents::{
    IncidentError, IncidentKind, IncidentSignal, IncidentStatus, IncidentStore,
    IncidentStoreConfig, MonitorCommit, SignalCondition,
};
use serde_json::json;
use std::{fs::OpenOptions, io::Write, path::Path};

#[path = "workflow_support.rs"]
mod support;
use support::TestDir;

fn commit(sequence: u64, condition: SignalCondition) -> MonitorCommit {
    MonitorCommit {
        monitor_id: "monitor-a".into(),
        sequence,
        checkpoint: json!({"source_id":"source-a", "generation":"generation-1", "cursor":sequence}),
        signals: vec![IncidentSignal {
            monitor_id: "monitor-a".into(),
            target_id: "target-a".into(),
            rule_id: "rule-a".into(),
            kind: IncidentKind::Target,
            condition,
            summary: "condition active".into(),
            evidence: json!({"source_id":"source-a", "generation":"generation-1", "sequence":sequence, "message":"condition active", "observation_id":format!("row-{sequence}")}),
        }],
        now_ms: sequence * 1000,
    }
}

fn open(path: &Path) -> IncidentStore {
    IncidentStore::open(path, IncidentStoreConfig::default()).unwrap()
}

#[test]
fn restart_preserves_atomic_checkpoint_and_merged_episode() {
    let dir = TestDir::new("incidents-restart");
    let path = dir.path.join("nested/incidents.jsonl");
    let mut store = open(&path);
    store.commit(commit(1, SignalCondition::Active)).unwrap();
    let id = store.list()[0].id.clone();
    store.commit(commit(2, SignalCondition::Active)).unwrap();
    let expected = store.get(&id).unwrap();
    assert_eq!(expected.revision, 2);
    assert_eq!(expected.occurrences, 2);
    assert_eq!(expected.first_seen, 1000);
    assert_eq!(expected.last_seen, 2000);
    let checkpoint = store.checkpoint("monitor-a").unwrap();
    assert_eq!(checkpoint.sequence, 2);
    assert_eq!(checkpoint.value["cursor"], 2);
    drop(store);
    let restarted = open(&path);
    assert_eq!(restarted.get(&id).unwrap(), expected);
    assert_eq!(restarted.checkpoint("monitor-a").unwrap(), checkpoint);
    assert_eq!(restarted.list().len(), 1);
}

#[test]
fn latest_identical_retry_does_not_write_or_overwrite_acknowledgement() {
    let dir = TestDir::new("incidents-retry");
    let path = dir.path.join("incidents.jsonl");
    let mut store = open(&path);
    let event = commit(1, SignalCondition::Active);
    store.commit(event.clone()).unwrap();
    let original = store.list()[0].clone();
    let acknowledged = store
        .acknowledge(&original.id, 1, "operator", "investigating", 1001)
        .unwrap();
    let length = std::fs::metadata(&path).unwrap().len();
    store.commit(event.clone()).unwrap();
    assert_eq!(std::fs::metadata(&path).unwrap().len(), length);
    assert_eq!(store.get(&original.id).unwrap(), acknowledged);
    let mut conflict = event;
    conflict.checkpoint["cursor"] = json!(999);
    assert!(matches!(
        store.commit(conflict),
        Err(IncidentError::Conflict(_))
    ));
    drop(store);
    let mut restarted = open(&path);
    restarted
        .commit(commit(1, SignalCondition::Active))
        .unwrap();
    assert_eq!(restarted.get(&original.id).unwrap(), acknowledged);
    assert_eq!(std::fs::metadata(&path).unwrap().len(), length);
}

#[test]
fn unknown_is_not_clear_and_clear_starts_a_new_later_episode() {
    let dir = TestDir::new("incidents-unknown");
    let path = dir.path.join("incidents.jsonl");
    let mut store = open(&path);
    store.commit(commit(1, SignalCondition::Unknown)).unwrap();
    store.commit(commit(2, SignalCondition::Clear)).unwrap();
    assert!(store.list().is_empty());
    store.commit(commit(3, SignalCondition::Active)).unwrap();
    let id = store.list()[0].id.clone();
    store.acknowledge(&id, 1, "operator", "", 3001).unwrap();
    store.commit(commit(4, SignalCondition::Unknown)).unwrap();
    let unknown = store.get(&id).unwrap();
    assert_eq!(unknown.status, IncidentStatus::Acknowledged);
    assert_eq!(unknown.condition, SignalCondition::Unknown);
    assert_eq!(unknown.occurrences, 1);
    assert_eq!(unknown.resolved_at, None);
    store.commit(commit(5, SignalCondition::Clear)).unwrap();
    let resolved = store.get(&id).unwrap();
    assert_eq!(resolved.status, IncidentStatus::Resolved);
    assert_eq!(resolved.resolved_at, Some(5000));
    assert!(resolved.acknowledgement.is_some());
    store.commit(commit(6, SignalCondition::Active)).unwrap();
    assert_eq!(store.list().len(), 2);
    let next = store
        .list()
        .into_iter()
        .find(|record| record.id != id)
        .unwrap();
    assert_eq!(next.status, IncidentStatus::Open);
    assert_eq!(next.first_seen, 6000);
    assert_eq!(next.occurrences, 1);
    assert_eq!(next.acknowledgement, None);
    drop(store);
    let restarted = open(&path);
    assert_eq!(restarted.get(&id).unwrap(), resolved);
    assert_eq!(restarted.get(&next.id).unwrap(), next);
}

#[test]
fn acknowledgement_requires_current_revision_and_never_resolves() {
    let dir = TestDir::new("incidents-ack");
    let mut store = open(&dir.path.join("incidents.jsonl"));
    store.commit(commit(1, SignalCondition::Active)).unwrap();
    let id = store.list()[0].id.clone();
    store.commit(commit(2, SignalCondition::Active)).unwrap();
    assert!(matches!(
        store.acknowledge(&id, 1, "operator", "seen", 2001),
        Err(IncidentError::Conflict(_))
    ));
    let ack = store
        .acknowledge(&id, 2, "operator", "investigating", 2001)
        .unwrap();
    assert_eq!(ack.status, IncidentStatus::Acknowledged);
    assert_eq!(ack.condition, SignalCondition::Active);
    assert_eq!(ack.resolved_at, None);
    assert_eq!(ack.last_seen, 2000);
    assert_eq!(ack.acknowledgement.as_ref().unwrap().actor, "operator");
    assert!(matches!(
        store.acknowledge(&id, 3, "another", "seen", 2002),
        Err(IncidentError::Conflict(_))
    ));
    assert_eq!(store.get(&id).unwrap(), ack);
}

#[test]
fn monitor_sequences_and_signal_identity_cannot_skip_or_mix() {
    let dir = TestDir::new("incidents-sequence");
    let mut store = open(&dir.path.join("incidents.jsonl"));
    assert!(matches!(
        store.commit(commit(2, SignalCondition::Active)),
        Err(IncidentError::Conflict(_))
    ));
    let mut bad = commit(1, SignalCondition::Active);
    bad.signals[0].monitor_id = "another-monitor".into();
    assert!(matches!(store.commit(bad), Err(IncidentError::Invalid(_))));
    assert_eq!(store.checkpoint("monitor-a"), None);
    assert!(store.list().is_empty());
    store.commit(commit(1, SignalCondition::Active)).unwrap();
    assert!(matches!(
        store.commit(commit(3, SignalCondition::Active)),
        Err(IncidentError::Conflict(_))
    ));
    store.commit(commit(2, SignalCondition::Active)).unwrap();
    assert!(matches!(
        store.commit(commit(1, SignalCondition::Active)),
        Err(IncidentError::Conflict(_))
    ));
}

#[test]
fn target_and_coverage_are_independent_keys() {
    let dir = TestDir::new("incidents-coverage");
    let mut store = open(&dir.path.join("incidents.jsonl"));
    let mut event = commit(1, SignalCondition::Active);
    let mut coverage = event.signals[0].clone();
    coverage.kind = IncidentKind::Coverage;
    coverage.summary = "source unreachable".into();
    event.signals.push(coverage);
    store.commit(event).unwrap();
    assert_eq!(store.list().len(), 2);
    let mut clear_coverage = commit(2, SignalCondition::Clear);
    clear_coverage.signals[0].kind = IncidentKind::Coverage;
    store.commit(clear_coverage).unwrap();
    assert_eq!(
        store
            .list()
            .iter()
            .filter(|record| record.status == IncidentStatus::Resolved)
            .count(),
        1
    );
    assert!(
        store
            .list()
            .iter()
            .any(|record| record.kind == IncidentKind::Target
                && record.status == IncidentStatus::Open)
    );
}

#[test]
fn ordered_signals_preserve_fault_then_clear_and_recurrence_within_one_batch() {
    let dir = TestDir::new("incidents-batch-transitions");
    let path = dir.path.join("incidents.jsonl");
    let mut store = open(&path);
    let mut event = commit(1, SignalCondition::Active);
    let mut clear = event.signals[0].clone();
    clear.condition = SignalCondition::Clear;
    clear.evidence = json!({"observation_id":"row-2", "message":"condition clear"});
    let mut recurrence = event.signals[0].clone();
    recurrence.evidence = json!({"observation_id":"row-3", "message":"condition active again"});
    event.signals.extend([clear, recurrence]);
    store.commit(event).unwrap();
    let records = store.list();
    assert_eq!(records.len(), 2);
    assert_eq!(records[0].status, IncidentStatus::Resolved);
    assert_eq!(records[0].revision, 2);
    assert_eq!(records[0].occurrences, 1);
    assert_eq!(records[0].evidence["observation_id"], "row-2");
    assert_eq!(records[1].status, IncidentStatus::Open);
    assert_eq!(records[1].revision, 1);
    assert_eq!(records[1].evidence["observation_id"], "row-3");
    assert_eq!(store.checkpoint("monitor-a").unwrap().sequence, 1);
    drop(store);
    assert_eq!(std::fs::read_to_string(&path).unwrap().lines().count(), 1);
    assert_eq!(open(&path).list(), records);
}

#[test]
fn wall_clock_rollback_uses_durable_sequence_and_monotonic_display_times() {
    let dir = TestDir::new("incidents-clock-rollback");
    let path = dir.path.join("incidents.jsonl");
    let mut store = open(&path);
    store.commit(commit(1, SignalCondition::Active)).unwrap();
    let id = store.list()[0].id.clone();
    let acknowledged = store
        .acknowledge(&id, 1, "operator", "clock adjusted", 2)
        .unwrap();
    assert_eq!(acknowledged.acknowledgement.unwrap().at_ms, 1000);
    let mut rollback = commit(2, SignalCondition::Clear);
    rollback.now_ms = 3;
    store.commit(rollback.clone()).unwrap();
    assert_eq!(store.get(&id).unwrap().resolved_at, Some(1000));
    assert_eq!(store.checkpoint("monitor-a").unwrap().updated_at_ms, 1000);
    drop(store);
    let mut restarted = open(&path);
    restarted.commit(rollback).unwrap();
    let mut later = commit(3, SignalCondition::Active);
    later.now_ms = 4;
    restarted.commit(later).unwrap();
    assert_eq!(
        restarted.checkpoint("monitor-a").unwrap().updated_at_ms,
        1000
    );
    assert!(
        restarted
            .list()
            .iter()
            .all(|record| record.first_seen == 1000 && record.last_seen == 1000)
    );
}

#[test]
fn capacity_rejection_preserves_whole_checkpoint_and_batch() {
    let dir = TestDir::new("incidents-capacity");
    let path = dir.path.join("incidents.jsonl");
    let config = IncidentStoreConfig {
        max_incidents: 1,
        max_monitors: 1,
        ..Default::default()
    };
    let mut store = IncidentStore::open(&path, config).unwrap();
    store.commit(commit(1, SignalCondition::Active)).unwrap();
    let original = store.list()[0].clone();
    let checkpoint = store.checkpoint("monitor-a");
    let length = std::fs::metadata(&path).unwrap().len();
    let mut over = commit(2, SignalCondition::Clear);
    let mut new_signal = over.signals[0].clone();
    new_signal.rule_id = "another-rule".into();
    new_signal.condition = SignalCondition::Active;
    over.signals.push(new_signal);
    assert!(matches!(
        store.commit(over),
        Err(IncidentError::Capacity(_))
    ));
    assert_eq!(store.get(&original.id).unwrap(), original);
    assert_eq!(store.checkpoint("monitor-a"), checkpoint);
    assert_eq!(std::fs::metadata(&path).unwrap().len(), length);
    let mut other = commit(1, SignalCondition::Active);
    other.monitor_id = "second-monitor".into();
    other.signals[0].monitor_id.clone_from(&other.monitor_id);
    assert!(matches!(
        store.commit(other),
        Err(IncidentError::Capacity(_))
    ));
    // History counts toward capacity; resolution does not silently delete it.
    store.commit(commit(2, SignalCondition::Clear)).unwrap();
    assert!(matches!(
        store.commit(commit(3, SignalCondition::Active)),
        Err(IncidentError::Capacity(_))
    ));
}

#[test]
fn journal_and_evidence_limits_reject_before_any_cursor_advance() {
    let dir = TestDir::new("incidents-byte-limits");
    let path = dir.path.join("small.jsonl");
    let config = IncidentStoreConfig {
        max_journal_bytes: 1,
        ..Default::default()
    };
    let mut limited = IncidentStore::open(&path, config).unwrap();
    assert!(matches!(
        limited.commit(commit(1, SignalCondition::Active)),
        Err(IncidentError::Capacity(_))
    ));
    assert_eq!(limited.checkpoint("monitor-a"), None);
    assert_eq!(std::fs::metadata(&path).unwrap().len(), 0);
    let mut store = open(&dir.path.join("incidents.jsonl"));
    let mut over = commit(1, SignalCondition::Active);
    over.signals[0].evidence = json!({"message":"x".repeat(16_384)});
    assert!(matches!(
        store.commit(over),
        Err(IncidentError::Capacity(_))
    ));
    let mut non_object = commit(1, SignalCondition::Active);
    non_object.checkpoint = json!([]);
    assert!(matches!(
        store.commit(non_object),
        Err(IncidentError::Invalid(_))
    ));
    assert!(store.list().is_empty());
    assert_eq!(store.checkpoint("monitor-a"), None);
}

#[test]
fn malformed_and_partial_records_fail_closed_without_truncating() {
    let dir = TestDir::new("incidents-corrupt");
    for (name, suffix) in [
        ("partial", b"{\"format\":1".as_slice()),
        ("complete", b"{bad}\n".as_slice()),
    ] {
        let path = dir.path.join(format!("{name}.jsonl"));
        let mut store = open(&path);
        store.commit(commit(1, SignalCondition::Active)).unwrap();
        drop(store);
        OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap()
            .write_all(suffix)
            .unwrap();
        let original = std::fs::read(&path).unwrap();
        assert!(matches!(
            IncidentStore::open(&path, Default::default()),
            Err(IncidentError::Corrupt(_))
        ));
        assert_eq!(std::fs::read(&path).unwrap(), original);
    }
}

#[test]
fn replay_rejects_semantic_corruption_using_live_rules() {
    let dir = TestDir::new("incidents-replay");
    let original_path = dir.path.join("original.jsonl");
    let mut store = open(&original_path);
    store.commit(commit(1, SignalCondition::Active)).unwrap();
    drop(store);
    let entry: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&original_path).unwrap()).unwrap();
    for (index, change) in [
        "sequence",
        "monitor_sequence",
        "signal_monitor",
        "unknown_field",
        "version",
    ]
    .iter()
    .enumerate()
    {
        let mut mutated = entry.clone();
        match *change {
            "sequence" => mutated["sequence"] = json!(2),
            "monitor_sequence" => mutated["event"]["commit"]["sequence"] = json!(2),
            "signal_monitor" => {
                mutated["event"]["commit"]["signals"][0]["monitor_id"] = json!("different")
            }
            "unknown_field" => mutated["event"]["commit"]["unrecognized"] = json!(true),
            "version" => mutated["format"] = json!(2),
            _ => unreachable!(),
        }
        let path = dir.path.join(format!("corrupt-{index}.jsonl"));
        std::fs::write(&path, format!("{mutated}\n")).unwrap();
        assert!(
            matches!(
                IncidentStore::open(&path, Default::default()),
                Err(IncidentError::Corrupt(_))
            ),
            "{change}"
        );
    }
}

#[test]
fn one_writer_close_releases_lock_and_retains_only_read_access() {
    let dir = TestDir::new("incidents-close");
    let path = dir.path.join("incidents.jsonl");
    let mut first = open(&path);
    first.commit(commit(1, SignalCondition::Active)).unwrap();
    let original = first.list()[0].clone();
    assert!(IncidentStore::open(&path, Default::default()).is_err());
    first.close().unwrap();
    let second = open(&path);
    assert_eq!(second.get(&original.id).unwrap(), original);
    assert_eq!(first.get(&original.id).unwrap(), original);
    assert!(matches!(
        first.commit(commit(2, SignalCondition::Active)),
        Err(IncidentError::Unavailable(_))
    ));
    assert!(matches!(
        first.acknowledge(&original.id, 1, "operator", "seen", 1001),
        Err(IncidentError::Unavailable(_))
    ));
    assert_eq!(
        first.map_records(|record| record.id.clone()),
        vec![original.id]
    );
}

#[test]
fn source_paths_and_hard_link_journals_are_rejected() {
    assert!(matches!(
        IncidentStore::open("relative.jsonl", Default::default()),
        Err(IncidentError::Invalid(_))
    ));
    let source =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("must-not-create-incident-data/incidents.jsonl");
    assert!(matches!(
        IncidentStore::open(&source, Default::default()),
        Err(IncidentError::Invalid(_))
    ));
    assert!(!source.parent().unwrap().exists());
    let dir = TestDir::new("incidents-links");
    let original = dir.path.join("original.jsonl");
    let alias = dir.path.join("alias.jsonl");
    std::fs::write(&original, "").unwrap();
    std::fs::hard_link(&original, &alias).unwrap();
    assert!(matches!(
        IncidentStore::open(&alias, Default::default()),
        Err(IncidentError::Invalid(_))
    ));
}
