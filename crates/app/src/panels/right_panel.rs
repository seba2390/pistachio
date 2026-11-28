//! The right-hand panel with "Diff" and "Files" tabs for the chat's working folder.
//!
//! Each tab is its own view ([`DiffView`], [`FilesView`]) that loads on the background
//! executor. While the panel is visible a debounced filesystem watcher on the folder keeps the
//! active tab live; the inactive tab is only marked stale and reloads when shown. Hiding the
//! panel (or switching folders) drops the watcher, so a hidden panel costs nothing.
//!
//! A folder on an SSH host has no file-change events: there the active tab is re-read every
//! [`REMOTE_POLL`] while visible, and whenever the agent finishes a turn.

mod diff_lines;
mod diff_model;
mod diff_view;
mod file_tree;
mod file_viewer;
mod files_view;

use std::path::{Component, Path, PathBuf};
use std::time::Duration;

use gpui_kit::component::button::Button;
use gpui_kit::component::tab::{Tab, TabBar};
use gpui_kit::component::{ActiveTheme as _, IconName, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use self::diff_view::DiffView;
use self::file_viewer::{FileViewer, FileViewerEvent};
use self::files_view::{FilesEvent, FilesView, relative_to};
use crate::theme::palette;
use crate::ui::QuietButton as _;
use crate::ui::{TOP_BAR_HEIGHT, WindowDrag};

/// Quiet time before a burst of filesystem events triggers a reload.
const WATCH_DEBOUNCE: Duration = Duration::from_millis(300);
/// How often a visible panel re-reads a folder on an SSH host.
const REMOTE_POLL: Duration = Duration::from_secs(10);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RightTab {
    Diff,
    Files,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RightPanelEvent {
    /// Mention this path in the chat composer. Relative to the panel's folder (the chat's
    /// cwd) when inside it, absolute otherwise.
    InsertPath(PathBuf),
}

pub struct RightPanel {
    cwd: PathBuf,
    /// Where `cwd` lives (this computer or an SSH host).
    location: workspace::Location,
    tab: RightTab,
    /// Whether the panel is on screen; only then does it watch and load.
    visible: bool,
    diff: Entity<DiffView>,
    files: Entity<FilesView>,
    /// The file open in the Files tab, shown instead of the tree until closed.
    viewer: Option<(Entity<FileViewer>, Subscription)>,
    /// The tab may be out of date and reloads when it is next shown.
    diff_stale: bool,
    files_stale: bool,
    drag: WindowDrag,
    /// Owns the filesystem watcher while visible; dropping the task stops watching.
    _watch: Task<()>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<RightPanelEvent> for RightPanel {}

impl RightPanel {
    pub fn new(cwd: PathBuf, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let location = workspace::Location::Local;
        let diff = cx.new(|_| DiffView::new(cwd.clone(), location.clone()));
        let files = cx.new(|_| FilesView::new(cwd.clone(), location.clone()));
        let subscriptions =
            vec![
                // The header shows the diff summary.
                cx.observe(&diff, |_, _, cx| cx.notify()),
                cx.subscribe_in(&files, window, |this, _, event: &FilesEvent, window, cx| {
                    match event {
                        FilesEvent::Insert(path) => {
                            cx.emit(RightPanelEvent::InsertPath(relative_to(&this.cwd, path)));
                        }
                        FilesEvent::Open(path) => this.open_file(path.clone(), window, cx),
                    }
                }),
            ];
        Self {
            cwd,
            location,
            tab: RightTab::Diff,
            visible: false,
            diff,
            files,
            viewer: None,
            diff_stale: true,
            files_stale: true,
            drag: WindowDrag::default(),
            _watch: Task::ready(()),
            _subscriptions: subscriptions,
        }
    }

    /// Expands every file in the Changes tab once loaded (`PISTACHIO_OPEN=diff-expand-all`).
    pub fn expand_all_diffs(&mut self, cx: &mut Context<Self>) {
        self.diff
            .update(cx, |diff, _| diff.expand_all_when_loaded());
    }

    /// Shows `path` (absolute, or relative to the folder) in the Files tab's viewer.
    pub fn open_file(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        self.open_file_at(path, None, window, cx);
    }

    /// [`Self::open_file`], scrolled to `line` (1-based).
    pub fn open_file_at(
        &mut self,
        path: PathBuf,
        line: Option<u32>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let path = if path.is_relative() {
            self.cwd.join(path)
        } else {
            path
        };
        let viewer = cx.new(|cx| {
            FileViewer::new(self.cwd.clone(), path, self.location.clone(), window, cx).at_line(line)
        });
        let subscription = cx.subscribe(
            &viewer,
            |this, _, event: &FileViewerEvent, cx| match event {
                FileViewerEvent::Close => this.close_file(cx),
                FileViewerEvent::Insert(path) => {
                    cx.emit(RightPanelEvent::InsertPath(relative_to(&this.cwd, path)));
                }
            },
        );
        self.viewer = Some((viewer, subscription));
        self.set_tab(RightTab::Files, cx);
        cx.notify();
    }

    /// The file in the viewer.
    #[cfg(all(test, unix))]
    pub fn open_path(&self, cx: &App) -> Option<PathBuf> {
        let (viewer, _) = self.viewer.as_ref()?;
        Some(viewer.read(cx).path().to_path_buf())
    }

    /// Back from the viewer to the tree.
    fn close_file(&mut self, cx: &mut Context<Self>) {
        self.viewer = None;
        cx.notify();
    }

    pub fn tab(&self) -> RightTab {
        self.tab
    }

    pub fn set_tab(&mut self, tab: RightTab, cx: &mut Context<Self>) {
        if self.tab != tab {
            self.tab = tab;
            self.refresh_stale(cx);
            cx.notify();
        }
    }

    pub fn set_cwd(&mut self, cwd: PathBuf, location: workspace::Location, cx: &mut Context<Self>) {
        if self.cwd == cwd && self.location == location {
            return;
        }
        self.cwd.clone_from(&cwd);
        self.location = location.clone();
        // The open file belongs to the old folder.
        self.viewer = None;
        let diff_location = location.clone();
        self.diff
            .update(cx, |diff, cx| diff.set_cwd(cwd.clone(), diff_location, cx));
        self.files
            .update(cx, |files, cx| files.set_cwd(cwd, location, cx));
        self.diff_stale = true;
        self.files_stale = true;
        if self.visible {
            self.watch(cx);
            self.refresh_stale(cx);
        } else {
            self._watch = Task::ready(());
        }
        cx.notify();
    }

    /// Re-reads git status / the file tree (called when the agent settles). While hidden this
    /// only marks both tabs stale.
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        self.diff_stale = true;
        self.files_stale = true;
        self.refresh_stale(cx);
    }

    /// Starts (or stops) watching and loading. Rendering the panel implies it is visible, so
    /// only hiding strictly needs this call; showing it again reloads whatever went stale.
    pub fn set_visible(&mut self, visible: bool, cx: &mut Context<Self>) {
        if self.visible == visible {
            return;
        }
        self.visible = visible;
        if visible {
            self.watch(cx);
            self.refresh_stale(cx);
        } else {
            // Nothing is watched while hidden, so anything may change meanwhile.
            self._watch = Task::ready(());
            self.diff_stale = true;
            self.files_stale = true;
        }
    }

    /// Reloads the active tab if it is stale and the panel is visible.
    fn refresh_stale(&mut self, cx: &mut Context<Self>) {
        if !self.visible {
            return;
        }
        match self.tab {
            RightTab::Diff if self.diff_stale => {
                self.diff_stale = false;
                self.diff.update(cx, |diff, cx| diff.refresh(cx));
            }
            RightTab::Files if self.files_stale => {
                self.files_stale = false;
                self.files.update(cx, |files, cx| files.refresh(cx));
            }
            RightTab::Diff | RightTab::Files => {}
        }
    }

    /// The refresh button: reloads the active tab unconditionally.
    fn reload(&mut self, cx: &mut Context<Self>) {
        match self.tab {
            RightTab::Diff => self.diff_stale = true,
            RightTab::Files => self.files_stale = true,
        }
        self.refresh_stale(cx);
    }

    /// (Re)starts the watcher on the current folder. It is created on the background
    /// executor (a recursive watch can take a while on some platforms) and lives inside the
    /// task, which forwards its debounced batches to [`Self::on_fs_change`].
    fn watch(&mut self, cx: &mut Context<Self>) {
        if !self.location.is_local() {
            // No remote file events: poll the active tab while visible.
            self._watch = cx.spawn(async move |this, cx| {
                loop {
                    cx.background_executor().timer(REMOTE_POLL).await;
                    let alive = this.update(cx, |this, cx| match this.tab {
                        RightTab::Diff => {
                            this.files_stale = true;
                            this.diff.update(cx, |diff, cx| diff.refresh(cx));
                        }
                        RightTab::Files => {
                            this.diff_stale = true;
                            this.files.update(cx, |files, cx| files.refresh(cx));
                        }
                    });
                    if alive.is_err() {
                        break;
                    }
                }
            });
            return;
        }
        let root = self.cwd.clone();
        self._watch = cx.spawn(async move |this, cx| {
            let (sender, receiver) = async_channel::unbounded::<Vec<PathBuf>>();
            let watched = root.clone();
            let watcher = cx
                .background_executor()
                .spawn(async move {
                    workspace::watch::Watcher::new(&watched, WATCH_DEBOUNCE, move |paths| {
                        let _ = sender.try_send(paths);
                    })
                })
                .await;
            let _watcher = match watcher {
                Ok(watcher) => watcher,
                Err(error) => {
                    log::debug!("not watching {}: {error}", root.display());
                    return;
                }
            };
            while let Ok(mut paths) = receiver.recv().await {
                // Merge batches that queued up meanwhile into one reload.
                while let Ok(more) = receiver.try_recv() {
                    paths.extend(more);
                }
                if this
                    .update(cx, |this, cx| this.on_fs_change(paths, cx))
                    .is_err()
                {
                    break;
                }
            }
        });
    }

    fn on_fs_change(&mut self, mut paths: Vec<PathBuf>, cx: &mut Context<Self>) {
        match self.tab {
            RightTab::Diff => self.diff.update(cx, |diff, cx| diff.refresh(cx)),
            RightTab::Files => self.diff_stale = true,
        }
        // `.git` internals never show in the tree.
        paths.retain(|path| !is_in_git_dir(path));
        if paths.is_empty() {
            return;
        }
        if let Some((viewer, _)) = &self.viewer
            && paths.iter().any(|path| path == viewer.read(cx).path())
        {
            viewer.update(cx, |viewer, cx| viewer.mark_stale(cx));
        }
        match self.tab {
            RightTab::Files => self
                .files
                .update(cx, |files, cx| files.on_changes(&paths, cx)),
            RightTab::Diff => self.files_stale = true,
        }
    }

    /// The panel is always the rightmost column: its top bar keeps room for the window's
    /// buttons where the app draws them.
    fn render_header(&self, window: &Window, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let controls = crate::ui::window_controls::inset(window);
        let theme = cx.theme();
        let (muted, border) = (theme.muted_foreground, theme.border);
        let colors = palette(cx);
        let summary = match self.tab {
            RightTab::Diff => self.diff.read(cx).summary().cloned(),
            RightTab::Files => None,
        };
        let panel = cx.entity().downgrade();

        self.drag.apply(
            h_flex()
                .id("right-panel-top-bar")
                .h(TOP_BAR_HEIGHT)
                .flex_none()
                .w_full()
                .pl_3()
                .pr(px(8.) + controls)
                .gap_2()
                .border_b_1()
                .border_color(border)
                .child(
                    // Clicking a tab must not start a window drag.
                    div()
                        .flex_none()
                        .on_mouse_down(MouseButton::Left, |_, window, _| window.prevent_default())
                        .child(
                            TabBar::new("right-panel-tabs")
                                .segmented()
                                .small()
                                .selected_index(match self.tab {
                                    RightTab::Diff => 0,
                                    RightTab::Files => 1,
                                })
                                .on_click(move |ix: &usize, _, cx| {
                                    let tab = if *ix == 0 {
                                        RightTab::Diff
                                    } else {
                                        RightTab::Files
                                    };
                                    let _ = panel.update(cx, |panel, cx| panel.set_tab(tab, cx));
                                })
                                .child(Tab::new().label("Diff"))
                                .child(Tab::new().label("Files")),
                        ),
                )
                .child(
                    h_flex()
                        .flex_1()
                        .min_w_0()
                        .gap_1p5()
                        .text_xs()
                        .text_color(muted)
                        .when_some(summary, |this, summary| {
                            this.child(div().min_w_0().truncate().child(summary.label))
                                .children(summary.additions.map(|text| {
                                    div().flex_none().text_color(colors.added).child(text)
                                }))
                                .children(summary.deletions.map(|text| {
                                    div().flex_none().text_color(colors.removed).child(text)
                                }))
                        }),
                )
                .child(
                    Button::new("right-panel-refresh")
                        .quiet(cx)
                        .small()
                        .icon(IconName::RefreshCw)
                        .accessibility_label("Refresh")
                        .tooltip("Refresh")
                        .on_click(cx.listener(|this, _, _, cx| this.reload(cx))),
                ),
        )
    }
}

/// Whether `path` is a `.git` directory or inside one.
fn is_in_git_dir(path: &Path) -> bool {
    path.components()
        .any(|component| matches!(component, Component::Normal(name) if name == ".git"))
}

impl Render for RightPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let _timer = crate::perf::render("right_panel");
        if !self.visible {
            // Being rendered means being on screen (a hidden resizable panel renders nothing).
            cx.defer_in(window, |this, _, cx| this.set_visible(true, cx));
        }
        let content = match self.tab {
            RightTab::Diff => self.diff.clone().into_any_element(),
            RightTab::Files => match &self.viewer {
                Some((viewer, _)) => viewer.clone().into_any_element(),
                None => self.files.clone().into_any_element(),
            },
        };
        v_flex()
            .size_full()
            .child(self.render_header(window, cx))
            .child(div().flex_1().min_h_0().child(content))
    }
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};
    use std::process::Command;
    use std::time::{Duration, Instant};

    use gpui_kit::{TestAppContext, VisualTestContext};

    use super::{RightPanel, RightTab, is_in_git_dir};

    /// A fresh git repository with one untracked file and one subdirectory.
    fn scratch_repo() -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("pistachio-right-panel-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::write(dir.join("notes.txt"), "one\ntwo\n").unwrap();
        std::fs::write(dir.join("src/lib.rs"), "fn x() {}\n").unwrap();
        let status = Command::new("git")
            .args(["init", "-q"])
            .current_dir(&dir)
            .status()
            .unwrap();
        assert!(status.success());
        dir
    }

    /// Runs the executor and redraws until `done` holds (loads run git on real threads).
    fn settle(cx: &mut VisualTestContext, done: impl Fn(&mut VisualTestContext) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            cx.run_until_parked();
            cx.update(|window, cx| window.draw(cx).clear(cx));
            if done(cx) {
                return;
            }
            assert!(Instant::now() < deadline, "panel did not settle");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// Showing the panel loads the diff; switching tabs loads the tree; rows render.
    #[gpui_kit::test]
    #[allow(clippy::unused_async)] // gpui tests take an async body
    async fn loads_both_tabs_when_shown(cx: &mut TestAppContext) {
        cx.executor().allow_parking();
        cx.update(gpui_kit::component::init);
        let dir = scratch_repo();
        let (panel, cx) = cx.add_window_view(|window, cx| RightPanel::new(dir.clone(), window, cx));

        settle(cx, |cx| {
            cx.read(|cx| panel.read(cx).diff.read(cx).summary().is_some())
        });
        let label = cx.read(|cx| {
            panel
                .read(cx)
                .diff
                .read(cx)
                .summary()
                .unwrap()
                .label
                .clone()
        });
        assert!(label.ends_with("2 files"), "{label}");
        assert!(cx.read(|cx| panel.read(cx).visible));

        // Expanding an untracked file shows its patch: hunk header plus two added lines.
        let diff = cx.read(|cx| panel.read(cx).diff.clone());
        cx.update(|_, cx| diff.update(cx, |diff, cx| diff.toggle(Path::new("notes.txt"), cx)));
        settle(cx, |cx| cx.read(|cx| diff.read(cx).row_count() == 2 + 3));

        cx.update(|_, cx| panel.update(cx, |panel, cx| panel.set_tab(RightTab::Files, cx)));
        settle(cx, |cx| {
            cx.read(|cx| panel.read(cx).files.read(cx).row_count() == 2)
        });

        cx.update(|_, cx| panel.update(cx, |panel, cx| panel.set_visible(false, cx)));
        assert!(cx.read(|cx| panel.read(cx).diff_stale));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn git_dir_paths() {
        assert!(is_in_git_dir(Path::new("/r/.git")));
        assert!(is_in_git_dir(Path::new("/r/.git/index")));
        assert!(!is_in_git_dir(Path::new("/r/.gitignore")));
        assert!(!is_in_git_dir(Path::new("/r/src/git/a.rs")));
    }
}
