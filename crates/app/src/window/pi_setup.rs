//! pi on this computer: looked for at startup, and when it is not found, a card that installs
//! it (see [`pi_install`]) or lets the user point at it.
//!
//! pi found somewhere other than the `PATH` (a Node.js version manager's folder, Pistachio's
//! own install) becomes Settings → pi's program, quietly.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use super::update_prompt::{Prompt, UpdateState, header, render_progress};
use super::*;
use crate::updater::Step;
use gpui_kit::component::Disableable as _;

/// How far the card got.
pub(super) enum PiState {
    Offer,
    /// Installing or checking, with the step it is on.
    Working(&'static str),
    Failed(SharedString),
}

/// What looking for pi found.
#[derive(Debug, PartialEq, Eq)]
enum Found {
    /// The configured program is there, or `pi` is on the `PATH`.
    Ready,
    /// Somewhere else: it becomes the configured program.
    At(PathBuf),
    Missing,
}

/// Where `program` (the configured one, or a bare `pi`) is, given the `PATH` children get.
fn look(program: &Path, path: Option<&OsString>) -> Found {
    let bare = program.components().count() == 1;
    if !bare && program.is_file() {
        return Found::Ready;
    }
    if bare {
        let own = std::env::var_os("PATH");
        if path
            .into_iter()
            .chain(own.as_ref())
            .any(|path| pi_install::on_path(path).is_some())
        {
            return Found::Ready;
        }
    }
    match pi_install::find(path.map(OsString::as_os_str)) {
        Some(found) => Found::At(found),
        None => Found::Missing,
    }
}

/// The installer's steps as the card shows them.
fn step_label(step: &str) -> &'static str {
    match step {
        "Looking for Node.js" => "Looking for Node.js…",
        "Downloading Node.js" => "Downloading Node.js…",
        "Verifying Node.js" => "Verifying Node.js…",
        "Unpacking Node.js" => "Unpacking Node.js…",
        "Installing pi" => "Installing pi from npm…",
        "Checking pi" => "Checking pi…",
        _ => "Installing…",
    }
}

impl MainWindow {
    /// Makes sure new sessions find pi: quietly when it is somewhere Pistachio knows, else the
    /// card. Not when `PISTACHIO_PI_BIN` chooses it.
    pub(super) fn look_for_pi(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if std::env::var_os("PISTACHIO_PI_BIN").is_some() {
            return;
        }
        let config = cx.global::<AppConfig>();
        let (program, env) = (config.pi_program.clone(), config.child_env.clone());
        cx.spawn_in(window, async move |this, cx| {
            let found = cx
                .background_executor()
                .spawn(async move { look(&program, env.var("PATH").as_ref()) })
                .await;
            let _ = this.update_in(cx, |this, window, cx| match found {
                Found::Ready => {}
                Found::At(path) => {
                    log::info!("pi is not on the PATH; using {}", path.display());
                    this.use_pi(&path, window, cx);
                }
                Found::Missing => {
                    log::warn!("pi was not found on this computer");
                    let updating = matches!(
                        this.update_prompt,
                        Some(Prompt::Update {
                            state: UpdateState::Working(_),
                            ..
                        })
                    );
                    if !updating {
                        this.show_pi_missing(window, cx);
                    }
                }
            });
        })
        .detach();
    }

    /// The card, whether pi is there or not (`PISTACHIO_OPEN=pi-missing`).
    pub(super) fn show_pi_missing(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.show_prompt(
            Prompt::PiMissing {
                state: PiState::Offer,
            },
            window,
            cx,
        );
    }

    /// Makes `path` the pi that sessions start, and restarts the session shown on this
    /// computer (it tried to start pi before pi was found).
    fn use_pi(&mut self, path: &Path, window: &mut Window, cx: &mut Context<Self>) {
        let path = path.to_string_lossy().into_owned();
        AppSettings::update(cx, |settings| settings.pi_program = Some(path));
        if self.host(cx).is_none() {
            self.reload_models(None, ReloadReason::Asked, window, cx);
        }
    }

    fn set_pi_state(&mut self, state: PiState, cx: &mut Context<Self>) {
        if let Some(Prompt::PiMissing { state: shown }) = self.update_prompt.as_mut() {
            *shown = state;
            cx.notify();
        }
    }

    fn pi_working(&self) -> bool {
        matches!(
            self.update_prompt,
            Some(Prompt::PiMissing {
                state: PiState::Working(_)
            })
        )
    }

    /// Uses `path` and closes the card, saying so.
    fn pi_ready(
        &mut self,
        path: &Path,
        message: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.set_pi_state(PiState::Offer, cx);
        self.dismiss_update_prompt(cx);
        self.use_pi(path, window, cx);
        window.push_notification(message, cx);
    }

    /// Installs pi in the background, showing each step.
    fn install_pi(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.pi_working() {
            return;
        }
        self.set_pi_state(PiState::Working("Starting…"), cx);
        let (steps, progress) = async_channel::unbounded::<String>();
        let task = cx.background_executor().spawn(async move {
            pi_install::install(&mut |step| {
                let _ = steps.send_blocking(step.to_owned());
            })
        });
        cx.spawn_in(window, async move |this, cx| {
            while let Ok(step) = progress.recv().await {
                let shown = this.update(cx, |this, cx| {
                    this.set_pi_state(PiState::Working(step_label(&step)), cx);
                });
                if shown.is_err() {
                    return;
                }
            }
            let result = task.await;
            let _ = this.update_in(cx, |this, window, cx| match result {
                Ok(path) => {
                    log::info!("installed pi at {}", path.display());
                    this.pi_ready(&path, "pi is installed.".to_owned(), window, cx);
                }
                Err(error) => {
                    log::warn!("installing pi failed: {error}");
                    this.set_pi_state(PiState::Failed(error.into()), cx);
                }
            });
        })
        .detach();
    }

    /// Asks where pi is (the program, or the folder it is in) and checks it.
    fn find_pi(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.pi_working() {
            return;
        }
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: true,
            multiple: false,
            prompt: Some("Use This pi".into()),
        });
        let env = cx.global::<AppConfig>().child_env.clone();
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(mut paths))) = paths.await else {
                return;
            };
            let Some(chosen) = paths.pop() else {
                return;
            };
            let _ = this.update(cx, |this, cx| {
                this.set_pi_state(PiState::Working("Checking pi…"), cx);
            });
            let checked = cx
                .background_executor()
                .spawn(async move {
                    let program = if chosen.is_dir() {
                        pi_install::on_path(chosen.as_os_str())
                            .ok_or_else(|| format!("There is no pi in {}.", chosen.display()))?
                    } else {
                        chosen
                    };
                    let version = pi_install::check(&program, env.vars())?;
                    Ok::<_, String>((program, version))
                })
                .await;
            let _ = this.update_in(cx, |this, window, cx| match checked {
                Ok((program, version)) => {
                    let message = format!("Using pi {version} at {}.", program.display());
                    this.pi_ready(&program, message, window, cx);
                }
                Err(error) => this.set_pi_state(PiState::Failed(error.into()), cx),
            });
        })
        .detach();
    }

    /// The card: what is missing, and Install pi / Find pi….
    pub(super) fn render_pi_missing(&self, state: &PiState, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let (muted, border) = (theme.muted_foreground, theme.border);
        let palette = crate::theme::palette(cx);
        let (accent, error) = (palette.accent, palette.error);
        let working = matches!(state, PiState::Working(_));
        let failed = matches!(state, PiState::Failed(_));
        let place = if cfg!(windows) {
            r"%USERPROFILE%\.pistachio\server"
        } else {
            "~/.pistachio/server"
        };
        let status: AnyElement = match state {
            PiState::Offer => div()
                .text_xs()
                .text_color(muted)
                .child(format!(
                    "Install pi downloads it from npm, with Node.js if this computer has no \
                     recent one, into {place}: nothing else changes and no administrator \
                     rights are needed. If pi is already installed somewhere Pistachio did not \
                     look, Find pi… points to it."
                ))
                .into_any_element(),
            PiState::Working(step) => {
                render_progress(&Step::Preparing(step), accent, border, muted)
            }
            PiState::Failed(message) => div()
                .id("pi-setup-error")
                .max_h(px(180.))
                .overflow_y_scroll()
                .p_3()
                .rounded_lg()
                .border_1()
                .border_color(error.alpha(0.5))
                .bg(error.alpha(0.08))
                .text_sm()
                .text_color(error)
                .child(message.clone())
                .into_any_element(),
        };
        crate::online::refresh(cx);
        let install = crate::ui::download::offline_aware(
            Button::new("pi-install").label(if failed { "Try again" } else { "Install pi" }),
            cx,
        )
        .primary()
        .loading(working)
        .disabled(working)
        .on_click(cx.listener(|_, _, window, cx| {
            let this = cx.weak_entity();
            crate::online::when_online(window, cx, move |window, cx| {
                let _ = this.update(cx, |this, cx| this.install_pi(window, cx));
            });
        }));
        v_flex()
            .gap_4()
            .child(header(
                AppIcon::PiLogo,
                "pi isn't installed".to_owned(),
                "Pistachio runs the pi coding agent, and it wasn't found on this computer."
                    .to_owned(),
                cx,
            ))
            .child(status)
            .child(
                h_flex()
                    .w_full()
                    .justify_between()
                    .gap_2()
                    .child(div().when(!working, |this| {
                        this.child(Button::new("pi-later").quiet(cx).label("Not now").on_click(
                            cx.listener(|this, _, _, cx| {
                                this.dismiss_update_prompt(cx);
                            }),
                        ))
                    }))
                    .child(
                        h_flex()
                            .gap_2()
                            .child(
                                Button::new("pi-find")
                                    .label("Find pi…")
                                    .disabled(working)
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.find_pi(window, cx);
                                    })),
                            )
                            .child(install),
                    ),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;
    use std::path::Path;

    use super::{Found, look, step_label};

    #[test]
    #[cfg(unix)]
    fn finds_pi_where_it_is() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = tempfile::tempdir().unwrap();
        let pi = dir.path().join("pi");
        std::fs::write(&pi, "#!/bin/sh\necho 1.0.0\n").unwrap();
        std::fs::set_permissions(&pi, std::fs::Permissions::from_mode(0o755)).unwrap();
        let path = OsString::from(dir.path());
        assert_eq!(look(Path::new("pi"), Some(&path)), Found::Ready);
        assert_eq!(look(&pi, None), Found::Ready);
        // A configured program that is gone: pi on the PATH takes its place.
        assert_eq!(
            look(&dir.path().join("gone/pi"), Some(&path)),
            Found::At(pi.clone())
        );
        assert_eq!(step_label("Installing pi"), "Installing pi from npm…");
    }
}
