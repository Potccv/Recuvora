//! Private storage fault tests, compiled only into the library test build.
use super::*;
use crate::recovery::workflow_test_support::TestDir;
use serde_json::json;

fn observation(sequence: u64) -> MonitorCommit {
    MonitorCommit {
        monitor_id: "monitor".into(),
        sequence,
        checkpoint: json!({"cursor":sequence}),
        now_ms: sequence,
        signals: vec![IncidentSignal {
            monitor_id: "monitor".into(),
            target_id: "target".into(),
            rule_id: "rule".into(),
            kind: IncidentKind::Target,
            condition: SignalCondition::Active,
            summary: "failure".into(),
            evidence: json!({"value":sequence}),
        }],
    }
}

#[test]
fn failed_file_write_preserves_checkpoint_and_incident_then_disables_mutation() {
    let dir = TestDir::new("incident-write-failure");
    let path = dir.path.join("incidents.jsonl");
    let mut store = IncidentStore::open(&path, Default::default()).unwrap();
    store.commit(observation(1)).unwrap();
    let checkpoint = store.checkpoint("monitor");
    let record = store.list()[0].clone();
    // A real read-only file handle fails write_all without a synthetic product
    // failure flag. This models an OS write denial before any complete append.
    store.journal = Some(File::open(&path).unwrap());
    assert!(matches!(
        store.commit(observation(2)),
        Err(IncidentError::Io(_))
    ));
    assert_eq!(store.checkpoint("monitor"), checkpoint);
    assert_eq!(store.list(), vec![record.clone()]);
    assert!(matches!(
        store.commit(observation(2)),
        Err(IncidentError::Unavailable(_))
    ));
    assert!(matches!(
        store.acknowledge(&record.id, 1, "operator", "", 3),
        Err(IncidentError::Unavailable(_))
    ));
    drop(store);
    let reopened = IncidentStore::open(&path, Default::default()).unwrap();
    assert_eq!(reopened.checkpoint("monitor"), checkpoint);
    assert_eq!(reopened.list(), vec![record]);
}

#[test]
fn replay_rejects_stale_acknowledgement_and_duplicate_transaction() {
    let dir = TestDir::new("incident-replay-transitions");
    for stale_ack in [true, false] {
        let path = dir.path.join(format!("{stale_ack}.jsonl"));
        let mut store = IncidentStore::open(&path, Default::default()).unwrap();
        store.commit(observation(1)).unwrap();
        let id = store.list()[0].id.clone();
        drop(store);
        let event = if stale_ack {
            Event::Acknowledge {
                id,
                expected_revision: 2,
                actor: "operator".into(),
                note: "".into(),
                now_ms: 2,
            }
        } else {
            Event::Monitor {
                commit: observation(1),
            }
        };
        let mut bytes = serde_json::to_vec(&JournalEntry {
            format: 1,
            sequence: 2,
            event,
        })
        .unwrap();
        bytes.push(b'\n');
        std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap()
            .write_all(&bytes)
            .unwrap();
        assert!(matches!(
            IncidentStore::open(&path, Default::default()),
            Err(IncidentError::Corrupt(_))
        ));
    }
}
