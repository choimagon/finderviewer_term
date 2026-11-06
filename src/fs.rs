use anyhow::{bail, Context, Result};
use std::{
    fs,
    io::{self, Read, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Sort {
    Folders,
    Name,
    Recent,
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Policy {
    Error,
    Overwrite,
    Skip,
}
#[derive(Clone, Debug)]
pub struct Entry {
    pub path: PathBuf,
    pub name: String,
    pub is_dir: bool,
    pub size: u64,
    pub modified: SystemTime,
    pub is_symlink: bool,
}
#[derive(Clone, Debug)]
pub struct Manager {
    pub root: PathBuf,
}
pub fn target_exists(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok()
}
fn name(value: &str) -> Result<&str> {
    if value.trim().is_empty()
        || value == "."
        || value == ".."
        || value.contains('\0')
        || Path::new(value).file_name() != Some(value.as_ref())
        || value.contains('/')
        || cfg!(windows) && value.chars().any(|c| "<>:\"/\\|?*".contains(c))
    {
        bail!("경로 대신 유효한 이름만 입력하세요.");
    }
    Ok(value)
}
fn expand(path: &Path) -> PathBuf {
    if let Ok(rest) = path.strip_prefix("~") {
        if let Some(home) = std::env::var_os("HOME") {
            return PathBuf::from(home).join(rest);
        }
    }
    path.to_path_buf()
}
static COUNTER: AtomicU64 = AtomicU64::new(0);
fn stage(parent: &Path) -> PathBuf {
    parent.join(format!(
        ".finderview-stage-{}-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ))
}
#[cfg(unix)]
fn same(a: &fs::Metadata, b: &fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;
    a.dev() == b.dev() && a.ino() == b.ino()
}
#[cfg(not(unix))]
fn same(_a: &fs::Metadata, _b: &fs::Metadata) -> bool {
    false
}
fn signature(m: &fs::Metadata) -> (u64, SystemTime, bool) {
    (m.len(), m.modified().unwrap_or(UNIX_EPOCH), m.is_dir())
}
fn replace(path: &Path, expected: Option<fs::Metadata>) -> Result<()> {
    match expected {
        None => {
            if target_exists(path) {
                bail!("대상이 변경되었습니다. 다시 시도하세요.");
            }
        }
        Some(previous) => {
            let current = fs::symlink_metadata(path)?;
            if signature(&previous) != signature(&current)
                || cfg!(unix) && !same(&previous, &current)
            {
                bail!("덮어쓰기 확인 후 대상이 변경되었습니다.");
            }
            trash::delete(path).context("기존 항목을 휴지통으로 보내지 못했습니다.")?;
        }
    }
    Ok(())
}
#[cfg(any(target_os = "macos", target_os = "linux"))]
fn exclusive(source: &Path, target: &Path) -> Result<()> {
    use std::{ffi::CString, os::unix::ffi::OsStrExt};
    let s = CString::new(source.as_os_str().as_bytes())?;
    let t = CString::new(target.as_os_str().as_bytes())?;
    #[cfg(target_os = "macos")]
    let result = unsafe { libc::renamex_np(s.as_ptr(), t.as_ptr(), libc::RENAME_EXCL) };
    #[cfg(target_os = "linux")]
    let result = unsafe {
        libc::renameat2(
            libc::AT_FDCWD,
            s.as_ptr(),
            libc::AT_FDCWD,
            t.as_ptr(),
            libc::RENAME_NOREPLACE,
        )
    };
    if result != 0 {
        return Err(io::Error::last_os_error()).context(
            "덮어쓰기 방지 이동 실패 (다른 디스크는 복사 후 원본을 휴지통으로 이동하세요)",
        );
    }
    Ok(())
}
#[cfg(windows)]
fn exclusive(source: &Path, target: &Path) -> Result<()> {
    fs::rename(source, target)?;
    Ok(())
}
#[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
fn exclusive(_source: &Path, _target: &Path) -> Result<()> {
    bail!("이 환경은 안전한 이동을 지원하지 않습니다.")
}
fn copy_tree(source: &Path, target: &Path) -> Result<()> {
    let meta = fs::symlink_metadata(source)?;
    if meta.file_type().is_symlink() {
        #[cfg(unix)]
        std::os::unix::fs::symlink(fs::read_link(source)?, target)?;
        #[cfg(windows)]
        {
            if source.is_dir() {
                std::os::windows::fs::symlink_dir(fs::read_link(source)?, target)?;
            } else {
                std::os::windows::fs::symlink_file(fs::read_link(source)?, target)?;
            }
        }
    } else if meta.is_dir() {
        fs::create_dir(target)?;
        for entry in fs::read_dir(source)? {
            let entry = entry?;
            copy_tree(&entry.path(), &target.join(entry.file_name()))?;
        }
        fs::set_permissions(target, meta.permissions())?;
    } else if meta.is_file() {
        let mut input = fs::File::open(source)?;
        let mut output = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(target)?;
        let mut buffer = vec![0u8; 1024 * 1024];
        loop {
            let n = input.read(&mut buffer)?;
            if n == 0 {
                break;
            }
            output.write_all(&buffer[..n])?;
        }
        output.sync_all()?;
        fs::set_permissions(target, meta.permissions())?;
    } else {
        bail!("일반 파일·폴더·심볼릭 링크만 복사할 수 있습니다.");
    }
    Ok(())
}
fn cleanup(path: &Path) {
    if let Ok(meta) = fs::symlink_metadata(path) {
        if meta.is_dir() && !meta.file_type().is_symlink() {
            let _ = fs::remove_dir_all(path);
        } else {
            let _ = fs::remove_file(path);
        }
    }
}
impl Manager {
    pub fn new(root: impl AsRef<Path>) -> Result<Self> {
        let root = expand(root.as_ref()).canonicalize()?;
        if !root.is_dir() {
            bail!("폴더 경로를 선택하세요.");
        }
        Ok(Self { root })
    }
    fn absolute(&self, path: &Path) -> PathBuf {
        let path = expand(path);
        if path.is_absolute() {
            path
        } else {
            self.root.join(path)
        }
    }
    pub fn validate(&self, path: &Path, directory: bool) -> Result<PathBuf> {
        let resolved = self.absolute(path).canonicalize()?;
        if !resolved.starts_with(&self.root) {
            bail!("선택한 시작 폴더 밖으로는 이동할 수 없습니다.");
        }
        if directory && !resolved.is_dir() {
            bail!("폴더가 아닙니다.");
        }
        Ok(resolved)
    }
    fn mutable(&self, path: &Path, external: bool) -> Result<PathBuf> {
        let absolute = self.absolute(path);
        let filename = absolute
            .file_name()
            .context("시작 폴더 자체는 변경할 수 없습니다.")?;
        let candidate = absolute
            .parent()
            .context("상위 폴더 없음")?
            .canonicalize()?
            .join(filename);
        if candidate == self.root {
            bail!("시작 폴더 자체는 변경할 수 없습니다.");
        }
        if !external && !candidate.starts_with(&self.root) {
            bail!("시작 폴더 밖 항목은 변경할 수 없습니다.");
        }
        fs::symlink_metadata(&candidate)?;
        Ok(candidate)
    }
    pub fn list(&self, path: &Path, hidden: bool, sort: Sort) -> Result<Vec<Entry>> {
        let directory = self.validate(path, true)?;
        let mut entries = Vec::new();
        for item in fs::read_dir(directory)? {
            let item = item?;
            let path = item.path();
            let name = item.file_name().to_string_lossy().into_owned();
            if !hidden && name.starts_with('.') {
                continue;
            }
            let meta = match fs::symlink_metadata(&path) {
                Ok(m) => m,
                Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
                Err(e) => return Err(e.into()),
            };
            entries.push(Entry {
                is_dir: path.is_dir(),
                path,
                name,
                size: meta.len(),
                modified: meta.modified().unwrap_or(UNIX_EPOCH),
                is_symlink: meta.file_type().is_symlink(),
            });
        }
        entries.sort_by(|a, b| {
            let names = || {
                a.name
                    .to_lowercase()
                    .cmp(&b.name.to_lowercase())
                    .then(a.name.cmp(&b.name))
            };
            match sort {
                Sort::Folders => b.is_dir.cmp(&a.is_dir).then_with(names),
                Sort::Name => names(),
                Sort::Recent => b.modified.cmp(&a.modified).then_with(names),
            }
        });
        Ok(entries)
    }
    pub fn transfer(
        &self,
        source: &Path,
        directory: &Path,
        copy: bool,
        policy: Policy,
        new_name: Option<&str>,
        external: bool,
    ) -> Result<Option<PathBuf>> {
        if external && !copy {
            bail!("외부 가져오기는 복사만 가능합니다.");
        }
        let source = self.mutable(source, external)?;
        let directory = self.validate(directory, true)?;
        let filename = match new_name {
            Some(value) => std::ffi::OsStr::new(name(value)?),
            None => source.file_name().context("원본 이름 없음")?,
        };
        let target = directory.join(filename);
        if source == target {
            return Ok(None);
        }
        if source.starts_with(&target) || target == self.root {
            bail!("원본 또는 원본의 상위 폴더를 덮어쓸 수 없습니다.");
        }
        let meta = fs::symlink_metadata(&source)?;
        if meta.is_dir() && !meta.file_type().is_symlink() && directory.starts_with(&source) {
            bail!("폴더를 자기 하위 폴더로 복사·이동할 수 없습니다.");
        }
        let expected = fs::symlink_metadata(&target).ok();
        if let Some(ref existing) = expected {
            if same(&meta, existing) {
                bail!("원본 자체를 덮어쓸 수 없습니다.");
            }
            match policy {
                Policy::Skip => return Ok(None),
                Policy::Error => bail!("같은 이름이 이미 있습니다: {}", target.display()),
                Policy::Overwrite => {}
            }
        }
        #[cfg(unix)]
        if !copy {
            use std::os::unix::fs::MetadataExt;
            if meta.dev() != fs::metadata(&directory)?.dev() {
                bail!("다른 디스크 이동은 복사 후 원본을 휴지통으로 이동하세요.");
            }
        }
        if copy {
            let temporary = stage(&directory);
            fs::create_dir(&temporary)?;
            let item = temporary.join("item");
            let operation: Result<()> = (|| {
                copy_tree(&source, &item)?;
                self.validate(&directory, true)?;
                replace(&target, expected)?;
                exclusive(&item, &target)
            })();
            cleanup(&temporary);
            operation?;
        } else {
            self.mutable(&source, external)?;
            replace(&target, expected)?;
            exclusive(&source, &target)?;
        }
        Ok(Some(target))
    }
    pub fn rename(&self, source: &Path, new_name: &str, overwrite: bool) -> Result<PathBuf> {
        let original = self.mutable(source, false)?;
        let target = original.parent().unwrap().join(name(new_name)?);
        if original == target {
            return Ok(original);
        }
        if let Ok(existing) = fs::symlink_metadata(&target) {
            if same(&fs::symlink_metadata(&original)?, &existing) {
                let temporary = stage(original.parent().unwrap());
                exclusive(&original, &temporary)?;
                if let Err(error) = exclusive(&temporary, &target) {
                    let rollback = exclusive(&temporary, &original);
                    if rollback.is_err() {
                        bail!(
                            "이름 변경 및 복원 실패: 임시 항목 {} / {error}",
                            temporary.display()
                        );
                    }
                    return Err(error);
                }
                return Ok(target);
            }
        }
        self.transfer(
            &original,
            original.parent().unwrap(),
            false,
            if overwrite {
                Policy::Overwrite
            } else {
                Policy::Error
            },
            Some(new_name),
            false,
        )?;
        Ok(target)
    }
    pub fn trash(&self, paths: &[PathBuf]) -> Result<usize> {
        let mut selected = Vec::new();
        for path in paths {
            let path = self.mutable(path, false)?;
            if !selected.contains(&path) {
                selected.push(path);
            }
        }
        let snapshot = selected.clone();
        selected.retain(|path| {
            !snapshot.iter().any(|parent| {
                parent != path
                    && path.starts_with(parent)
                    && fs::symlink_metadata(parent)
                        .map(|m| m.is_dir() && !m.file_type().is_symlink())
                        .unwrap_or(false)
            })
        });
        let mut completed = 0;
        for path in selected {
            self.mutable(&path, false)?;
            trash::delete(&path).with_context(|| format!("{completed}개 휴지통 이동 후 중단"))?;
            completed += 1;
        }
        Ok(completed)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;
    #[test]
    fn list_unicode_hidden_and_folder_sort() {
        let t = tempdir().unwrap();
        fs::create_dir(t.path().join("한글 폴더")).unwrap();
        fs::write(t.path().join("a.docx"), b"x").unwrap();
        fs::write(t.path().join(".hidden"), b"x").unwrap();
        let m = Manager::new(t.path()).unwrap();
        let entries = m.list(t.path(), false, Sort::Folders).unwrap();
        assert_eq!(entries.len(), 2);
        assert!(entries[0].is_dir);
        assert_eq!(m.list(t.path(), true, Sort::Name).unwrap().len(), 3);
    }
    #[test]
    fn copy_move_rename_preserve_content() {
        let t = tempdir().unwrap();
        fs::create_dir(t.path().join("dest")).unwrap();
        let source = t.path().join("한글 공백.txt");
        fs::write(&source, b"unchanged").unwrap();
        let m = Manager::new(t.path()).unwrap();
        let copied = m
            .transfer(
                &source,
                &t.path().join("dest"),
                true,
                Policy::Error,
                None,
                false,
            )
            .unwrap()
            .unwrap();
        assert!(source.exists());
        assert_eq!(fs::read(&copied).unwrap(), b"unchanged");
        let renamed = m.rename(&copied, "다른 이름.txt", false).unwrap();
        assert!(!copied.exists());
        assert_eq!(fs::read(renamed).unwrap(), b"unchanged");
    }
    #[test]
    fn conflict_never_clobbers() {
        let t = tempdir().unwrap();
        fs::create_dir(t.path().join("dest")).unwrap();
        let source = t.path().join("file");
        let target = t.path().join("dest/file");
        fs::write(&source, b"new").unwrap();
        fs::write(&target, b"old").unwrap();
        let m = Manager::new(t.path()).unwrap();
        assert!(m
            .transfer(
                &source,
                &t.path().join("dest"),
                false,
                Policy::Error,
                None,
                false
            )
            .is_err());
        assert_eq!(fs::read(&target).unwrap(), b"old");
        assert!(m
            .transfer(
                &source,
                &t.path().join("dest"),
                true,
                Policy::Skip,
                None,
                false
            )
            .unwrap()
            .is_none());
        assert!(source.exists());
    }
    #[test]
    fn boundaries_and_names() {
        let t = tempdir().unwrap();
        let other = tempdir().unwrap();
        let m = Manager::new(t.path()).unwrap();
        assert!(m.validate(other.path(), true).is_err());
        assert!(m.rename(t.path(), "no", false).is_err());
        fs::write(t.path().join("file"), b"x").unwrap();
        for invalid in ["../escape", "", ".", "..", "a/b"] {
            assert!(m.rename(&t.path().join("file"), invalid, false).is_err());
        }
        assert!(m.trash(&[t.path().to_path_buf()]).is_err());
    }
    #[test]
    fn own_descendant_rejected() {
        let t = tempdir().unwrap();
        let folder = t.path().join("a");
        fs::create_dir_all(folder.join("b")).unwrap();
        let m = Manager::new(t.path()).unwrap();
        assert!(m
            .transfer(&folder, &folder.join("b"), true, Policy::Error, None, false)
            .is_err());
    }
    #[test]
    fn import_preserves_original() {
        let t = tempdir().unwrap();
        let external = tempdir().unwrap();
        let source = external.path().join("external.pdf");
        fs::write(&source, b"binary").unwrap();
        let m = Manager::new(t.path()).unwrap();
        let result = m
            .transfer(&source, t.path(), true, Policy::Error, None, true)
            .unwrap()
            .unwrap();
        assert_eq!(fs::read(result).unwrap(), b"binary");
        assert!(source.exists());
        assert!(m
            .transfer(&source, t.path(), false, Policy::Error, Some("bad"), true)
            .is_err());
    }
    #[cfg(unix)]
    #[test]
    fn symlink_boundary_and_preserving_copy() {
        let t = tempdir().unwrap();
        let outside = tempdir().unwrap();
        std::os::unix::fs::symlink(outside.path(), t.path().join("outside")).unwrap();
        fs::create_dir(t.path().join("dest")).unwrap();
        let m = Manager::new(t.path()).unwrap();
        assert!(m.validate(&t.path().join("outside"), true).is_err());
        let copied = m
            .transfer(
                &t.path().join("outside"),
                &t.path().join("dest"),
                true,
                Policy::Error,
                None,
                false,
            )
            .unwrap()
            .unwrap();
        assert!(fs::symlink_metadata(copied)
            .unwrap()
            .file_type()
            .is_symlink());
    }
    #[test]
    fn exclusive_rename_race_guard() {
        let t = tempdir().unwrap();
        let a = t.path().join("a");
        let b = t.path().join("b");
        fs::write(&a, b"a").unwrap();
        fs::write(&b, b"b").unwrap();
        assert!(exclusive(&a, &b).is_err());
        assert_eq!(fs::read(&b).unwrap(), b"b");
        assert!(a.exists());
    }
}
