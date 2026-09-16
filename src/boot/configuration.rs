//! Shared trusted configuration loading for CLI and HTTP host assembly.
use crate::harnesses::{HarnessError, HarnessRegistryConfig};
use crate::recovery::workflow::{RepairConfig, WorkflowError};
use std::fs::File;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
pub fn load_repair_config(
    path: &Path,
    needs_target: bool,
) -> Result<(RepairConfig, Vec<PathBuf>), WorkflowError> {
    reject_links(path)?;
    let path = path.canonicalize()?;
    let file = File::open(&path)?;
    if !file.metadata()?.is_file() {
        return Err(invalid("repair configuration must be a file"));
    }
    let mut bytes = Vec::new();
    file.take(64 * 1024 + 1).read_to_end(&mut bytes)?;
    if bytes.len() > 64 * 1024 {
        return Err(invalid("repair configuration exceeds 64 KiB"));
    }
    let mut config: RepairConfig = serde_json::from_slice(&bytes)?;
    config.validate()?;
    let base = path
        .parent()
        .ok_or_else(|| invalid("configuration has no parent"))?;
    if let Some(extension) = &mut config.extensions_config {
        if extension.is_relative() {
            *extension = base.join(&*extension);
        }
        reject_links(extension)?;
        *extension = extension.canonicalize()?;
    }
    for value in [
        &mut config.harness_config,
        &mut config.target_root,
        &mut config.reviewer_directory,
        &mut config.data_dir,
    ] {
        if value.is_relative() {
            *value = base.join(&*value);
        }
    }
    // Validate lexical ancestors before resolving or creating trusted state;
    // never create runtime state inside a target or source tree through links.
    let reviewer_directory = config.reviewer_directory.clone();
    for value in [
        &mut config.target_root,
        &mut config.reviewer_directory,
        &mut config.harness_config,
    ] {
        *value = absolute_normal(value)?;
        if needs_target {
            reject_links(value)?;
            if *value != reviewer_directory || config.reviewer_workspace.is_none() {
                *value = value.canonicalize()?;
            }
        }
    }
    let source = match Path::new(env!("CARGO_MANIFEST_DIR")).canonicalize() {
        Ok(p) => Some(p),
        Err(e) if e.kind() == io::ErrorKind::NotFound => None,
        Err(e) => return Err(e.into()),
    };
    if source.as_ref().is_some_and(|s| {
        path.starts_with(s)
            || config.harness_config.starts_with(s)
            || config
                .extensions_config
                .as_ref()
                .is_some_and(|p| p.starts_with(s))
    }) {
        return Err(invalid(
            "active configurations must be outside the project source tree",
        ));
    }
    if path.starts_with(&config.target_root)
        || config.harness_config.starts_with(&config.target_root)
        || config.reviewer_directory.starts_with(&config.target_root)
        || config
            .extensions_config
            .as_ref()
            .is_some_and(|p| p.starts_with(&config.target_root))
    {
        return Err(invalid(
            "configuration and reviewer context must be outside the repair target",
        ));
    }
    let state = absolute_normal(&config.data_dir)?;
    if state.starts_with(&config.target_root)
        || source.as_ref().is_some_and(|s| state.starts_with(s))
    {
        return Err(invalid("state directory must be outside target and source"));
    }
    reject_links(&state)?;
    std::fs::create_dir_all(&state)?;
    config.data_dir = state.canonicalize()?;
    let installation = std::env::current_exe()?
        .canonicalize()?
        .parent()
        .ok_or_else(|| invalid("executable has no parent"))?
        .to_path_buf();
    let mut protected = vec![
        path,
        config.harness_config.clone(),
        config.data_dir.clone(),
        config.reviewer_directory.clone(),
        installation,
    ];
    if let Some(source) = source {
        protected.push(source);
    }
    if let Some(extension) = &config.extensions_config {
        protected.push(extension.clone());
        if needs_target {
            protected.extend(extension_protected_paths(extension, &config.target_root)?);
        }
    }
    Ok((config, protected))
}

/// Protect transport executables, package working directories and explicit
/// local file arguments (including --config paths). Remote SSH arguments are
/// opaque; the node owns validation of its remote installation paths.
pub fn extension_protected_paths(
    path: &Path,
    target: &Path,
) -> Result<Vec<PathBuf>, WorkflowError> {
    reject_links(path)?;
    let config = crate::nodes::ExtensionsConfig::load(path).map_err(|e| invalid(e.to_string()))?;
    let mut protected = vec![path.canonicalize()?];
    for definition in config.extensions {
        let program = definition.command.program;
        let cwd = definition.command.cwd;
        for path in [&program, &cwd] {
            reject_links(path)?;
        }
        let program = program.canonicalize()?;
        let cwd = cwd.canonicalize()?;
        let ssh_transport = program
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| {
                name.eq_ignore_ascii_case("ssh") || name.eq_ignore_ascii_case("ssh.exe")
            });
        let package = program
            .parent()
            .ok_or_else(|| invalid("extension executable has no parent"))?
            .to_path_buf();
        protected.extend([program, package, cwd.clone()]);
        let mut require_config = false;
        for arg in definition.command.args {
            if !ssh_transport && arg == "--config" {
                require_config = true;
                continue;
            }
            let explicit = !ssh_transport && (require_config || arg.starts_with("--config="));
            let value = arg.strip_prefix("--config=").unwrap_or(&arg);
            require_config = false;
            let candidate = Path::new(value);
            let candidate = if candidate.is_absolute() {
                candidate.to_path_buf()
            } else {
                cwd.join(candidate)
            };
            if explicit || candidate.is_file() {
                reject_links(&candidate)?;
                protected.push(candidate.canonicalize()?);
            }
        }
        if require_config {
            return Err(invalid(
                "extension --config requires a local configuration path",
            ));
        }
    }
    for path in &protected {
        if path.starts_with(target) || target.starts_with(path) {
            return Err(invalid(
                "repair target overlaps extension configuration, program or package context",
            ));
        }
    }
    protected.sort();
    protected.dedup();
    Ok(protected)
}

fn absolute_normal(path: &Path) -> Result<PathBuf, WorkflowError> {
    use std::path::Component;
    let mut output = PathBuf::new();
    for part in path.components() {
        match part {
            Component::CurDir => {}
            Component::ParentDir => {
                if !output.pop() {
                    return Err(invalid("path escapes filesystem root"));
                }
            }
            other => output.push(other.as_os_str()),
        }
    }
    if !output.is_absolute() {
        return Err(invalid("absolute path required"));
    }
    Ok(output)
}

fn reject_links(path: &Path) -> Result<(), WorkflowError> {
    for ancestor in path.ancestors() {
        match std::fs::symlink_metadata(ancestor) {
            Ok(meta) => {
                #[cfg(windows)]
                {
                    use std::os::windows::fs::MetadataExt;
                    if meta.file_attributes() & 0x400 != 0 {
                        return Err(invalid(
                            "reparse points in configuration or runtime paths are forbidden",
                        ));
                    }
                }
                if meta.file_type().is_symlink() {
                    return Err(invalid("linked configuration/runtime paths are forbidden"));
                }
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
    }
    Ok(())
}

fn invalid(message: impl Into<String>) -> WorkflowError {
    WorkflowError::Invalid(message.into())
}

pub fn load_harness_config(path: &Path) -> Result<HarnessRegistryConfig, HarnessError> {
    let path = std::fs::canonicalize(path).map_err(|error| {
        HarnessError::InvalidConfiguration(format!("cannot resolve active configuration: {error}"))
    })?;
    let project = match std::fs::canonicalize(env!("CARGO_MANIFEST_DIR")) {
        Ok(project) => Some(project),
        // A copied executable does not require its build machine's source tree.
        Err(error) if error.kind() == io::ErrorKind::NotFound => None,
        Err(error) => {
            return Err(HarnessError::InvalidConfiguration(format!(
                "cannot inspect source directory: {error}"
            )));
        }
    };
    if project.is_some_and(|project| path.starts_with(project)) {
        return Err(HarnessError::InvalidConfiguration(
            "active configuration must be outside the project source tree; copy the example to an external directory and adjust workspace_roots".to_owned(),
        ));
    }
    let config = HarnessRegistryConfig::load(path)?;
    config.validate()?;
    Ok(config)
}
