use serde_json::Value;
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

const PUBLIC_DIR: &str = "src/interfaces/ui/public";

fn project_path(relative: impl AsRef<Path>) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(relative)
}

fn read_project_file(relative: impl AsRef<Path>) -> String {
    let relative = relative.as_ref();
    fs::read_to_string(project_path(relative))
        .unwrap_or_else(|error| panic!("cannot read {}: {error}", relative.display()))
        .replace("\r\n", "\n")
}

fn between<'a>(source: &'a str, start: &str, end: &str) -> &'a str {
    let (_, after_start) = source
        .split_once(start)
        .unwrap_or_else(|| panic!("missing start marker: {start}"));
    let (body, _) = after_start
        .split_once(end)
        .unwrap_or_else(|| panic!("missing end marker after: {start}"));
    body
}

fn quoted_values(value: &str) -> BTreeSet<&str> {
    value
        .split('"')
        .enumerate()
        .filter_map(|(index, part)| (index % 2 == 1).then_some(part))
        .collect()
}

#[test]
fn shared_static_asset_set_is_complete_and_nonempty() {
    for relative in [
        "index.html",
        "styles.css",
        "app.js",
        "api.js",
        "dom.js",
        "history.js",
        "monitoring.js",
        "plugin-monitoring.js",
        "project-logs.js",
        "refresh.js",
        "data.js",
        "shell.js",
        "favicon.svg",
        "favicon.ico",
    ] {
        let path = project_path(Path::new(PUBLIC_DIR).join(relative));
        let metadata = fs::metadata(&path)
            .unwrap_or_else(|error| panic!("missing shared UI asset {}: {error}", path.display()));
        assert!(
            metadata.is_file(),
            "UI asset is not a file: {}",
            path.display()
        );
        assert!(metadata.len() > 0, "UI asset is empty: {}", path.display());
    }
}

#[test]
fn index_loads_the_shared_entrypoints_and_accessible_announcer() {
    let index = read_project_file(format!("{PUBLIC_DIR}/index.html"));

    assert!(index.contains("href=\"./styles.css\""));
    assert!(index.contains("type=\"module\" src=\"./app.js\""));
    assert!(index.contains("http-equiv=\"Content-Security-Policy\""));
    for directive in [
        "default-src 'self'",
        "script-src 'self'",
        "object-src 'none'",
        "base-uri 'none'",
        "form-action 'none'",
    ] {
        assert!(index.contains(directive), "CSP is missing {directive:?}");
    }

    let app_mount = index.find("id=\"app\"").expect("missing UI mount");
    let announcer = index
        .find("id=\"route-announcer\"")
        .expect("missing route announcer");
    assert_ne!(
        app_mount, announcer,
        "route announcer must be a separate node"
    );
    assert!(index[announcer..].contains("role=\"status\""));
    assert!(index[announcer..].contains("aria-live=\"polite\""));
}

#[test]
fn shared_client_behaviour_preserves_unknown_conflicts_and_safe_text() {
    let node = std::env::var_os("RECUVORA_NODE_PATH").unwrap_or_else(|| "node".into());
    let mut child = std::process::Command::new(node)
        .arg("--experimental-vm-modules")
        .arg(project_path("tests/ui_client.mjs"))
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect(
            "UI behavioural tests require Node.js; set RECUVORA_NODE_PATH if it is not on PATH",
        );
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while child
        .try_wait()
        .expect("cannot inspect Node.js test process")
        .is_none()
    {
        if std::time::Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("UI behavioural tests exceeded their 30 second deadline");
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    let output = child
        .wait_with_output()
        .expect("cannot collect Node.js test output");
    assert!(
        output.status.success(),
        "UI behavioural checks failed:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn javascript_has_no_persistence_or_dynamic_html_escape_hatches() {
    let javascript = [
        "app.js",
        "api.js",
        "dom.js",
        "history.js",
        "monitoring.js",
        "plugin-monitoring.js",
        "project-logs.js",
        "refresh.js",
        "data.js",
        "shell.js",
    ]
    .map(|name| read_project_file(format!("{PUBLIC_DIR}/{name}")))
    .join("\n");

    for forbidden in [
        ".innerHTML",
        ".outerHTML",
        "insertAdjacentHTML(",
        "document.write(",
        "eval(",
        "XMLHttpRequest",
        "WebSocket(",
        "EventSource(",
        "sendBeacon(",
        "serviceWorker",
        "localStorage",
        "sessionStorage",
        "indexedDB",
        "caches.open(",
    ] {
        assert!(
            !javascript.contains(forbidden),
            "shared UI JavaScript contains forbidden API {forbidden:?}"
        );
    }

    assert!(javascript.contains("document.createTextNode(String(child))"));
    assert!(javascript.contains("routeAnnouncer.textContent ="));
}

#[test]
fn approval_catalog_has_all_states_and_distinguishes_unknown() {
    let app = read_project_file(format!("{PUBLIC_DIR}/app.js"));
    let catalog = between(&app, "const approvalStates = Object.freeze({", "\n});");
    let actual = catalog
        .lines()
        .filter_map(|line| {
            let line = line.strip_prefix("  ")?;
            let (key, value) = line.split_once(':')?;
            (value.trim_start().starts_with("{ label:")
                && key
                    .chars()
                    .all(|character| character == '_' || character.is_ascii_lowercase()))
            .then_some(key)
        })
        .collect::<BTreeSet<_>>();
    let expected = [
        "pending",
        "waiting_human",
        "approved",
        "denied",
        "revoked",
        "canceled",
        "expired",
        "executing",
        "executed",
        "failed",
        "unknown",
    ]
    .into_iter()
    .collect::<BTreeSet<_>>();

    assert_eq!(actual, expected);
    assert!(catalog.contains("unknown: { label: \"结果未知\", tone: \"unknown\" }"));
    assert!(catalog.contains("failed: { label: \"执行失败\", tone: \"danger\" }"));
    assert!(app.contains("结果未知：写入可能已经发生"));
    assert!(app.contains("切勿重新执行"));

    let styles = read_project_file(format!("{PUBLIC_DIR}/styles.css"));
    assert!(styles.contains(".tone-unknown"));
    assert!(styles.contains(".callout-unknown"));
    let print_styles = styles
        .split_once("@media print {")
        .expect("missing print styles")
        .1;
    let print_hidden = between(print_styles, "\n  .sidebar,", "\n  .main-shell");
    assert!(!print_hidden.contains(".preview-banner"));
    assert!(print_styles.contains("\n  .preview-banner {"));
}

#[test]
fn desktop_shell_and_cargo_features_point_to_the_same_ui() {
    let tauri: Value = serde_json::from_str(&read_project_file("tauri.conf.json"))
        .expect("tauri.conf.json must be valid JSON");
    assert_eq!(
        tauri.pointer("/build/frontendDist").and_then(Value::as_str),
        Some(PUBLIC_DIR)
    );
    assert_eq!(
        tauri.pointer("/bundle/active").and_then(Value::as_bool),
        Some(false)
    );

    let manifest = read_project_file("Cargo.toml");
    let features = between(&manifest, "[features]\n", "\n[dependencies]");
    let desktop_feature = features
        .lines()
        .find_map(|line| line.strip_prefix("desktop-ui = "))
        .expect("desktop-ui feature is missing");
    assert!(quoted_values(desktop_feature).contains("web-ui"));

    let desktop_bin = manifest
        .split("[[bin]]")
        .find(|block| block.contains("name = \"recuvora-desktop\""))
        .expect("recuvora-desktop binary is missing");
    let required_features = desktop_bin
        .lines()
        .find_map(|line| line.strip_prefix("required-features = "))
        .expect("recuvora-desktop required-features is missing");
    assert_eq!(
        quoted_values(required_features),
        BTreeSet::from(["desktop-ui"])
    );
}
