//! Real process protocol integration; this executable is only a test fixture.
use recuvora::monitors::{MonitorEngine, MonitorsConfig};
use recuvora::nodes::{AllowedMethod, ExtensionDefinition, ExtensionRegistry, ExtensionsConfig};
use recuvora::protocol::{
    CommandSpec, ContractDeclaration, ExtensionKind, ExtensionMetadata, Message, MethodDeclaration,
};
use recuvora::recovery::incidents::{IncidentKind, IncidentStatus};
use serde_json::json;
use std::collections::BTreeMap;
use std::error::Error;
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

type Result<T = ()> = std::result::Result<T, Box<dyn Error + Send + Sync>>;
fn main() -> Result {
    if std::env::args().nth(1).as_deref() == Some("--monitor-fixture") {
        if std::env::var("RECUVORA_MONITOR_FIXTURE").as_deref() != Ok("1") {
            return Err("fixture requires explicit environment".into());
        }
        return fixture();
    }
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(run())
}
fn read(reader: &mut impl BufRead) -> Result<Option<Message>> {
    let mut line = String::new();
    if reader.read_line(&mut line)? == 0 {
        return Ok(None);
    }
    Ok(Some(serde_json::from_str(&line)?))
}
fn write(writer: &mut impl Write, message: Message) -> Result {
    serde_json::to_writer(&mut *writer, &message)?;
    writer.write_all(b"\n")?;
    writer.flush()?;
    Ok(())
}
fn fixture() -> Result {
    let stdin = std::io::stdin();
    let mut input = stdin.lock();
    let stdout = std::io::stdout();
    let mut output = stdout.lock();
    let Some(Message::Hello {
        expected_id, kind, ..
    }) = read(&mut input)?
    else {
        return Err("expected hello".into());
    };
    write(
        &mut output,
        Message::Ready {
            metadata: ExtensionMetadata {
                protocol_version: 1,
                id: expected_id,
                kind,
                contracts: vec![ContractDeclaration {
                    id: "example.monitor".into(),
                    version: 1,
                    methods: vec![MethodDeclaration {
                        name: "observe".into(),
                        read_only: true,
                        input_schema: json!({"type":"object"}),
                        output_schema: json!({"type":"object"}),
                    }],
                }],
                capabilities: vec![],
                workspaces: vec![],
            },
        },
    )?;
    let Some(Message::Call { id, params, .. }) = read(&mut input)? else {
        return Ok(());
    };
    if kind == ExtensionKind::Plugin {
        write(
            &mut output,
            Message::Callback {
                id: "read-node".into(),
                parent_id: id.clone(),
                method: "service.call".into(),
                params: json!({"node_id":"probe-node","contract":"example.monitor","version":1,"method":"observe","params":params}),
            },
        )?;
        match read(&mut input)? {
            Some(Message::Result { result, .. }) => {
                write(&mut output, Message::Result { id, result })?
            }
            _ => return Ok(()), // Lost node result must propagate as Unknown, never empty healthy data.
        }
    } else {
        let sequence = params["cursor"]
            .as_str()
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(0)
            + 1;
        if sequence >= 5 {
            return Ok(());
        }
        write(
            &mut output,
            Message::Result {
                id,
                result: json!({
                    "schema_version":1,"target_id":"service","source_id":"probe","generation":"g1",
                    "cursor":params["cursor"],"next_cursor":sequence.to_string(),"coverage":"complete","has_more":false,"error":null,
                    "samples":[{"id":format!("sample-{sequence}"),"sequence":sequence,"age_ms":0,"value":{"ready":sequence>=3},"evidence":{"source":"wire-fixture"}}]
                }),
            },
        )?;
    }
    // Normal result is followed by stdin EOF; no persistent transport is implied.
    while read(&mut input)?.is_some() {}
    Ok(())
}

struct TestDir {
    path: PathBuf,
    root: PathBuf,
}
impl TestDir {
    fn new() -> Result<Self> {
        let root = PathBuf::from(
            std::env::var_os("RECUVORA_TEST_TEMP").ok_or("external test root required")?,
        );
        if !root.is_absolute() {
            return Err("absolute test root required".into());
        }
        std::fs::create_dir_all(&root)?;
        let root = root.canonicalize()?;
        if root.starts_with(Path::new(env!("CARGO_MANIFEST_DIR")).canonicalize()?) {
            return Err("external root required".into());
        }
        let stamp = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
        let path = root.join(format!("monitor-wire-{}-{stamp}", std::process::id()));
        std::fs::create_dir(&path)?;
        Ok(Self { path, root })
    }
}
impl Drop for TestDir {
    fn drop(&mut self) {
        assert_eq!(self.path.parent(), Some(self.root.as_path()));
        std::fs::remove_dir_all(&self.path).expect("remove recorded monitor wire fixture");
    }
}
fn definition(id: &str, kind: ExtensionKind, dir: &Path) -> Result<ExtensionDefinition> {
    Ok(ExtensionDefinition {
        id: id.into(),
        kind,
        enabled: true,
        command: CommandSpec {
            program: std::env::current_exe()?,
            args: vec!["--monitor-fixture".into()],
            cwd: dir.into(),
            env: BTreeMap::from([("RECUVORA_MONITOR_FIXTURE".into(), "1".into())]),
        },
        namespaces: if kind == ExtensionKind::Plugin {
            vec!["example.monitor".into()]
        } else {
            vec![]
        },
        allow_calls: vec![AllowedMethod {
            contract: "example.monitor".into(),
            version: 1,
            method: "observe".into(),
        }],
        allow_nodes: if kind == ExtensionKind::Plugin {
            vec!["probe-node".into()]
        } else {
            vec![]
        },
    })
}
async fn run() -> Result {
    let dir = TestDir::new()?;
    let registry = Arc::new(
        ExtensionRegistry::connect(ExtensionsConfig {
            schema_version: 1,
            extensions: vec![
                definition("probe-plugin", ExtensionKind::Plugin, &dir.path)?,
                definition("probe-node", ExtensionKind::Node, &dir.path)?,
            ],
        })
        .await?,
    );
    let config: MonitorsConfig = serde_json::from_value(json!({"schema_version":1,"monitors":[{
        "id":"ready","target_id":"service","source_id":"probe","extension_id":"probe-plugin",
        "contract":"example.monitor","version":1,"method":"observe","params":{},"interval_ms":200,"timeout_ms":5000,
        "stale_after_ms":20000,"startup_grace_ms":20000,
        "rule":{"pointer":"/ready","operator":"eq","value":true,"failure_samples":2,"success_samples":2}
    }]}))?;
    let mut engine = MonitorEngine::start(config, registry.clone(), dir.path.join("runtime"))?;
    let handle = engine.handle();
    tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            let records = handle.incidents()?;
            let recovered = records
                .iter()
                .any(|r| r.kind == IncidentKind::Target && r.status == IncidentStatus::Resolved);
            let lost = records
                .iter()
                .any(|r| r.kind == IncidentKind::Coverage && r.status == IncidentStatus::Open);
            if recovered && lost {
                return Result::Ok(());
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await??;
    let records = handle.incidents()?;
    let target = records
        .iter()
        .find(|r| r.kind == IncidentKind::Target)
        .ok_or("missing target incident")?;
    assert_eq!(target.evidence["sample_id"], "sample-4");
    assert!(target.resolved_at.is_some());
    assert!(
        handle
            .monitor("ready")?
            .ok_or("missing monitor")?
            .last_error
            .is_some()
    );
    engine.shutdown().await?;
    registry.shutdown().await?;
    println!(
        "monitor_wire: real plugin/node calls, durable target fault and evidence resolution, disconnect coverage passed"
    );
    Ok(())
}
