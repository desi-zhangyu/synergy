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
