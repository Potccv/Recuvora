fn main() -> Result<(), Box<dyn std::error::Error>> {
    #[cfg(feature = "desktop-ui")]
    build_desktop()?;
    Ok(())
}

#[cfg(feature = "desktop-ui")]
fn build_desktop() -> Result<(), Box<dyn std::error::Error>> {
    use std::{env, fs, io, path::Path};

    fn regular_directory(path: &Path) -> io::Result<()> {
        let metadata = fs::symlink_metadata(path)?;
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            return Err(io::Error::other(
                "Tauri staging requires a regular directory",
            ));
        }
        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt;
            if metadata.file_attributes() & 0x400 != 0 {
                return Err(io::Error::other("linked Tauri staging is not supported"));
            }
        }
        Ok(())
    }

    fn stage_input(source: &Path, destination: &Path) -> io::Result<()> {
        println!("cargo:rerun-if-changed={}", source.display());
        let contents = fs::read(source)?;
        match fs::symlink_metadata(destination) {
            Ok(metadata) => {
                if !metadata.is_file() || metadata.file_type().is_symlink() {
                    return Err(io::Error::other(
                        "Tauri staged input must be a regular file",
                    ));
                }
                if fs::read(destination)? == contents {
                    return Ok(());
                }
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        fs::write(destination, contents)
    }

    let project =
        Path::new(&env::var_os("CARGO_MANIFEST_DIR").ok_or("missing manifest directory")?)
            .canonicalize()?;
    let output = Path::new(&env::var_os("OUT_DIR").ok_or("missing Cargo output directory")?)
        .canonicalize()?;
    if output.starts_with(&project) {
        return Err(
            "Desktop build output must be outside the source tree; set CARGO_TARGET_DIR".into(),
        );
    }
    let staging = output.join("tauri-build-stage");
    fs::create_dir_all(&staging)?;
    regular_directory(&staging)?;
    if !staging.canonicalize()?.starts_with(&output) {
        return Err("Tauri staging escaped the Cargo output directory".into());
    }

    // tauri-build 2.6 reads these from cwd and writes gen/schemas there before
    // copying ACL output to OUT_DIR. Stage its build inputs, not a second package
    // or UI source. generate_context! still embeds the original shared public/.
    for name in ["Cargo.toml", "tauri.conf.json"] {
        stage_input(&project.join(name), &staging.join(name))?;
    }
    let icon = project.join("src/interfaces/ui/public/favicon.ico");
    println!("cargo:rerun-if-changed={}", icon.display());
    // This shell currently has no native commands, custom ACL or platform config.
    // Fail explicitly if those inputs are added without extending the staging.
    for name in [
        "permissions",
        "capabilities",
        "tauri.windows.conf.json",
        "tauri.macos.conf.json",
        "tauri.linux.conf.json",
    ] {
        println!("cargo:rerun-if-changed={}", project.join(name).display());
        if project.join(name).exists() {
            return Err(format!("Tauri build staging must include new input: {name}").into());
        }
    }
    let original_directory = env::current_dir()?;
    env::set_current_dir(&staging)?;
    let result = tauri_build::try_build(
        tauri_build::Attributes::new()
            .windows_attributes(tauri_build::WindowsAttributes::new().window_icon_path(icon)),
    );
    env::set_current_dir(original_directory)?;
    result?;
    Ok(())
}
