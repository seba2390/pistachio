//! What is kept of files attached to chats: where each is, and a small picture of a PDF's first
//! page. Messages refer to them by an id made from their contents.

use std::fs;
use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use crate::{Kind, cache, plain};

const MAX_FILES: usize = 1000;
const MAX_BYTES: u64 = 64 * 1024 * 1024;
/// The longest side of a picture, in pixels.
const SIZE: &str = "320";
/// How much of a text its small picture shows.
const START_BYTES: u64 = 1024;
const START_LINES: usize = 8;
const START_CHARS: usize = 24;
const TIMEOUT: Duration = Duration::from_secs(20);

fn folder(dir: &Path) -> PathBuf {
    dir.join("attached")
}

/// The file below `dir` for `id`, which a message names: only what [`remember`] returns.
fn file(dir: &Path, id: &str, extension: &str) -> Option<PathBuf> {
    let valid = id.len() == 32 && id.bytes().all(|b| b.is_ascii_hexdigit());
    valid.then(|| folder(dir).join(format!("{id}.{extension}")))
}

/// Keeps where the file at `path` is, with a picture of it if it is a PDF, and returns its id.
/// Blocks.
pub fn remember(dir: &Path, path: &Path) -> Option<String> {
    let bytes = fs::read(path).ok()?;
    let id = python_env::fetch::sha256(&bytes)[..32].to_owned();
    fs::create_dir_all(folder(dir)).ok()?;
    cache::write_private(&file(dir, &id, "path")?, path.to_str()?.as_bytes()).ok()?;
    let picture = file(dir, &id, "png")?;
    if Kind::of(path) == Some(Kind::Pdf) && !picture.exists() {
        let tmp = folder(dir).join(format!("{id}.{}.tmp.png", std::process::id()));
        if draw(path, &tmp) {
            let _ = fs::rename(&tmp, &picture);
        }
        let _ = fs::remove_file(&tmp);
    }
    cache::trim(&folder(dir), MAX_FILES, MAX_BYTES);
    Some(id)
}

/// The picture of the file `id`.
pub fn picture(dir: &Path, id: &str) -> Option<PathBuf> {
    file(dir, id, "png").filter(|path| path.exists())
}

/// Where the file `id` is, if it is still there.
pub fn original(dir: &Path, id: &str) -> Option<PathBuf> {
    let path = fs::read_to_string(file(dir, id, "path")?).ok()?;
    Some(PathBuf::from(path)).filter(|path| path.is_file())
}

/// The start of the lines that the text file at `path` starts with, for a small picture of it;
/// `None` if it is something else. Blocks.
pub fn first_lines(path: &Path) -> Option<String> {
    // Not a folder, or a pipe that would never end.
    if !fs::metadata(path).ok()?.is_file() {
        return None;
    }
    let mut bytes = Vec::new();
    let file = fs::File::open(path).ok()?;
    file.take(START_BYTES).read_to_end(&mut bytes).ok()?;
    // Cut within a character.
    if let Err(error) = std::str::from_utf8(&bytes)
        && error.error_len().is_none()
    {
        bytes.truncate(error.valid_up_to());
    }
    start(&plain::decode(bytes)?)
}

/// [`first_lines`] of a text.
pub fn start(text: &str) -> Option<String> {
    let lines: Vec<String> = text
        .trim_start_matches('\u{feff}')
        .lines()
        .take(START_LINES)
        .map(|line| line.chars().take(START_CHARS).collect())
        .collect();
    let is_text = lines.iter().any(|line| !line.trim().is_empty());
    is_text.then(|| lines.join("\n"))
}

/// Draws the first page of the PDF at `path` with what the system has: macOS's `sips`, or
/// poppler's `pdftoppm`. Without one, the file is shown without a picture.
fn draw(path: &Path, to: &Path) -> bool {
    let mut command = if cfg!(target_os = "macos") {
        let mut command = Command::new("/usr/bin/sips");
        command.args(["-s", "format", "png", "-Z", SIZE]);
        command.arg(path).arg("--out").arg(to);
        command
    } else if cfg!(unix) {
        let mut command = platform::command("pdftoppm");
        command.args([
            "-png",
            "-singlefile",
            "-f",
            "1",
            "-l",
            "1",
            "-scale-to",
            SIZE,
        ]);
        // It adds the `.png`.
        command.arg(path).arg(to.with_extension(""));
        command
    } else {
        return false;
    };
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let Ok(mut child) = command.spawn() else {
        return false;
    };
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return status.success() && to.exists(),
            Ok(None) if started.elapsed() < TIMEOUT => {
                std::thread::sleep(Duration::from_millis(20));
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return false;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    fn a_file_is_found_by_its_id() {
        let dir = tempfile::tempdir().unwrap();
        let dir = dir.path();
        let path = dir.join("check.PDF");
        fs::write(&path, include_bytes!("check.pdf")).unwrap();
        let id = remember(dir, &path).unwrap();
        assert_eq!(original(dir, &id), Some(path.clone()));
        if cfg!(target_os = "macos") {
            let png = fs::read(picture(dir, &id).unwrap()).unwrap();
            assert!(png.starts_with(b"\x89PNG"));
        }
        // The same again, from somewhere else.
        let copy = dir.join("copy.pdf");
        fs::rename(&path, &copy).unwrap();
        assert_eq!(original(dir, &id), None);
        assert_eq!(remember(dir, &copy).as_deref(), Some(id.as_str()));
        assert_eq!(original(dir, &id), Some(copy));
        let left = fs::read_dir(folder(dir)).unwrap().flatten();
        assert!(
            !left
                .into_iter()
                .any(|e| e.file_name().to_string_lossy().contains("tmp"))
        );
    }

    #[test]
    fn files_that_cannot_be_drawn_have_no_picture() {
        let dir = tempfile::tempdir().unwrap();
        let dir = dir.path();
        for name in ["broken.pdf", "a.txt"] {
            let path = dir.join(name);
            fs::write(&path, name).unwrap();
            let id = remember(dir, &path).unwrap();
            assert_eq!(picture(dir, &id), None);
            assert_eq!(original(dir, &id), Some(path));
        }
        assert_eq!(remember(dir, &dir.join("missing.pdf")), None);
    }

    #[test]
    fn the_first_lines_of_texts() {
        let dir = tempfile::tempdir().unwrap();
        let lines = |contents: &[u8]| {
            let path = dir.path().join("file");
            fs::write(&path, contents).unwrap();
            first_lines(&path)
        };
        assert_eq!(lines(b"one\r\n\ntwo\n").as_deref(), Some("one\n\ntwo"));
        let long = lines("æ".repeat(2000).as_bytes()).unwrap();
        assert_eq!(long, "æ".repeat(START_CHARS));
        // Cut within the second byte of a character.
        assert!(lines(format!("a{}", "æ".repeat(2000)).as_bytes()).is_some());
        assert_eq!(
            lines(b"1\n".repeat(99).as_slice()).unwrap().lines().count(),
            8
        );
        for other in [
            b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR\xff".as_slice(),
            b"a\0b",
            b" \n\n",
            b"",
        ] {
            assert_eq!(lines(other), None);
        }
        assert_eq!(first_lines(dir.path()), None);
        assert_eq!(first_lines(&dir.path().join("missing")), None);
    }

    /// A message can say any id.
    #[test]
    fn other_ids_lead_nowhere() {
        let dir = tempfile::tempdir().unwrap();
        let dir = dir.path();
        fs::write(dir.join("x.png"), "").unwrap();
        fs::write(dir.join("x.path"), dir.join("x.png").to_str().unwrap()).unwrap();
        fs::create_dir_all(folder(dir)).unwrap();
        for id in ["../x", "", "x", &"g".repeat(32)] {
            assert_eq!(picture(dir, id), None);
            assert_eq!(original(dir, id), None);
        }
    }
}
