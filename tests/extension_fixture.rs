//! Shared integration-test child process. Never a product executable.
use recuvora::nodes::{
    AllowedMethod, ExtensionDefinition, MONITORING_VIEW_CAPABILITY, MONITORING_VIEW_METHOD,
};
use recuvora::protocol::{
    CommandSpec, ContractDeclaration, ExtensionKind, ExtensionMetadata, Message, MethodDeclaration,
    Outcome,
};
use serde_json::json;
use std::collections::BTreeMap;
use std::error::Error;
use std::io::{BufRead, Write};

pub type TestResult<T = ()> = Result<T, Box<dyn Error + Send + Sync>>;

pub fn definition(id: &str, kind: ExtensionKind, mode: &str) -> TestResult<ExtensionDefinition> {
    let contract = if id == "fixture-node" {
        "recuvora.harness"
    } else {
        "com.example.logs"
    };
    let methods = if id == "fixture-node" {
        vec!["run", "projects", "create_project"]
    } else {
        vec!["query"]
    };
    let mut allow_calls = methods
        .into_iter()
        .map(|method| AllowedMethod {
            contract: contract.into(),
            version: 1,
            method: method.into(),
        })
        .collect::<Vec<_>>();
    if matches!(
        mode,
        "view-good"
            | "view-bad-result"
            | "view-callback"
            | "view-wait-cancel"
            | "view-duplicate"
            | "view-not-read-only"
    ) {
        allow_calls.push(AllowedMethod {
            contract: "com.example.logs.monitoring_view".into(),
            version: 1,
            method: MONITORING_VIEW_METHOD.into(),
        });
    }
    if mode == "view-duplicate" {
        allow_calls.push(AllowedMethod {
            contract: "com.example.logs.other_view".into(),
            version: 1,
            method: MONITORING_VIEW_METHOD.into(),
        });
    }
    Ok(ExtensionDefinition {
        id: id.into(),
        kind,
        enabled: true,
        command: CommandSpec {
            program: std::env::current_exe()?,
            args: vec!["--fixture".into(), mode.into()],
            cwd: std::env::current_dir()?,
            env: BTreeMap::from([("RECUVORA_EXTENSION_FIXTURE".into(), "1".into())]),
        },
        namespaces: if kind == ExtensionKind::Plugin {
            vec!["com.example.logs".into()]
        } else {
            vec![]
        },
        allow_calls,
        allow_nodes: if kind == ExtensionKind::Plugin {
            vec!["logs-node".into()]
        } else {
            vec![]
        },
    })
}

pub fn maybe_fixture() -> TestResult<bool> {
    let args = std::env::args().collect::<Vec<_>>();
    if args.get(1).map(String::as_str) != Some("--fixture") {
        return Ok(false);
    }
    if std::env::var("RECUVORA_EXTENSION_FIXTURE").as_deref() != Ok("1") {
        return Err("fixture mode requires explicit environment".into());
    }
    fixture(args.get(2).map(String::as_str).unwrap_or("good"))?;
    Ok(true)
}
fn read(reader: &mut impl BufRead) -> TestResult<Option<Message>> {
    let mut line = String::new();
    if reader.read_line(&mut line)? == 0 {
        return Ok(None);
    }
    Ok(Some(serde_json::from_str(&line)?))
}
fn write(writer: &mut impl Write, message: Message) -> TestResult {
    serde_json::to_writer(&mut *writer, &message)?;
    writer.write_all(b"\n")?;
    writer.flush()?;
    Ok(())
}
fn fixture(mode: &str) -> TestResult {
    let input = std::io::stdin();
    let mut reader = input.lock();
    let output = std::io::stdout();
    let mut writer = output.lock();
    let Some(Message::Hello {
        expected_id, kind, ..
    }) = read(&mut reader)?
    else {
        return Err("expected hello".into());
    };
    let harness = expected_id == "fixture-node";
    let mut contracts = if harness {
        vec![ContractDeclaration {
            id: "recuvora.harness".into(),
            version: 1,
            methods: [
                ("run", false, "object"),
                ("projects", true, "array"),
                ("create_project", false, "object"),
            ]
            .into_iter()
            .map(|(name, read_only, output)| MethodDeclaration {
                name: name.into(),
                read_only,
                input_schema: json!({"type":"object"}),
                output_schema: json!({"type":output}),
            })
            .collect(),
        }]
    } else {
        vec![ContractDeclaration {
            id: "com.example.logs".into(),
            version: 1,
            methods: vec![MethodDeclaration {
                name: "query".into(),
                read_only: true,
                input_schema: json!({"type":"object","properties":{"needle":{"type":"string","maxLength":64}},"required":["needle"],"additionalProperties":false}),
                output_schema: json!({"type":"object","properties":{"entries":{"type":"array","items":{"type":"string"},"maxItems":16}},"required":["entries"],"additionalProperties":false}),
            }],
        }]
    };
    if !harness && mode.starts_with("view-") {
        contracts.push(ContractDeclaration {
            id: "com.example.logs.monitoring_view".into(),
            version: 1,
            methods: vec![MethodDeclaration {
                name: MONITORING_VIEW_METHOD.into(),
                read_only: mode != "view-not-read-only",
                input_schema: json!({"type":"object","properties":{"schema_version":{"type":"integer","enum":[1]}},"required":["schema_version"],"additionalProperties":false}),
                output_schema: json!({"type":"object"}),
            }],
        });
        if mode == "view-duplicate" {
            contracts.push(ContractDeclaration {
                id: "com.example.logs.other_view".into(),
                version: 1,
                methods: vec![MethodDeclaration {
                    name: MONITORING_VIEW_METHOD.into(),
                    read_only: true,
                    input_schema: json!({"type":"object"}),
                    output_schema: json!({"type":"object"}),
                }],
            });
        }
    }
    let mut metadata = ExtensionMetadata {
        protocol_version: if mode == "bad-version" { 9 } else { 1 },
        id: if mode == "bad-identity" {
            "impostor".into()
        } else {
            expected_id.clone()
        },
        kind,
        contracts,
        capabilities: if harness {
            ["text", "projects", "tools", "client_visibility", "approval"]
                .into_iter()
                .map(str::to_owned)
                .collect()
        } else if mode.starts_with("view-") {
            vec![MONITORING_VIEW_CAPABILITY.into()]
        } else {
            vec![]
        },
        workspaces: if harness {
            vec!["work".into(), "review".into()]
        } else {
            vec![]
        },
    };
    if mode == "bad-schema" {
        metadata.contracts[0].methods[0].input_schema =
            json!({"type":"object","$ref":"unimplemented"});
    }
    write(&mut writer, Message::Ready { metadata })?;
    let Some(Message::Call {
        id, method, params, ..
    }) = read(&mut reader)?
    else {
        return Ok(());
    };
    if mode == "disconnect" {
        return Ok(());
    }
    if mode == "wait-cancel" {
        if let Some(Message::Cancel { id: cancel_id }) = read(&mut reader)? {
            assert_eq!(cancel_id, id);
            write(
                &mut writer,
                Message::Error {
                    id,
                    code: "cancelled".into(),
                    message: "dispatched request cancelled; persistent effects need reconciliation"
                        .into(),
                    outcome: Outcome::Unknown,
                },
            )?;
        }
        return Ok(());
    }
    if mode == "view-wait-cancel" && method == MONITORING_VIEW_METHOD {
        let marker = std::env::var_os("RECUVORA_VIEW_STARTED")
            .ok_or("view wait fixture requires a start marker")?;
        std::fs::write(marker, b"started")?;
        if let Some(Message::Cancel { id: cancel_id }) = read(&mut reader)? {
            assert_eq!(cancel_id, id);
            write(
                &mut writer,
                Message::Error {
                    id,
                    code: "cancelled".into(),
                    message: "monitoring view descriptor cancelled".into(),
                    outcome: Outcome::Cancelled,
                },
            )?;
        }
        return Ok(());
    }
    if !harness {
        let result = if method == MONITORING_VIEW_METHOD {
            if mode == "view-callback" {
                write(
                    &mut writer,
                    Message::Callback {
                        id: "view-read-node".into(),
                        parent_id: id.clone(),
                        method: "service.call".into(),
                        params: json!({"node_id":"logs-node","contract":"com.example.logs","version":1,"method":"query","params":{"needle":"forbidden"}}),
                    },
                )?;
            }
            if mode == "view-bad-result" {
                json!("invalid descriptor")
            } else {
                json!({"schema_version":1,"title":"Fixture monitoring","sections":[]})
            }
        } else if mode == "bad-result" {
            json!("invalid output")
        } else if kind == ExtensionKind::Plugin {
            write(
                &mut writer,
                Message::Callback {
                    id: "read-node".into(),
                    parent_id: id.clone(),
                    method: "service.call".into(),
                    params: json!({"node_id":"logs-node","contract":"com.example.logs","version":1,"method":"query","params":params}),
                },
            )?;
            match read(&mut reader)? {
                Some(Message::Result {
                    id: callback_id,
                    result,
                }) if callback_id == "read-node" => {
                    json!({"entries":[format!("consumed:{}",result["entries"][0].as_str().unwrap_or(""))]})
                }
                _ => return Err("plugin callback failed".into()),
            }
        } else {
            json!({"entries":[params["needle"]]})
        };
        write(&mut writer, Message::Result { id, result })?;
    } else {
        assert_eq!(params["workspace"]["node_id"], "fixture-node");
        assert!(
            ["work", "review"]
                .contains(&params["workspace"]["workspace_id"].as_str().unwrap_or(""))
        );
        let remote_path = "C:\\OnlyOnTheNode\\Project";
        let result = match method.as_str() {
            "projects" => json!([{"id":"p1","name":"Example","roots":[remote_path]}]),
            "create_project" => json!({"id":"created","name":params["name"],"roots":[remote_path]}),
            "run" => {
                let approval = params["role"] == "approval";
                if approval {
                    assert_eq!(params["visibility"], "hidden");
                    assert_eq!(params["placement"]["type"], "none");
                    assert!(params["tools"].as_array().is_some_and(Vec::is_empty));
                }
                if params["tools"]
                    .as_array()
                    .is_some_and(|tools| !tools.is_empty())
                {
                    let callback = Message::Callback {
                        id: "tool-rpc".into(),
                        parent_id: if mode == "wrong-parent" {
                            "other".into()
                        } else {
                            id.clone()
                        },
                        method: "tool".into(),
                        params: json!({"harness_id":params["harness_id"],"thread_id":"thread-1","turn_id":"turn-1","call_id":"call-1","tool":params["tools"][0]["name"],"arguments":{"text":"hello"}}),
                    };
                    write(&mut writer, callback.clone())?;
                    match read(&mut reader)? {
                        Some(Message::Result { result, .. }) => assert_eq!(result["success"], true),
                        Some(Message::Cancel { .. }) | None => return Ok(()),
                        _ => return Err("tool callback failed".into()),
                    }
                    if mode == "duplicate-tool" {
                        write(&mut writer, callback)?;
                        let _ = read(&mut reader)?;
                        return Ok(());
                    }
                }
                json!({"thread_id":"thread-1","session_id":"session-1","project_directory":remote_path,"visibility":params["visibility"],"native_project_id":params["placement"].get("project_id"),"client_project_grouping":{"type":if params["visibility"]=="hidden" {"not_applicable"}else{"unverified"}},"final_response":if approval {"APPROVAL_FIXTURE"}else{"REMOTE_FIXTURE"}})
            }
            _ => return Err("unknown Harness method".into()),
        };
        write(&mut writer, Message::Result { id, result })?;
    }
    // Handshake probe and call transports are explicitly closed by the host.
    let _ = read(&mut reader)?;
    Ok(())
}
