//! Listing every file under a folder, so a whole folder can be queued as one transfer per file.
//! Symbolic links are never followed (they could loop or lead outside the folder), and the walk
//! stops with an error instead of queuing an unreasonable number of files.

use super::{join_remote, Entry, RemoteFs};
use crate::error::{AppError, ErrorCode, Result};
use std::path::{Path, PathBuf};

pub const MAX_FILES: usize = 20_000;
pub const MAX_DEPTH: usize = 32;

/// A file found under the folder that was walked. `rel_dir` is the folder it sits in, relative to
/// the folder's parent and starting with the folder's own name (`Photos/2024`), so copying
/// `Photos` to somewhere keeps the `Photos` folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found<T> {
    pub rel_dir: Vec<String>,
    pub item: T,
}

fn too_many() -> AppError {
    AppError::new(ErrorCode::ErrInvalidArgument, format!("Folder berisi lebih dari {MAX_FILES} berkas atau terlalu dalam. Salin bagian yang lebih kecil dulu."))
}

/// Every file (not folder, not link) under the remote folder `root`.
pub async fn walk_remote(fs: &mut dyn RemoteFs, root: &str) -> Result<Vec<Found<Entry>>> {
    let root_name = root.trim_end_matches('/').rsplit('/').next().unwrap_or("").to_string();
    if root_name.is_empty() {
        return Err(AppError::new(ErrorCode::ErrInvalidPath, "Nama folder kosong."));
    }
    let mut out = Vec::new();
    let mut stack = vec![(root.to_string(), vec![root_name])];
    while let Some((dir, rel)) = stack.pop() {
        if rel.len() > MAX_DEPTH {
            return Err(too_many());
        }
        // Name the folder that failed: a bare "tidak ditemukan" does not say which one inside a big tree.
        let listing = fs.list(&dir).await.map_err(|e| AppError::new(e.code, format!("{dir}: {}", e.message)))?;
        for e in listing {
            // Links are not followed; sockets, FIFOs and devices have nothing to copy.
            if e.is_link || e.is_special {
                continue;
            }
            if e.is_dir {
                let mut sub = rel.clone();
                sub.push(e.name.clone());
                stack.push((join_remote(&dir, &e.name), sub));
            } else {
                out.push(Found { rel_dir: rel.clone(), item: e });
                if out.len() > MAX_FILES {
                    return Err(too_many());
                }
            }
        }
    }
    out.sort_by(|a, b| (&a.rel_dir, &a.item.name).cmp(&(&b.rel_dir, &b.item.name)));
    Ok(out)
}

/// Every file (not folder, not link) under the local folder `root`.
pub fn walk_local(root: &Path) -> Result<Vec<Found<PathBuf>>> {
    let root_name = root.file_name().map(|n| n.to_string_lossy().into_owned()).ok_or_else(|| AppError::new(ErrorCode::ErrInvalidPath, "Nama folder kosong."))?;
    let mut out = Vec::new();
    let mut stack = vec![(root.to_path_buf(), vec![root_name])];
    while let Some((dir, rel)) = stack.pop() {
        if rel.len() > MAX_DEPTH {
            return Err(too_many());
        }
        let rd = std::fs::read_dir(&dir).map_err(|e| AppError::new(ErrorCode::ErrPathUnwritable, format!("{}: {e}", dir.display())))?;
        for entry in rd.flatten() {
            let Ok(ft) = entry.file_type() else { continue };
            if ft.is_symlink() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            if ft.is_dir() {
                let mut sub = rel.clone();
                sub.push(name);
                stack.push((entry.path(), sub));
            } else if ft.is_file() {
                out.push(Found { rel_dir: rel.clone(), item: entry.path() });
                if out.len() > MAX_FILES {
                    return Err(too_many());
                }
            }
        }
    }
    out.sort_by(|a, b| (&a.rel_dir, &a.item).cmp(&(&b.rel_dir, &b.item)));
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::fs::{RemoteReader, RemoteWriter};
    use async_trait::async_trait;
    use std::collections::BTreeMap;

    /// Folders and their entries, nothing else.
    struct Tree(BTreeMap<String, Vec<Entry>>);

    fn e(dir: &str, name: &str, is_dir: bool, is_link: bool) -> Entry {
        Entry { name: name.into(), path: join_remote(dir, name), is_dir, size: 1, modified: None, is_link, is_special: false }
    }

    #[async_trait]
    impl RemoteFs for Tree {
        async fn list(&mut self, d: &str) -> Result<Vec<Entry>> {
            self.0.get(d).cloned().ok_or_else(|| AppError::new(ErrorCode::ErrNotFound, "tidak ada"))
        }
        async fn stat(&mut self, _p: &str) -> Result<Entry> {
            unimplemented!()
        }
        async fn mkdir(&mut self, _p: &str) -> Result<()> {
            unimplemented!()
        }
        async fn rename(&mut self, _a: &str, _b: &str) -> Result<()> {
            unimplemented!()
        }
        async fn remove(&mut self, _p: &str, _d: bool) -> Result<()> {
            unimplemented!()
        }
        async fn read_at(&mut self, _p: &str, _o: u64, _l: usize) -> Result<Vec<u8>> {
            unimplemented!()
        }
        async fn open_read(&mut self, _p: &str, _o: u64) -> Result<RemoteReader> {
            unimplemented!()
        }
        async fn open_write(&mut self, _p: &str, _o: u64, _a: bool) -> Result<RemoteWriter> {
            unimplemented!()
        }
        async fn home(&mut self) -> Result<String> {
            Ok("/".into())
        }
        async fn close(self: Box<Self>) {}
    }

    #[tokio::test]
    async fn sockets_fifos_and_devices_are_skipped_not_walked_into() {
        let mut sock = e("/p/app", "daemon.sock", false, false);
        sock.is_special = true;
        let mut m = BTreeMap::new();
        m.insert("/p/app".to_string(), vec![e("/p/app", "main.rs", false, false), sock]);
        let found = walk_remote(&mut Tree(m), "/p/app").await.unwrap();
        let names: Vec<&str> = found.iter().map(|f| f.item.name.as_str()).collect();
        assert_eq!(names, ["main.rs"]);
    }

    #[tokio::test]
    async fn walk_lists_files_under_the_folder_with_relative_folders_and_skips_links() {
        let mut m = BTreeMap::new();
        m.insert("/data/Photos".to_string(), vec![e("/data/Photos", "a.jpg", false, false), e("/data/Photos", "2024", true, false), e("/data/Photos", "loop", true, true), e("/data/Photos", "shortcut.jpg", false, true)]);
        m.insert("/data/Photos/2024".to_string(), vec![e("/data/Photos/2024", "b.jpg", false, false), e("/data/Photos/2024", "empty", true, false)]);
        m.insert("/data/Photos/2024/empty".to_string(), vec![]);
        let found = walk_remote(&mut Tree(m), "/data/Photos").await.unwrap();
        let got: Vec<(String, String)> = found.iter().map(|f| (f.rel_dir.join("/"), f.item.name.clone())).collect();
        assert_eq!(got, [("Photos".to_string(), "a.jpg".to_string()), ("Photos/2024".to_string(), "b.jpg".to_string())]);
    }

    #[tokio::test]
    async fn a_folder_that_cannot_be_listed_is_named_in_the_error() {
        let mut m = BTreeMap::new();
        m.insert("/data/Photos".to_string(), vec![e("/data/Photos", "2024", true, false)]);
        let err = walk_remote(&mut Tree(m), "/data/Photos").await.unwrap_err();
        assert_eq!(err.code, ErrorCode::ErrNotFound);
        assert!(err.message.contains("/data/Photos/2024"), "{}", err.message);
    }

    #[tokio::test]
    async fn a_folder_with_too_many_files_is_refused() {
        let files: Vec<Entry> = (0..=MAX_FILES).map(|i| e("/big", &format!("f{i}"), false, false)).collect();
        let mut m = BTreeMap::new();
        m.insert("/big".to_string(), files);
        assert_eq!(walk_remote(&mut Tree(m), "/big").await.unwrap_err().code, ErrorCode::ErrInvalidArgument);
    }

    #[test]
    fn local_walk_keeps_the_folder_name_and_skips_nothing_real() {
        let root = std::env::temp_dir().join(format!("uk-walk-{}", uuid::Uuid::new_v4())).join("Proyek");
        std::fs::create_dir_all(root.join("src/deep")).unwrap();
        std::fs::write(root.join("a.txt"), b"1").unwrap();
        std::fs::write(root.join("src/deep/b.txt"), b"2").unwrap();
        std::fs::create_dir_all(root.join("kosong")).unwrap();
        let found = walk_local(&root).unwrap();
        let got: Vec<String> = found.iter().map(|f| format!("{}|{}", f.rel_dir.join("/"), f.item.file_name().unwrap().to_string_lossy())).collect();
        assert_eq!(got, ["Proyek|a.txt", "Proyek/src/deep|b.txt"]);
        let _ = std::fs::remove_dir_all(root.parent().unwrap());
    }
}
