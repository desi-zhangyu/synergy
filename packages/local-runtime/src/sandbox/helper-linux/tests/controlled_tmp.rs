use std::path::{Path, PathBuf};
use synergy_sandbox_linux::controlled_tmp;

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "synergy-controlled-tmp-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&root).unwrap();
        Self(root)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn prepares_a_missing_session_directory_on_the_execution_host() {
    let fixture = Fixture::new();
    let temporary = fixture.0.join(".synergy/tmp/synergy-98-session");
    controlled_tmp::prepare(&fixture.0, Some(&temporary)).unwrap();
    std::fs::write(temporary.join("probe"), b"remote execution").unwrap();
    controlled_tmp::prepare(&fixture.0, Some(&temporary)).unwrap();
    assert_eq!(
        std::fs::read(temporary.join("probe")).unwrap(),
        b"remote execution"
    );
}

#[test]
fn does_not_create_an_external_or_non_session_temporary_directory() {
    let fixture = Fixture::new();
    let external = fixture.0.join("external");
    let nested = fixture.0.join(".synergy/tmp/synergy-98-session/nested");
    for temporary in [external.as_path(), nested.as_path(), Path::new("/tmp")] {
        controlled_tmp::prepare(&fixture.0, Some(temporary)).unwrap();
    }
    assert!(!external.exists());
    assert!(!nested.exists());
}

#[test]
fn refuses_symlinks_at_each_controlled_directory_component() {
    for component in [
        ".synergy",
        ".synergy/tmp",
        ".synergy/tmp/synergy-98-session",
    ] {
        let fixture = Fixture::new();
        let outside = fixture.0.join("outside");
        std::fs::create_dir(&outside).unwrap();
        let link = fixture.0.join(component);
        std::fs::create_dir_all(link.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(&outside, &link).unwrap();
        assert!(controlled_tmp::prepare(
            &fixture.0,
            Some(&fixture.0.join(".synergy/tmp/synergy-98-session"))
        )
        .is_err());
        assert_eq!(std::fs::read_dir(&outside).unwrap().count(), 0);
    }
}
