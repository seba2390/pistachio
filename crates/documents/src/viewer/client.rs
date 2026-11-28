//! The app's side of the helper that draws pages ([`super::serve`]).

use std::io::{BufRead as _, BufReader, Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Stdio};

use super::serve::{Answer, COMMAND};
use super::{PageRequest, Picture, Size};
use crate::cli::{FLAG, Request};

/// Larger than any picture it draws.
const MAX_PIXELS: usize = 64 * 1024 * 1024;

/// A PDF file that is shown.
#[derive(Debug)]
pub struct Viewer {
    exe: PathBuf,
    path: PathBuf,
    /// `None` after it stopped on a page: another draws the next one.
    helper: Option<Helper>,
}

impl Viewer {
    /// Opens the PDF file at `path`, running `exe` as the helper. Blocks.
    pub fn open(exe: &Path, path: &Path) -> Result<(Self, Vec<Size>), String> {
        let (helper, pages) = Helper::start(exe, path)?;
        let viewer = Self {
            exe: exe.to_path_buf(),
            path: path.to_path_buf(),
            helper: Some(helper),
        };
        Ok((viewer, pages))
    }

    /// Draws a page. Blocks.
    pub fn draw(&mut self, request: &PageRequest) -> Result<Picture, String> {
        let mut helper = match self.helper.take() {
            Some(helper) => helper,
            None => Helper::start(&self.exe, &self.path)?.0,
        };
        match helper.draw(request) {
            Ok(drawn) => {
                self.helper = Some(helper);
                drawn
            }
            Err(Gone) => Err("it could not be drawn".into()),
        }
    }
}

/// The helper stopped, or says something else than it should.
struct Gone;

#[derive(Debug)]
struct Helper {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}

impl Helper {
    fn start(exe: &Path, path: &Path) -> Result<(Self, Vec<Size>), String> {
        let mut child = platform::command(exe)
            .args([FLAG, COMMAND])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|error| format!("its reader did not start: {error}"))?;
        let (Some(stdin), Some(stdout)) = (child.stdin.take(), child.stdout.take()) else {
            return Err("its reader did not start".into());
        };
        let mut helper = Self {
            child,
            stdin,
            stdout: BufReader::new(stdout),
        };
        let request = Request {
            path: path.to_path_buf(),
        };
        let opened = helper
            .send(&request)
            .and_then(|()| helper.answer())
            .unwrap_or_else(|Gone| Answer::Failed {
                error: "its reader stopped".into(),
            });
        match opened {
            Answer::Opened { pages } if !pages.is_empty() => Ok((helper, pages)),
            Answer::Failed { error } => Err(error),
            _ => Err("its reader stopped".into()),
        }
    }

    fn send(&mut self, request: &impl serde::Serialize) -> Result<(), Gone> {
        let line = serde_json::to_string(request).map_err(|_| Gone)?;
        writeln!(self.stdin, "{line}").map_err(|_| Gone)?;
        self.stdin.flush().map_err(|_| Gone)
    }

    fn answer(&mut self) -> Result<Answer, Gone> {
        let mut line = String::new();
        self.stdout.read_line(&mut line).map_err(|_| Gone)?;
        serde_json::from_str(&line).map_err(|_| Gone)
    }

    fn draw(&mut self, request: &PageRequest) -> Result<Result<Picture, String>, Gone> {
        self.send(request)?;
        match self.answer()? {
            Answer::Drawn {
                width,
                height,
                links,
            } => {
                let count = (width as usize)
                    .checked_mul(height as usize)
                    .filter(|count| *count <= MAX_PIXELS)
                    .ok_or(Gone)?;
                let mut pixels = vec![0; count * 4];
                self.stdout.read_exact(&mut pixels).map_err(|_| Gone)?;
                Ok(Ok(Picture {
                    width,
                    height,
                    links,
                    pixels,
                }))
            }
            Answer::Failed { error } => Ok(Err(error)),
            Answer::Opened { .. } => Err(Gone),
        }
    }
}

impl Drop for Helper {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[cfg(test)]
#[cfg(unix)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;

    const REQUEST: PageRequest = PageRequest {
        page: 0,
        scale: 1.,
        dark: false,
    };

    fn viewer(dir: &Path, script: &str) -> Result<(Viewer, Vec<Size>), String> {
        let exe = dir.join("app");
        crate::write_program(&exe, script);
        Viewer::open(&exe, Path::new("/a b.pdf"))
    }

    #[test]
    fn pages_come_from_the_helper() {
        let dir = tempfile::tempdir().unwrap();
        let script = "test \"$1 $2\" = '--documents-tool pages' || exit 1\n\
             read line; test \"$line\" = '{\"path\":\"/a b.pdf\"}' || exit 1\n\
             echo '{\"opened\":{\"pages\":[{\"width\":2,\"height\":1}]}}'\n\
             read line; echo '{\"drawn\":{\"width\":2,\"height\":1,\"links\":[]}}'; printf 12345678\n\
             read line; echo '{\"failed\":{\"error\":\"no\"}}'; read line";
        let (mut viewer, pages) = viewer(dir.path(), script).unwrap();
        let size = Size {
            width: 2.,
            height: 1.,
        };
        assert_eq!(pages, [size]);
        let picture = viewer.draw(&REQUEST).unwrap();
        assert_eq!((picture.width, picture.height), (2, 1));
        assert_eq!(picture.pixels, b"12345678");
        assert_eq!(viewer.draw(&REQUEST).unwrap_err(), "no");
    }

    #[test]
    fn files_that_do_not_open() {
        let dir = tempfile::tempdir().unwrap();
        let say = "read line; echo '{\"failed\":{\"error\":\"it is damaged\"}}'";
        assert_eq!(viewer(dir.path(), say).unwrap_err(), "it is damaged");
        for broken in ["exit 1", "echo what", "echo '{\"opened\":{\"pages\":[]}}'"] {
            let error = viewer(dir.path(), broken).unwrap_err();
            assert_eq!(error, "its reader stopped", "{broken}");
        }
    }

    /// A page that breaks the helper does not break the next one.
    #[test]
    fn it_goes_on_after_a_page_that_breaks_it() {
        let dir = tempfile::tempdir().unwrap();
        let broke = dir.path().join("broke");
        let script = format!(
            "read line; echo '{{\"opened\":{{\"pages\":[{{\"width\":2,\"height\":1}}]}}}}'\n\
             read line; test -e '{0}' || {{ touch '{0}'; exit 1; }}\n\
             echo '{{\"drawn\":{{\"width\":1,\"height\":1,\"links\":[]}}}}'; printf 1234; read line",
            broke.display()
        );
        let (mut viewer, _) = viewer(dir.path(), &script).unwrap();
        assert_eq!(viewer.draw(&REQUEST).unwrap_err(), "it could not be drawn");
        assert_eq!(viewer.draw(&REQUEST).unwrap().pixels, b"1234");
    }
}
