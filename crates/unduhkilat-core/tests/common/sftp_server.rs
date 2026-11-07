//! SFTP subsystem handler for the in-process test SSH server: serves a real temporary folder as `/`.

#![allow(dead_code)]

use russh_sftp::protocol::{Attrs, Data, File, FileAttributes, Handle, Name, OpenFlags, Status, StatusCode, Version};
use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};

enum Open {
    File(tokio::fs::File),
    Dir(Vec<File>, bool),
}

pub struct FolderSftp {
    root: PathBuf,
    handles: HashMap<String, Open>,
    next: u64,
}

impl FolderSftp {
    pub fn new(root: PathBuf) -> Self {
        FolderSftp { root, handles: HashMap::new(), next: 1 }
    }

    /// Map a client path (always `/`-rooted) into the folder; `..` is refused.
    fn local(&self, p: &str) -> Result<PathBuf, StatusCode> {
        let mut out = self.root.clone();
        for part in Path::new(p.trim_start_matches('/')).components() {
            match part {
                Component::Normal(n) => out.push(n),
                Component::CurDir => {}
                _ => return Err(StatusCode::PermissionDenied),
            }
        }
        Ok(out)
    }

    fn handle(&mut self, o: Open) -> String {
        let h = format!("h{}", self.next);
        self.next += 1;
        self.handles.insert(h.clone(), o);
        h
    }
}

fn status(id: u32) -> Status {
    Status { id, status_code: StatusCode::Ok, error_message: "Ok".into(), language_tag: "en-US".into() }
}

fn map_io(e: std::io::Error) -> StatusCode {
    match e.kind() {
        std::io::ErrorKind::NotFound => StatusCode::NoSuchFile,
        std::io::ErrorKind::PermissionDenied => StatusCode::PermissionDenied,
        _ => StatusCode::Failure,
    }
}

fn attrs_of(m: &std::fs::Metadata) -> FileAttributes {
    FileAttributes::from(m)
}

impl russh_sftp::server::Handler for FolderSftp {
    type Error = StatusCode;

    fn unimplemented(&self) -> Self::Error {
        StatusCode::OpUnsupported
    }

    async fn init(&mut self, _v: u32, _e: HashMap<String, String>) -> Result<Version, Self::Error> {
        Ok(Version::new())
    }

    async fn open(&mut self, id: u32, filename: String, pflags: OpenFlags, _a: FileAttributes) -> Result<Handle, Self::Error> {
        let path = self.local(&filename)?;
        let mut o = tokio::fs::OpenOptions::new();
        o.read(pflags.contains(OpenFlags::READ)).write(pflags.contains(OpenFlags::WRITE)).create(pflags.contains(OpenFlags::CREATE)).truncate(pflags.contains(OpenFlags::TRUNCATE));
        if pflags.contains(OpenFlags::APPEND) {
            o.append(true);
        }
        let f = o.open(path).await.map_err(map_io)?;
        Ok(Handle { id, handle: self.handle(Open::File(f)) })
    }

    async fn close(&mut self, id: u32, handle: String) -> Result<Status, Self::Error> {
        if let Some(Open::File(mut f)) = self.handles.remove(&handle) {
            let _ = f.flush().await;
        }
        Ok(status(id))
    }

    async fn read(&mut self, id: u32, handle: String, offset: u64, len: u32) -> Result<Data, Self::Error> {
        let Some(Open::File(f)) = self.handles.get_mut(&handle) else { return Err(StatusCode::Failure) };
        f.seek(std::io::SeekFrom::Start(offset)).await.map_err(map_io)?;
        let mut buf = vec![0u8; len as usize];
        let n = f.read(&mut buf).await.map_err(map_io)?;
        if n == 0 {
            return Err(StatusCode::Eof);
        }
        buf.truncate(n);
        Ok(Data { id, data: buf })
    }

    async fn write(&mut self, id: u32, handle: String, offset: u64, data: Vec<u8>) -> Result<Status, Self::Error> {
        let Some(Open::File(f)) = self.handles.get_mut(&handle) else { return Err(StatusCode::Failure) };
        f.seek(std::io::SeekFrom::Start(offset)).await.map_err(map_io)?;
        f.write_all(&data).await.map_err(map_io)?;
        Ok(status(id))
    }

    async fn lstat(&mut self, id: u32, path: String) -> Result<Attrs, Self::Error> {
        let m = std::fs::symlink_metadata(self.local(&path)?).map_err(map_io)?;
        Ok(Attrs { id, attrs: attrs_of(&m) })
    }

    async fn stat(&mut self, id: u32, path: String) -> Result<Attrs, Self::Error> {
        let m = std::fs::metadata(self.local(&path)?).map_err(map_io)?;
        Ok(Attrs { id, attrs: attrs_of(&m) })
    }

    async fn fstat(&mut self, id: u32, handle: String) -> Result<Attrs, Self::Error> {
        let Some(Open::File(f)) = self.handles.get(&handle) else { return Err(StatusCode::Failure) };
        let m = f.metadata().await.map_err(map_io)?;
        Ok(Attrs { id, attrs: attrs_of(&m) })
    }

    async fn opendir(&mut self, id: u32, path: String) -> Result<Handle, Self::Error> {
        let dir = self.local(&path)?;
        let mut files = Vec::new();
        for e in std::fs::read_dir(dir).map_err(map_io)? {
            let e = e.map_err(map_io)?;
            let m = e.metadata().map_err(map_io)?;
            files.push(File::new(e.file_name().to_string_lossy().into_owned(), attrs_of(&m)));
        }
        Ok(Handle { id, handle: self.handle(Open::Dir(files, false)) })
    }

    async fn readdir(&mut self, id: u32, handle: String) -> Result<Name, Self::Error> {
        let Some(Open::Dir(files, done)) = self.handles.get_mut(&handle) else { return Err(StatusCode::Failure) };
        if *done {
            return Err(StatusCode::Eof);
        }
        *done = true;
        Ok(Name { id, files: std::mem::take(files) })
    }

    async fn remove(&mut self, id: u32, filename: String) -> Result<Status, Self::Error> {
        std::fs::remove_file(self.local(&filename)?).map_err(map_io)?;
        Ok(status(id))
    }

    async fn mkdir(&mut self, id: u32, path: String, _a: FileAttributes) -> Result<Status, Self::Error> {
        std::fs::create_dir(self.local(&path)?).map_err(map_io)?;
        Ok(status(id))
    }

    async fn rmdir(&mut self, id: u32, path: String) -> Result<Status, Self::Error> {
        std::fs::remove_dir(self.local(&path)?).map_err(map_io)?;
        Ok(status(id))
    }

    async fn realpath(&mut self, id: u32, path: String) -> Result<Name, Self::Error> {
        let mut parts: Vec<&str> = Vec::new();
        for p in path.split('/') {
            match p {
                "" | "." => {}
                ".." => {
                    parts.pop();
                }
                other => parts.push(other),
            }
        }
        Ok(Name { id, files: vec![File::dummy(format!("/{}", parts.join("/")))] })
    }

    async fn rename(&mut self, id: u32, oldpath: String, newpath: String) -> Result<Status, Self::Error> {
        std::fs::rename(self.local(&oldpath)?, self.local(&newpath)?).map_err(map_io)?;
        Ok(status(id))
    }
}
