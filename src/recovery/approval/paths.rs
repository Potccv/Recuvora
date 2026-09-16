//! Filesystem identity checks for the durable approval store.
use super::ApprovalError;
use std::{fs::File, path::Path};

pub(super) fn validate_external_dir(dir: &Path) -> Result<(), ApprovalError> {
    validate_external_dir_for_source(dir, Path::new(env!("CARGO_MANIFEST_DIR")))
}

pub(super) fn validate_external_dir_for_source(
    dir: &Path,
    project: &Path,
) -> Result<(), ApprovalError> {
    if !dir.is_absolute()
        || dir
            .components()
            .any(|part| matches!(part, std::path::Component::ParentDir))
    {
        return Err(ApprovalError::Invalid(
            "data directory must be absolute without parent traversal",
        ));
    }
    for ancestor in dir.ancestors() {
        match std::fs::symlink_metadata(ancestor) {
            Ok(metadata) if linked(&metadata) => {
                return Err(ApprovalError::Invalid("linked approval directory ancestor"));
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    let mut existing = dir;
    while !existing.exists() {
        existing = existing
            .parent()
            .ok_or(ApprovalError::Invalid("missing data directory ancestor"))?;
    }
    let resolved = existing.canonicalize()?;
    // Installed binaries do not require the compiler's original source tree.
    // Other errors still fail closed instead of bypassing a source restriction.
    let project = match project.canonicalize() {
        Ok(project) => Some(project),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(error.into()),
    };
    if project.is_some_and(|project| resolved.starts_with(project)) {
        return Err(ApprovalError::Invalid(
            "data directory must be outside project source",
        ));
    }
    Ok(())
}

pub(super) fn reject_nonregular(path: &Path) -> Result<(), ApprovalError> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if !metadata.file_type().is_file() || linked(&metadata) => Err(
            ApprovalError::Invalid("journal and lock must be regular files"),
        ),
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

fn linked(metadata: &std::fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if metadata.file_attributes() & 0x400 != 0 {
            return true;
        }
    }
    metadata.file_type().is_symlink()
}

pub(super) fn validate_open_file(file: &File) -> Result<(), ApprovalError> {
    let metadata = file.metadata()?;
    if !metadata.is_file() || linked(&metadata) {
        return Err(ApprovalError::Invalid(
            "approval state must be a regular unlinked file",
        ));
    }
    #[cfg(windows)]
    {
        if winapi_util::file::information(file)?.number_of_links() != 1 {
            return Err(ApprovalError::Invalid(
                "hard-linked approval state is forbidden",
            ));
        }
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if metadata.nlink() != 1 {
            return Err(ApprovalError::Invalid(
                "hard-linked approval state is forbidden",
            ));
        }
    }
    Ok(())
}
