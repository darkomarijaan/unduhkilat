//! File categories: extension classifier + default seed (FR-029–FR-032).

use crate::db::Db;
use crate::error::{AppError, Result};
use crate::models::Category;
use crate::repo;
use std::path::Path;

/// Lowercased extension of `file_name` (without dot), if any.
pub fn extension_of(file_name: &str) -> Option<String> {
    let name = file_name.rsplit(['/', '\\']).next().unwrap_or(file_name);
    let (stem, ext) = name.rsplit_once('.')?;
    if stem.is_empty() || ext.is_empty() {
        return None;
    }
    Some(ext.to_ascii_lowercase())
}

fn matches_ext(list: &str, ext: &str) -> bool {
    list.split(',').map(|t| t.trim().trim_start_matches('.').to_ascii_lowercase()).any(|t| t == ext)
}

/// Choose the category for `file_name`; unknown extension -> the default category (FR-030).
pub fn classify<'a>(file_name: &str, categories: &'a [Category]) -> Option<&'a Category> {
    if let Some(ext) = extension_of(file_name) {
        if let Some(c) = categories.iter().find(|c| !c.is_default && matches_ext(&c.file_extensions, &ext)) {
            return Some(c);
        }
    }
    categories.iter().find(|c| c.is_default)
}

fn validate_directory(dir: &str) -> Result<()> {
    let p = Path::new(dir);
    if dir.trim().is_empty() || !p.is_absolute() {
        return Err(crate::error::AppError::new(
            crate::error::ErrorCode::ErrInvalidPath,
            format!("{dir} is not an absolute path"),
        ));
    }
    Ok(())
}

fn new_category(name: &str, dir: &Path, exts: &str, is_default: bool) -> Category {
    let ts = repo::now();
    Category {
        id: uuid::Uuid::new_v4().to_string(),
        name: name.into(),
        target_directory: dir.to_string_lossy().into_owned(),
        file_extensions: exts.into(),
        is_default,
        created_at: ts.clone(),
        updated_at: ts,
    }
}

/// Insert the built-in categories under `base_dir` when none exist yet.
pub fn seed_defaults(db: &Db, base_dir: &Path) -> Result<()> {
    let base = base_dir.to_path_buf();
    db.write_blocking(move |conn| {
        if !repo::list_categories(conn)?.is_empty() {
            return Ok(());
        }
        let defs: [(&str, &str, bool); 7] = [
            ("Compressed", "zip,rar,7z,tar,gz,bz2,xz", false),
            ("Documents", "pdf,doc,docx,xls,xlsx,ppt,pptx,txt,epub,csv", false),
            ("Images", "iso,img,vhd,vhdx,vmdk,qcow2", false),
            ("Music", "mp3,flac,wav,aac,ogg,m4a,wma", false),
            ("Programs", "exe,msi,msix,apk", false),
            ("Video", "mp4,mkv,avi,mov,webm,flv,wmv,m4v", false),
            ("General", "*", true),
        ];
        for (name, exts, def) in defs {
            repo::insert_category(conn, &new_category(name, &base.join(name), exts, def))?;
        }
        Ok(())
    })
}

pub fn create(db: &Db, name: &str, dir: &str, exts: &str, is_default: bool) -> Result<Category> {
    if name.trim().is_empty() || exts.trim().is_empty() {
        return Err(AppError::invalid("category name and extension list are required"));
    }
    validate_directory(dir)?;
    let cat = new_category(name.trim(), Path::new(dir), exts, is_default);
    let c2 = cat.clone();
    db.write_blocking(move |conn| repo::insert_category(conn, &c2))?;
    Ok(cat)
}

pub fn update(db: &Db, cat: Category) -> Result<()> {
    validate_directory(&cat.target_directory)?;
    db.write_blocking(move |conn| repo::update_category(conn, &cat))
}

pub fn delete(db: &Db, id: &str) -> Result<()> {
    let id = id.to_string();
    db.write_blocking(move |conn| repo::delete_category(conn, &id))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cats() -> Vec<Category> {
        let base = Path::new("C:/dl");
        vec![
            new_category("Images", &base.join("Images"), "iso,img", false),
            new_category("Compressed", &base.join("Compressed"), "zip, .RAR,7z", false),
            new_category("General", &base.join("General"), "*", true),
        ]
    }

    #[test]
    fn classifies_by_extension_case_insensitively() {
        let c = cats();
        assert_eq!(classify("Ubuntu.ISO", &c).unwrap().name, "Images");
        assert_eq!(classify("a.b.rar", &c).unwrap().name, "Compressed");
    }

    #[test]
    fn unknown_or_missing_extension_goes_to_default() {
        let c = cats();
        assert_eq!(classify("a.xyz", &c).unwrap().name, "General");
        assert_eq!(classify("README", &c).unwrap().name, "General");
        assert_eq!(classify(".hidden", &c).unwrap().name, "General");
    }

    #[test]
    fn extension_of_handles_paths() {
        assert_eq!(extension_of("C:\\x\\y.Zip").as_deref(), Some("zip"));
        assert_eq!(extension_of("noext"), None);
    }

    #[test]
    fn crud_roundtrip_default_invariant_and_delete_rules() {
        let dir = std::env::temp_dir().join(format!("uk-cat-{}", uuid::Uuid::new_v4()));
        let db = Db::open(dir.join("t.db")).unwrap();
        seed_defaults(&db, Path::new("C:/dl")).unwrap();
        seed_defaults(&db, Path::new("C:/dl")).unwrap(); // idempotent
        let all = db.read(repo::list_categories).unwrap();
        assert_eq!(all.len(), 7);
        assert_eq!(all.iter().filter(|c| c.is_default).count(), 1);

        let c = create(&db, "Disk", "D:/ISOs", "iso", false).unwrap();
        assert_eq!(create(&db, "Disk", "D:/x", "iso", false).unwrap_err().code, crate::error::ErrorCode::ErrAlreadyExists);
        assert_eq!(create(&db, "Rel", "relative/dir", "x", false).unwrap_err().code, crate::error::ErrorCode::ErrInvalidPath);

        let mut c2 = c.clone();
        c2.is_default = true;
        update(&db, c2).unwrap();
        let all = db.read(repo::list_categories).unwrap();
        assert_eq!(all.iter().filter(|c| c.is_default).count(), 1, "single fallback invariant");
        assert!(delete(&db, &c.id).is_err(), "default cannot be deleted");
        drop(db);
        std::fs::remove_dir_all(dir).ok();
    }
}
