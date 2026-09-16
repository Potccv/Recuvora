//! Bounded local text actions. Mutation is private to the trusted host.
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::fs::{File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Component, Path, PathBuf};
use thiserror::Error;

pub const MAX_FILE_BYTES: usize = 16 * 1024;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct TextEdit {
    pub path: String,
    pub expected: String,
    pub replacement: String,
}

#[derive(Debug, Error)]
pub enum ActionError {
    #[error("action denied: {0}")]
    Denied(String),
    #[error("target changed; obtain a new approval")]
    Changed,
    #[error("action preparation failed: {0}")]
    Io(#[from] io::Error),
    #[error("write may have occurred; verify target before any retry: {0}")]
    Unknown(String),
}

#[derive(Debug)]
pub struct ScopedFiles {
    root: PathBuf,
    allowed: BTreeSet<String>,
    protected: Vec<PathBuf>,
    _root_handles: Vec<File>,
}

impl ScopedFiles {
    /// `protected` includes active policy/configuration, state, source and
    /// installation paths. The caller is trusted assembly, never model input.
    pub fn open(
        root: &Path,
        allowed: &[String],
        protected: &[PathBuf],
    ) -> Result<Self, ActionError> {
        if !root.is_absolute() || allowed.is_empty() || allowed.len() > 64 {
            return Err(denied("absolute target and 1..64 allowed files required"));
        }
        reject_control_directories(root)?;
        let handles = pin_directories(root)?;
        let root = root.canonicalize()?;
        reject_control_directories(&root)?;
        let root_handle = handles
            .last()
            .ok_or_else(|| denied("target root handle is unavailable"))?;
        verify_handle_path(root_handle, &root)?;
        let mut protected_paths = Vec::new();
        for path in protected {
            let canonical = path.canonicalize()?;
            if path_within(&root, &canonical)? {
                return Err(denied("target lies inside a protected path"));
            }
            protected_paths.push(canonical);
        }
        let mut names = BTreeSet::new();
        for name in allowed {
            validate_relative(name)?;
            if !names.insert(name.clone()) {
                return Err(denied("duplicate allowed file"));
            }
        }
        let scoped = Self {
            root,
            allowed: names,
            protected: protected_paths,
            _root_handles: handles,
        };
        for name in &scoped.allowed {
            // Detect missing, linked, protected or oversized targets at startup.
            scoped.read(name)?;
        }
        Ok(scoped)
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn allowed_files(&self) -> Vec<String> {
        self.allowed.iter().cloned().collect()
    }

    pub fn read(&self, path: &str) -> Result<String, ActionError> {
        let (mut file, _handles) = self.open_file(path, false)?;
        read_bounded(&mut file).map_err(Into::into)
    }

    pub(crate) fn prepare(&self, edit: &TextEdit) -> Result<PreparedEdit, ActionError> {
        if edit.expected.len() > MAX_FILE_BYTES || edit.replacement.len() > MAX_FILE_BYTES {
            return Err(denied("text exceeds 16 KiB"));
        }
        let (mut file, handles) = self.open_file(&edit.path, true)?;
        if read_bounded(&mut file)? != edit.expected {
            return Err(ActionError::Changed);
        }
        Ok(PreparedEdit {
            file,
            _handles: handles,
            expected_path: self.root.join(&edit.path),
            edit: edit.clone(),
        })
    }

    fn open_file(&self, name: &str, write: bool) -> Result<(File, Vec<File>), ActionError> {
        validate_relative(name)?;
        if !self.allowed.contains(name) {
            return Err(denied("file not explicitly allowed"));
        }
        let path = self.root.join(name);
        let parent = path.parent().ok_or_else(|| denied("file has no parent"))?;
        let handles = pin_directories(parent)?;
        let file = open_pinned_file(&path, write)?;
        // Resolve the object we actually opened, never a second path lookup.
        // A directory can acquire/remove a reparse attribute despite a handle
        // denying delete sharing; that cannot redirect this identity check.
        let actual = verify_handle_path(&file, &path)?;
        if !path_within(&actual, &self.root)? {
            return Err(denied("file resolves to a protected or out-of-scope path"));
        }
        for protected in &self.protected {
            if path_within(&actual, protected)? {
                return Err(denied("file resolves to a protected or out-of-scope path"));
            }
        }
        verify_file(&file)?;
        Ok((file, handles))
    }
}

pub(crate) struct PreparedEdit {
    file: File,
    _handles: Vec<File>,
    expected_path: PathBuf,
    edit: TextEdit,
}

#[derive(Clone, Debug, Serialize)]
pub struct ActionReceipt {
    pub path: String,
    pub bytes_written: usize,
    pub content_verified: bool,
}

impl PreparedEdit {
    /// Only the trusted workflow invokes this, after consuming its exact
    /// persisted permit. Handles forbid concurrent write/delete until receipt.
    pub(crate) fn execute(mut self) -> Result<ActionReceipt, ActionError> {
        verify_handle_path(&self.file, &self.expected_path)?;
        verify_file(&self.file)?;
        if read_bounded(&mut self.file)? != self.edit.expected {
            return Err(ActionError::Changed);
        }
        self.file.seek(SeekFrom::Start(0))?;
        let write_result = (|| -> io::Result<()> {
            self.file.write_all(self.edit.replacement.as_bytes())?;
            self.file.set_len(self.edit.replacement.len() as u64)?;
            self.file.sync_all()?;
            if read_bounded(&mut self.file)? != self.edit.replacement {
                return Err(io::Error::other(
                    "read-back did not match approved replacement",
                ));
            }
            Ok(())
        })();
        write_result.map_err(|error| ActionError::Unknown(error.to_string()))?;
        Ok(ActionReceipt {
            path: self.edit.path,
            bytes_written: self.edit.replacement.len(),
            content_verified: true,
        })
    }
}

fn read_bounded(file: &mut File) -> io::Result<String> {
    if file.metadata()?.len() > MAX_FILE_BYTES as u64 {
        return Err(io::Error::other("file exceeds 16 KiB"));
    }
    file.seek(SeekFrom::Start(0))?;
    let mut bytes = Vec::new();
    file.take(MAX_FILE_BYTES as u64 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > MAX_FILE_BYTES {
        return Err(io::Error::other("file exceeds 16 KiB"));
    }
    String::from_utf8(bytes).map_err(|_| io::Error::other("file is not UTF-8"))
}

fn validate_relative(name: &str) -> Result<(), ActionError> {
    if name.is_empty()
        || name.len() > 512
        || name.contains('\\')
        || name.contains(':')
        || name.chars().any(char::is_control)
    {
        return Err(denied("use a bounded relative path with forward slashes"));
    }
    for segment in name.split('/') {
        let upper = segment.to_ascii_uppercase();
        let stem = upper.split('.').next().unwrap_or("");
        if segment.is_empty()
            || segment == "."
            || segment == ".."
            || segment.starts_with('.')
            || segment.ends_with(['.', ' '])
            || matches!(stem, "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$")
            || ((stem.starts_with("COM") || stem.starts_with("LPT")) && stem.len() == 4)
        {
            return Err(denied("invalid or protected path component"));
        }
    }
    if Path::new(name)
        .components()
        .any(|part| !matches!(part, Component::Normal(_)))
    {
        return Err(denied("only ordinary relative path components are allowed"));
    }
    Ok(())
}

fn denied(message: &str) -> ActionError {
    ActionError::Denied(message.to_owned())
}

fn reject_control_directories(path: &Path) -> Result<(), ActionError> {
    #[cfg(test)]
    let allowed_test_root = external_test_root();
    let mut prefix = PathBuf::new();
    for part in path.components() {
        prefix.push(part.as_os_str());
        if let Component::Normal(name) = part {
            let name = name.to_string_lossy();
            if name.starts_with('.') {
                #[cfg(test)]
                if allowed_test_root.as_ref().is_some_and(|root| {
                    prefix
                        .canonicalize()
                        .is_ok_and(|candidate| candidate == *root)
                }) {
                    continue;
                }
                return Err(denied("target root cannot be inside a control directory"));
            }
        }
    }
    Ok(())
}

/// Unit tests keep all generated targets below one caller-provided external
/// directory. Permit only that directory itself to have a dot-prefixed name;
/// nested control directories remain denied. Production builds neither read
/// nor honor this environment variable.
#[cfg(test)]
fn external_test_root() -> Option<PathBuf> {
    let root = std::fs::canonicalize(std::env::var_os("RECUVORA_TEST_TEMP")?).ok()?;
    let project = std::fs::canonicalize(env!("CARGO_MANIFEST_DIR")).ok()?;
    if root.starts_with(&project) || project.starts_with(&root) {
        return None;
    }
    Some(root)
}

#[cfg(windows)]
fn pin_directories(path: &Path) -> io::Result<Vec<File>> {
    use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
    use std::path::Prefix;
    if !matches!(path.components().next(), Some(Component::Prefix(p)) if matches!(p.kind(), Prefix::Disk(_) | Prefix::VerbatimDisk(_)))
        || path
            .components()
            .any(|p| matches!(p, Component::ParentDir | Component::CurDir))
        || !path.is_absolute()
    {
        return Err(io::Error::other(
            "only absolute local disk directories are supported",
        ));
    }
    let mut ancestors: Vec<_> = path.ancestors().collect();
    ancestors.reverse();
    let mut handles = Vec::new();
    for ancestor in ancestors {
        let handle = OpenOptions::new()
            .access_mode(0)
            .share_mode(3)
            .custom_flags(0x0200_0000 | 0x0020_0000)
            .security_qos_flags(0x0010_0000 | 0x0001_0000)
            .open(ancestor)?;
        let metadata = handle.metadata()?;
        if !metadata.is_dir() || metadata.file_attributes() & 0x400 != 0 {
            return Err(io::Error::other("directory reparse points are forbidden"));
        }
        verify_handle_path(&handle, ancestor)?;
        handles.push(handle);
    }
    Ok(handles)
}

#[cfg(windows)]
fn open_pinned_file(path: &Path, write: bool) -> io::Result<File> {
    use std::os::windows::fs::OpenOptionsExt;
    OpenOptions::new()
        .read(true)
        .write(write)
        .share_mode(1)
        .custom_flags(0x0020_0000)
        .security_qos_flags(0x0010_0000 | 0x0001_0000)
        .open(path)
}

#[cfg(windows)]
fn verify_file(file: &File) -> io::Result<()> {
    let info = winapi_util::file::information(file)?;
    if !file.metadata()?.is_file()
        || info.file_attributes() & 0x400 != 0
        || info.number_of_links() != 1
    {
        return Err(io::Error::other(
            "only regular files without reparse points or hard links are allowed",
        ));
    }
    Ok(())
}

/// `filepath` returns the normalized path of an open Windows object and strips
/// the extended DOS prefix. Compare components without following paths again.
#[cfg(windows)]
fn verify_handle_path(file: &File, expected: &Path) -> io::Result<PathBuf> {
    use filepath::FilePath;
    let actual = file.path()?;
    if windows_components(&actual)? != windows_components(expected)? {
        return Err(io::Error::other(
            "opened object does not match its explicitly allowed path",
        ));
    }
    Ok(actual)
}

#[cfg(windows)]
fn path_within(path: &Path, root: &Path) -> io::Result<bool> {
    Ok(windows_components(path)?.starts_with(&windows_components(root)?))
}

#[cfg(windows)]
fn windows_components(path: &Path) -> io::Result<Vec<std::ffi::OsString>> {
    use std::path::Prefix;
    let mut parts = path.components();
    let drive = match parts.next() {
        Some(Component::Prefix(prefix)) => match prefix.kind() {
            Prefix::Disk(drive) | Prefix::VerbatimDisk(drive) => drive,
            _ => return Err(io::Error::other("only local DOS paths are supported")),
        },
        _ => return Err(io::Error::other("absolute DOS path required")),
    };
    if parts.next() != Some(Component::RootDir) {
        return Err(io::Error::other("rooted DOS path required"));
    }
    let mut result = vec![std::ffi::OsString::from(format!(
        "{}:",
        drive.to_ascii_lowercase() as char
    ))];
    for part in parts {
        match part {
            Component::Normal(name) => result.push(name.to_ascii_lowercase()),
            _ => {
                return Err(io::Error::other(
                    "path traversal or alias component rejected",
                ));
            }
        }
    }
    Ok(result)
}

#[cfg(not(windows))]
fn pin_directories(_: &Path) -> io::Result<Vec<File>> {
    Err(io::Error::other("text actions currently require Windows"))
}
#[cfg(not(windows))]
fn open_pinned_file(_: &Path, _: bool) -> io::Result<File> {
    Err(io::Error::other("text actions currently require Windows"))
}
#[cfg(not(windows))]
fn verify_file(_: &File) -> io::Result<()> {
    Err(io::Error::other("text actions currently require Windows"))
}
#[cfg(not(windows))]
fn verify_handle_path(_: &File, _: &Path) -> io::Result<PathBuf> {
    Err(io::Error::other("text actions currently require Windows"))
}
#[cfg(not(windows))]
fn path_within(_: &Path, _: &Path) -> io::Result<bool> {
    Err(io::Error::other("text actions currently require Windows"))
}

#[cfg(test)]
#[path = "../../tests/actions.rs"]
mod tests;
