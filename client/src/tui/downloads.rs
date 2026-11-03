//! Saving Party file downloads into the download directory.
//!
//! Two properties matter, and the previous version had neither:
//!
//! * **Never clobber.** It picked a free name with `exists()` and then wrote
//!   with `fs::write`, which truncates. Anything that appeared at that path in
//!   between — a second download of the same name finishing in the same tick,
//!   or a file the user saved — was silently overwritten. The name is now
//!   claimed with `create_new`, so the check and the claim are one syscall.
//! * **Never block the UI.** A community file is up to 100 MiB, and the write
//!   ran inline in the TUI's frame loop, freezing the whole interface for as
//!   long as the disk took. It now runs on the blocking pool and reports back
//!   through a channel the tick drains, like the download itself.

use std::io::Write;
use std::path::{Path, PathBuf};
use tokio::sync::oneshot;

/// Highest `name (n)` suffix tried before giving up. Unbounded used to mean an
/// endless loop on a directory where every create fails with `AlreadyExists`.
const MAX_NAME_SUFFIX: u32 = 10_000;

/// The outcome of one save: the display name, then where it landed.
pub(crate) type SaveOutcome = (String, std::io::Result<PathBuf>);

/// The `n`th candidate path for `name` under `dir`: `name`, then `stem (2).ext`,
/// `stem (3).ext`, …
fn candidate(dir: &Path, name: &str, n: u32) -> PathBuf {
    if n <= 1 {
        return dir.join(name);
    }
    let p = Path::new(name);
    let stem = p
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("received_file");
    match p.extension().and_then(|e| e.to_str()) {
        Some(ext) => dir.join(format!("{stem} ({n}).{ext}")),
        None => dir.join(format!("{stem} ({n})")),
    }
}

/// Write `data` to a path under `dir` that did not exist before this call,
/// returning that path. `name` must already be sanitized.
pub(crate) fn save_new_file(dir: &Path, name: &str, data: &[u8]) -> std::io::Result<PathBuf> {
    std::fs::create_dir_all(dir)?;
    for n in 1..=MAX_NAME_SUFFIX {
        let path = candidate(dir, name, n);
        let mut file = match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        };
        return match file.write_all(data).and_then(|()| file.sync_all()) {
            Ok(()) => Ok(path),
            Err(e) => {
                // A partial file under the user's chosen name is worse than
                // none: it looks like a finished download.
                drop(file);
                let _ = std::fs::remove_file(&path);
                Err(e)
            }
        };
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::AlreadyExists,
        format!("no free name for '{name}' after {MAX_NAME_SUFFIX} attempts"),
    ))
}

/// Start saving `data` off the UI thread. The returned receiver yields the
/// outcome; outside a tokio runtime (unit tests) the save runs inline.
pub(crate) fn spawn_save(
    dir: PathBuf,
    name: String,
    data: Vec<u8>,
) -> oneshot::Receiver<SaveOutcome> {
    let (tx, rx) = oneshot::channel();
    let job = move || {
        let result = save_new_file(&dir, &name, &data);
        let _ = tx.send((name, result));
    };
    match tokio::runtime::Handle::try_current() {
        Ok(handle) => {
            handle.spawn_blocking(job);
        }
        Err(_) => job(),
    }
    rx
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn never_clobbers_and_numbers_the_copies() {
        let dir = tempfile::tempdir().unwrap();
        let first = save_new_file(dir.path(), "report.pdf", b"one").unwrap();
        let second = save_new_file(dir.path(), "report.pdf", b"two").unwrap();
        let third = save_new_file(dir.path(), "report.pdf", b"three").unwrap();
        assert_eq!(first, dir.path().join("report.pdf"));
        assert_eq!(second, dir.path().join("report (2).pdf"));
        assert_eq!(third, dir.path().join("report (3).pdf"));
        assert_eq!(std::fs::read(&first).unwrap(), b"one");
        assert_eq!(std::fs::read(&second).unwrap(), b"two");
    }

    #[test]
    fn extensionless_names_get_the_suffix_too() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("notes"), b"mine").unwrap();
        let saved = save_new_file(dir.path(), "notes", b"theirs").unwrap();
        assert_eq!(saved, dir.path().join("notes (2)"));
        assert_eq!(std::fs::read(dir.path().join("notes")).unwrap(), b"mine");
    }

    #[test]
    fn creates_the_download_directory() {
        let dir = tempfile::tempdir().unwrap();
        let nested = dir.path().join("a/b");
        let saved = save_new_file(&nested, "x.bin", b"x").unwrap();
        assert_eq!(saved, nested.join("x.bin"));
    }

    #[tokio::test]
    async fn spawned_save_reports_back() {
        let dir = tempfile::tempdir().unwrap();
        let rx = spawn_save(dir.path().to_path_buf(), "a.txt".into(), b"hi".to_vec());
        let (name, result) = rx.await.unwrap();
        assert_eq!(name, "a.txt");
        assert_eq!(std::fs::read(result.unwrap()).unwrap(), b"hi");
    }
}
