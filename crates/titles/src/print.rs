//! Naming with the conversation's own model: one `pi --print` call beside the conversation.
//!
//! The conversation's pi process is never asked: a prompt sent there would land in the
//! session file and the model's context, and the session would no longer match what the
//! terminal `pi` shows. Instead a separate, short-lived pi runs with the same provider and
//! model and `--no-session` (nothing is written), no tools, skills, prompt templates, context
//! files, MCP or theme, and its own system prompt ([`crate::INSTRUCTIONS`]). The message
//! travels on stdin, which pi puts before the prompt, so text starting with `@` or `-` is
//! never read as a file or an option.

use std::ffi::OsString;
use std::io::{Read as _, Write as _};
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::mpsc;
use std::time::Duration;

use anyhow::{Context as _, bail};

/// How long a naming call may take before it is given up (and the process killed). A slow
/// provider must not leave processes behind; the conversation never waits for it.
pub const TIMEOUT: Duration = Duration::from_secs(45);

/// A process to run: the program, its arguments, extra environment and working folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Invocation {
    pub program: PathBuf,
    pub args: Vec<OsString>,
    pub env: Vec<(OsString, OsString)>,
    pub cwd: PathBuf,
}

/// pi's arguments for one naming call with `provider`'s `model`. `extensions` keeps pi's
/// extensions loaded, for a model that an extension provides (they are off otherwise: they
/// could act on the session start, and they slow the start down).
pub fn pi_args(provider: &str, model: &str, extensions: bool) -> Vec<String> {
    let mut args: Vec<String> = [
        "--print",
        "--no-session",
        // No model catalog refresh: the call needs only the model it names.
        "--offline",
        "--no-tools",
        "--no-skills",
        "--no-prompt-templates",
        "--no-context-files",
        "--no-mcp",
        "--no-themes",
    ]
    .map(str::to_owned)
    .into();
    if !extensions {
        args.push("--no-extensions".into());
    }
    args.extend([
        "--provider".into(),
        provider.into(),
        "--model".into(),
        model.into(),
        // A title needs no reasoning; pi clamps this to what the model supports.
        "--thinking".into(),
        "off".into(),
        "--system-prompt".into(),
        crate::INSTRUCTIONS.into(),
        "--".into(),
        "Reply with the title only.".into(),
    ]);
    args
}

/// What pi reads on stdin (and puts before the prompt): the start of the first message.
pub fn stdin_text(message: &str) -> String {
    format!(
        "The first message of the conversation:\n\n{}\n",
        crate::excerpt(message, crate::EXCERPT_CHARS)
    )
}

/// Whether pi failed because it does not know the provider or model without its extensions.
fn unknown_model(error: &anyhow::Error) -> bool {
    let text = format!("{error:#}").to_lowercase();
    text.contains("unknown provider")
        || text.contains("unknown model")
        || text.contains("model not found")
        || text.contains("no model")
}

/// Names a conversation that starts with `message` with the model `invocation(extensions)`
/// runs: first without pi's extensions, then with them if pi did not know the model.
pub fn name_with_pi(
    invocation: impl Fn(bool) -> Invocation,
    message: &str,
    timeout: Duration,
) -> anyhow::Result<String> {
    let input = stdin_text(message);
    let answer = match run(&invocation(false), &input, timeout) {
        Err(error) if unknown_model(&error) => run(&invocation(true), &input, timeout)?,
        other => other?,
    };
    crate::title_for(message, &answer)
}

/// Runs `invocation` with `input` on stdin and returns its stdout, or an error with the end
/// of its stderr when it fails or takes longer than `timeout` (then it is killed).
pub fn run(invocation: &Invocation, input: &str, timeout: Duration) -> anyhow::Result<String> {
    let mut child = platform::command(&invocation.program)
        .args(&invocation.args)
        .envs(invocation.env.iter().map(|(key, value)| (key, value)))
        .current_dir(&invocation.cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("starting {}", invocation.program.display()))?;
    // Each pipe on its own thread, so a full pipe can never block the others.
    if let Some(mut stdin) = child.stdin.take() {
        let input = input.to_owned();
        std::thread::spawn(move || {
            let _ = stdin.write_all(input.as_bytes());
        });
    }
    let stderr = child.stderr.take().map(|mut pipe| {
        std::thread::spawn(move || {
            let mut text = String::new();
            let _ = pipe.read_to_string(&mut text);
            text
        })
    });
    let (sender, receiver) = mpsc::channel();
    if let Some(mut stdout) = child.stdout.take() {
        std::thread::spawn(move || {
            let mut text = String::new();
            let result = stdout.read_to_string(&mut text).map(|_| text);
            let _ = sender.send(result);
        });
    }
    let Ok(stdout) = receiver.recv_timeout(timeout) else {
        let _ = child.kill();
        let _ = child.wait();
        bail!("no answer within {} seconds", timeout.as_secs());
    };
    let status = child.wait().context("waiting for pi")?;
    let stdout = stdout.context("reading pi's answer")?;
    if !status.success() {
        let stderr = stderr
            .and_then(|thread| thread.join().ok())
            .unwrap_or_default();
        let tail: Vec<&str> = stderr.lines().rev().take(3).collect();
        let tail: Vec<&str> = tail.into_iter().rev().collect();
        bail!("pi exited with {status}: {}", tail.join(" ").trim());
    }
    Ok(stdout)
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    /// Names a few first messages with real pi and a real model:
    /// `TITLES_PI_MODEL=provider/model cargo test -p titles real_pi -- --ignored --nocapture`.
    #[test]
    #[ignore = "needs pi and a reachable model (TITLES_PI_MODEL)"]
    fn real_pi_names_conversations() {
        let model = std::env::var("TITLES_PI_MODEL").expect("TITLES_PI_MODEL=provider/model");
        let (provider, model) = model.split_once('/').expect("provider/model");
        let messages = [
            "build a world class and beutiful 2d sider scroller platformer. You have complete artistic freedom",
            "can you look at why the tests in crates/term are flaky on linux and fix them?",
            "Hvad er forskellen på en Mac mini og en Mac Studio til lokal AI?",
            "Write a detailed 600 word essay about the history of the Python language, with markdown headings.",
        ];
        let invocation = |extensions: bool| Invocation {
            program: "pi".into(),
            args: pi_args(provider, model, extensions)
                .into_iter()
                .map(Into::into)
                .collect(),
            env: Vec::new(),
            cwd: std::env::temp_dir(),
        };
        for message in messages {
            let started = std::time::Instant::now();
            let title = name_with_pi(invocation, message, TIMEOUT)
                .unwrap_or_else(|error| panic!("naming failed for {message:?}: {error:#}"));
            eprintln!("{:>6.1?}  {title:<45}  <- {message}", started.elapsed());
            let words = title.split_whitespace().count();
            assert!((1..=crate::MAX_WORDS).contains(&words), "{title:?}");
        }
    }

    fn sh(script: &str) -> Invocation {
        Invocation {
            program: "/bin/sh".into(),
            args: vec!["-c".into(), script.into()],
            env: Vec::new(),
            cwd: std::env::temp_dir(),
        }
    }

    #[test]
    fn arguments_keep_the_session_out() {
        let args = pi_args("anthropic", "claude-x", false);
        for flag in ["--print", "--no-session", "--no-tools", "--no-extensions"] {
            assert!(args.iter().any(|arg| arg == flag), "{flag}");
        }
        let model = args.iter().position(|arg| arg == "--model").unwrap();
        assert_eq!(args[model + 1], "claude-x");
        // The prompt comes after `--`, so it is never read as an option.
        assert_eq!(args[args.len() - 2], "--");
        assert!(!pi_args("p", "m", true).contains(&"--no-extensions".to_owned()));
    }

    #[test]
    fn runs_with_the_message_on_stdin() {
        let echo = sh("printf 'Title: '; head -c 200 | tail -n +3 | head -n 1");
        let title = name_with_pi(|_| echo.clone(), "@notes.md and -x", TIMEOUT).unwrap();
        assert_eq!(title, "@notes.md and -x");
    }

    #[test]
    fn retries_with_extensions_for_unknown_models() {
        let invocation = |extensions: bool| {
            if extensions {
                sh("cat > /dev/null; echo 'Extension model title'")
            } else {
                sh("cat > /dev/null; echo 'Error: Unknown provider \"ext\"' >&2; exit 1")
            }
        };
        assert_eq!(
            name_with_pi(invocation, "hello there", TIMEOUT).unwrap(),
            "Extension model title"
        );
    }

    #[test]
    fn failures_and_timeouts_are_errors() {
        let error = run(&sh("echo 'bad key' >&2; exit 3"), "", TIMEOUT).unwrap_err();
        assert!(error.to_string().contains("bad key"), "{error}");
        let started = std::time::Instant::now();
        let error = run(&sh("sleep 5"), "", Duration::from_millis(200)).unwrap_err();
        assert!(error.to_string().contains("no answer"), "{error}");
        assert!(started.elapsed() < Duration::from_secs(4));
    }
}
