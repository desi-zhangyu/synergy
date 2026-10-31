use std::ffi::CString;
use std::fs::{File, OpenOptions};
use std::io;
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;

/// Runs as the execution user, after its Workspace writer has been acquired.
pub fn prepare(workspace: &Path, temporary: Option<&Path>) -> io::Result<()> {
    let workspace_dir = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(workspace)?;
    let metadata = directory(&workspace_dir, ".synergy")?;
    let base = directory(&metadata, "tmp")?;
    let base_path = workspace.join(".synergy").join("tmp");
    if let Some(temporary) = temporary.filter(|value| value.parent() == Some(base_path.as_path())) {
        if let Some(name) = temporary.file_name().and_then(|value| value.to_str()) {
            if name.starts_with("synergy-")
                && name
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_')
            {
                directory(&base, name)?;
            }
        }
    }
    Ok(())
}

fn directory(parent: &File, name: &str) -> io::Result<File> {
    let name = CString::new(name)?;
    // Keep every lookup relative to an opened directory; never follow a
    // workspace-controlled symlink while preparing a path before bwrap.
    if unsafe { libc::mkdirat(parent.as_raw_fd(), name.as_ptr(), 0o700) } != 0 {
        let error = io::Error::last_os_error();
        if error.kind() != io::ErrorKind::AlreadyExists {
            return Err(error);
        }
    }
    let fd = unsafe {
        libc::openat(
            parent.as_raw_fd(),
            name.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
        )
    };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(unsafe { File::from_raw_fd(fd) })
}

#[cfg(test)]
mod tests {
    use super::prepare;
    use std::fs;
    use std::os::unix::fs::symlink;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static NEXT: AtomicUsize = AtomicUsize::new(0);

    struct Workspace(PathBuf);
    impl Workspace {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "synergy-controlled-tmp-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::SeqCst)
            ));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for Workspace {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).unwrap();
        }
    }

    #[test]
    fn prepares_selected_session_directory_idempotently() {
        let workspace = Workspace::new();
        let temporary = workspace.0.join(".synergy/tmp/synergy-session_1");
        prepare(&workspace.0, Some(&temporary)).unwrap();
        fs::write(temporary.join("retained"), "data").unwrap();
        prepare(&workspace.0, Some(&temporary)).unwrap();
        assert_eq!(fs::read_to_string(temporary.join("retained")).unwrap(), "data");
    }

    #[test]
    fn does_not_create_unrelated_temporary_paths() {
        let workspace = Workspace::new();
        let other = workspace.0.join("other/synergy-session");
        prepare(&workspace.0, Some(&other)).unwrap();
        assert!(!other.exists());
        let nested = workspace.0.join(".synergy/tmp/nested/synergy-session");
        prepare(&workspace.0, Some(&nested)).unwrap();
        assert!(!nested.exists());
    }

    #[test]
    fn rejects_intermediate_and_session_symlinks() {
        for component in [".synergy", ".synergy/tmp", ".synergy/tmp/synergy-session"] {
            let workspace = Workspace::new();
            let outside = Workspace::new();
            let link = workspace.0.join(component);
            fs::create_dir_all(link.parent().unwrap()).unwrap();
            symlink(&outside.0, &link).unwrap();
            let temporary = workspace.0.join(".synergy/tmp/synergy-session");
            assert!(prepare(&workspace.0, Some(&temporary)).is_err());
            assert_eq!(fs::read_dir(&outside.0).unwrap().count(), 0);
        }
    }
}
