//! FR-035 / FR-062. The elevated paths cannot be exercised from an unelevated test run,
//! so the logic is tested through the unchecked entry points and the permission gate is
//! tested for refusal. If the test process IS elevated, the gate tests are skipped rather
//! than letting a test change machine-wide state.

use unduhkilat_core::admin;
use unduhkilat_core::db::Db;
use unduhkilat_core::error::ErrorCode;
use unduhkilat_core::native::{self, Status, Target};
use unduhkilat_core::paths::AppPaths;

fn temp(tag: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("uk-admin-{tag}-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn paths_in(root: &std::path::Path) -> AppPaths {
    let p = AppPaths { data_dir: root.to_path_buf(), state_dir: root.join("state"), logs_dir: root.join("logs"), portable: true };
    std::fs::create_dir_all(&p.state_dir).unwrap();
    std::fs::create_dir_all(&p.logs_dir).unwrap();
    p
}

#[test]
fn unelevated_callers_are_refused_before_anything_happens() {
    if admin::is_elevated() {
        eprintln!("skipped: test process is elevated");
        return;
    }
    let root = temp("deny");
    let paths = paths_in(&root);
    std::fs::write(paths.db_path(), b"db").unwrap();
    std::fs::write(paths.logs_dir.join("a.log"), b"log").unwrap();

    assert_eq!(admin::reset_data(&paths, true).unwrap_err().code, ErrorCode::ErrPermissionDenied);
    assert_eq!(admin::reset_data(&paths, false).unwrap_err().code, ErrorCode::ErrPermissionDenied, "permission is checked before the argument");
    assert!(paths.db_path().exists() && paths.logs_dir.join("a.log").exists(), "nothing deleted");

    let host = root.join("unduhkilat-host.exe");
    std::fs::write(&host, b"x").unwrap();
    assert_eq!(admin::register_host_machine_wide(&host).unwrap_err().code, ErrorCode::ErrPermissionDenied);
    assert!(!root.join("com.unduhkilat.host.json").exists(), "no manifest written either");
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn reset_removes_database_and_logs_but_never_downloaded_files() {
    let root = temp("reset");
    let paths = paths_in(&root);
    let downloads = temp("downloads");
    let keep = downloads.join("movie.mp4");
    std::fs::write(&keep, vec![7u8; 10_000]).unwrap();

    {
        let db = Db::open(paths.db_path()).unwrap();
        unduhkilat_core::category::seed_defaults(&db, &downloads).unwrap();
        db.write_blocking(|c| Ok(unduhkilat_core::repo::set_setting(c, "x", "y")?)).unwrap();
    }
    std::fs::write(paths.logs_dir.join("unduhkilat.log.2026-11-07"), vec![b'l'; 100_000]).unwrap();
    std::fs::write(paths.logs_dir.join("old.log"), b"old").unwrap();
    std::fs::write(format!("{}-wal", paths.db_path().display()), b"w").unwrap();
    std::fs::write(paths.state_dir.join("ipc.token"), b"t").unwrap();

    let r = admin::perform_reset(&paths).unwrap();
    assert!(r.database_purged && r.logs_purged);
    assert_eq!(r.downloaded_files_touched, 0);
    assert!(!paths.db_path().exists());
    assert!(!std::path::PathBuf::from(format!("{}-wal", paths.db_path().display())).exists());
    assert_eq!(std::fs::read_dir(&paths.logs_dir).unwrap().count(), 0, "logs emptied, directory kept");
    assert!(paths.state_dir.join("ipc.token").exists(), "unrelated state files are left alone");
    assert_eq!(std::fs::read(&keep).unwrap().len(), 10_000, "downloaded files untouched");

    // The app can start again from a clean slate, and a second reset is harmless.
    let db = Db::open(paths.db_path()).unwrap();
    assert!(db.read(unduhkilat_core::repo::list_categories).unwrap().is_empty());
    drop(db);
    assert!(admin::perform_reset(&paths).unwrap().database_purged);
    std::fs::remove_dir_all(root).ok();
    std::fs::remove_dir_all(downloads).ok();
}

#[test]
fn machine_registration_logic_writes_manifest_beside_the_binaries() {
    let root = temp("reg");
    let host = root.join("unduhkilat-host.exe");
    std::fs::write(&host, b"x").unwrap();
    let prefix = format!(r"Software\UnduhKilatTest\{}\", uuid::Uuid::new_v4());
    // Same code path as the HKLM command, pointed at a throwaway HKCU subtree.
    let t = Target { prefix: prefix.clone(), machine: false };
    let st = admin::register_host_in(&t, &host).unwrap();
    assert!(st.iter().all(|(_, s)| *s == Status::Registered), "{st:?}");
    let manifest = root.join("com.unduhkilat.host.json");
    assert!(manifest.exists(), "manifest sits next to the host so an in-place update keeps it valid");
    assert!(std::fs::read_to_string(&manifest).unwrap().contains(&native::allowed_origin()));
    native::__delete_test_prefix(&prefix);
    std::fs::remove_dir_all(root).ok();
}
