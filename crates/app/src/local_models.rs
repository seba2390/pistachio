//! A local model suggested for this computer (the [`local_models`] crate): scanning the computer,
//! downloading a suggestion, and running it while chats use it.
//!
//! What was set up is started when a chat is about to send to it ([`ensure_running`]), and
//! stopped when the app quits; nothing runs for a model no chat uses.

use std::collections::HashMap;
use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use gpui_kit::{
    App, AppContext as _, AsyncApp, Context, Entity, EventEmitter, Global, Subscription, Task,
    WeakEntity, Window,
};
use local_models::install::{self, InstallError, Part};
use local_models::{Choice, Hardware, Progress, Server, Setup, Suggestion, catalog, setups};

use crate::config::AppConfig;
use crate::ui::AppIcon;
use crate::ui::download::{Details, Part as DownloadPart, ask_to_download_and_run};

/// How long a scan of this computer takes at least, so its progress can be followed.
pub const SCAN_TIME: Duration = Duration::from_secs(2);

/// How full the bar of a scan is `elapsed` into it: full at [`SCAN_TIME`] once the scan
/// `found` what it looks for. A scan that takes longer slows near the end instead of standing
/// full.
pub fn scan_progress(elapsed: Duration, found: bool) -> f32 {
    let t = elapsed.as_secs_f32() / SCAN_TIME.as_secs_f32();
    if found || t <= 0.9 {
        t.min(1.)
    } else {
        0.99 - 0.09 * (-(t - 0.9) / 0.5).exp()
    }
}

/// What is known of this computer.
pub enum Scan {
    NotYet,
    /// Since when; `found` once the scan ended, which still shows until [`SCAN_TIME`] passed.
    Scanning {
        since: Instant,
        found: bool,
    },
    Done {
        hardware: Hardware,
        suggestions: Vec<Suggestion>,
    },
}

/// A setup's engine.
enum Run {
    Starting(Vec<async_channel::Sender<bool>>),
    Ready(Server),
}

/// How a choice stands.
#[derive(Debug, Clone, PartialEq)]
pub enum Status {
    /// Something to download first, this many bytes.
    Missing(u64),
    Installing(Option<Progress>),
    /// Downloaded, not running.
    Installed,
    Starting,
    Running,
}

/// Whether a setup's engine runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunState {
    Stopped,
    /// Started, loading the model.
    Starting,
    Running,
}

/// What removing a setup deletes from this computer.
#[derive(Debug, Clone, PartialEq)]
pub struct Installed {
    /// The model, unless another setup uses it too.
    pub model: Option<String>,
    /// The engine, unless another setup uses it too.
    pub engine: Option<String>,
    /// Bytes of the model's files.
    pub size: u64,
}

pub struct LocalModels {
    root: PathBuf,
    scan: Scan,
    setups: Vec<Setup>,
    job: Option<Job>,
    running: HashMap<String, Run>,
    /// Why the last download or start failed.
    error: Option<String>,
    _quit: Subscription,
}

/// The download (and build) that may be running; one at a time.
struct Job {
    key: String,
    progress: Option<Progress>,
    cancel: Arc<AtomicBool>,
    _task: Task<()>,
}

#[derive(Debug, Clone)]
pub enum LocalModelsEvent {
    /// A setup runs and answers at `url`; connect pi to it.
    Ready { url: String },
}

impl EventEmitter<LocalModelsEvent> for LocalModels {}

struct Handle(Entity<LocalModels>);

impl Global for Handle {}

/// After [`AppConfig`] is set.
pub fn init(cx: &mut App) {
    let root = cx.global::<AppConfig>().local_models_dir.clone();
    let models = cx.new(|cx| LocalModels::new(root, cx));
    cx.set_global(Handle(models));
}

pub fn local_models(cx: &App) -> Option<Entity<LocalModels>> {
    cx.try_global::<Handle>().map(|handle| handle.0.clone())
}

/// The login shell's `PATH`, which builds and engines run with. Blocks until it is known.
fn login_path(cx: &App) -> Option<OsString> {
    let env = cx.global::<AppConfig>().child_env.clone();
    env.vars()
        .iter()
        .find(|(name, _)| name == "PATH")
        .map(|(_, value)| value.clone())
}

/// Starts the setup the model `provider` of pi's `models.json` points at, if it is one and does
/// not run. Returns a task that ends once it answers (`true`) or could not start; `None` when
/// there is nothing to wait for.
pub fn ensure_running(provider: &str, cx: &mut App) -> Option<Task<bool>> {
    let models = local_models(cx)?;
    let agent_dir = match crate::pi_place::PiPlace::for_host(None, cx)? {
        crate::pi_place::PiPlace::Local { agent_dir } => agent_dir,
        crate::pi_place::PiPlace::Remote { .. } => return None,
    };
    if models.read(cx).setups.is_empty() {
        return None;
    }
    let text = std::fs::read_to_string(agent_dir.join("models.json")).ok()?;
    let file = pi_config::ModelsFile::parse(&text).ok()?;
    let base_url = file
        .providers()
        .into_iter()
        .find(|info| info.id == provider)?
        .base_url?;
    models.update(cx, |models, cx| {
        let key = setups::at(&models.setups, &base_url)?.key();
        models.wait_for(&key, cx)
    })
}

impl LocalModels {
    fn new(root: PathBuf, cx: &mut Context<Self>) -> Self {
        let quit = cx.on_app_quit(|this, _| {
            // Stops every engine.
            this.running.clear();
            async {}
        });
        Self {
            setups: setups::load(&root),
            root,
            scan: Scan::NotYet,
            job: None,
            running: HashMap::new(),
            error: None,
            _quit: quit,
        }
    }

    /// Takes `hardware` as this computer's (for tests).
    #[cfg(test)]
    pub fn scanned(&mut self, hardware: Hardware, cx: &mut Context<Self>) {
        let suggestions = local_models::suggest(&hardware, catalog());
        self.scan = Scan::Done {
            hardware,
            suggestions,
        };
        cx.notify();
    }

    pub fn scan(&self) -> &Scan {
        &self.scan
    }

    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    /// Looks at this computer (again), in the background, and makes the suggestions.
    pub fn scan_now(&mut self, cx: &mut Context<Self>) {
        if matches!(self.scan, Scan::Scanning { .. }) {
            return;
        }
        self.scan = Scan::Scanning {
            since: Instant::now(),
            found: false,
        };
        let root = self.root.clone();
        let env = cx.global::<AppConfig>().child_env.clone();
        // Long enough to see that it looked; a scan is often quicker.
        let shown = cx.background_executor().timer(SCAN_TIME);
        cx.spawn(async move |this, cx| {
            let scan = cx
                .background_spawn(async move {
                    let path = env
                        .vars()
                        .iter()
                        .find(|(name, _)| name == "PATH")
                        .map(|(_, value)| value.clone());
                    let hardware = local_models::hardware::scan(&root, path.as_deref());
                    let suggestions = local_models::suggest(&hardware, catalog());
                    Scan::Done {
                        hardware,
                        suggestions,
                    }
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                if let Scan::Scanning { found, .. } = &mut this.scan {
                    *found = true;
                    cx.notify();
                }
            });
            shown.await;
            let _ = this.update(cx, |this, cx| {
                if matches!(this.scan, Scan::Scanning { .. }) {
                    this.scan = scan;
                    cx.notify();
                }
            });
        })
        .detach();
        cx.notify();
    }

    fn setup(&self, key: &str) -> Option<&Setup> {
        self.setups.iter().find(|setup| setup.key() == key)
    }

    /// How `choice` stands on this computer.
    pub fn status(&self, choice: &Choice) -> Status {
        let key = choice.key();
        if let Some(job) = self.job.as_ref().filter(|job| job.key == key) {
            return Status::Installing(job.progress.clone());
        }
        match self.running.get(&key) {
            Some(Run::Starting(_)) => return Status::Starting,
            Some(Run::Ready(_)) => return Status::Running,
            None => {}
        }
        let missing: u64 = install::missing(&self.root, choice)
            .iter()
            .map(|part| part.size)
            .sum();
        if missing > 0 || self.setup(&key).is_none() {
            Status::Missing(missing)
        } else {
            Status::Installed
        }
    }

    pub fn is_busy(&self) -> bool {
        self.job.is_some()
    }

    /// The address of a running setup.
    pub fn url(&self, choice: &Choice) -> Option<String> {
        self.setup(&choice.key()).map(Setup::base_url)
    }

    /// The model, its context and the engine of the setup listening at `base_url`.
    pub fn chosen_at(
        &self,
        base_url: &str,
    ) -> Option<(&'static catalog::Model, &'static catalog::Engine, u64)> {
        let setup = setups::at(&self.setups, base_url)?;
        let catalog = catalog();
        Some((
            catalog.model(&setup.model)?,
            catalog.engine(&setup.engine)?,
            setup.context,
        ))
    }

    /// Whether the setup listening at `base_url` runs; `None` when Pistachio set none up there.
    pub fn run_state_at(&self, base_url: &str) -> Option<RunState> {
        let key = setups::at(&self.setups, base_url)?.key();
        Some(match self.running.get(&key) {
            Some(Run::Starting(_)) => RunState::Starting,
            Some(Run::Ready(_)) => RunState::Running,
            None => RunState::Stopped,
        })
    }

    /// Starts the engine of the setup listening at `base_url`, which loads its model.
    pub fn start_at(&mut self, base_url: &str, cx: &mut Context<Self>) {
        if let Some(key) = setups::at(&self.setups, base_url).map(Setup::key) {
            self.start(&key, false, cx);
        }
    }

    /// Stops the engine of the setup listening at `base_url`; its files stay.
    pub fn stop_at(&mut self, base_url: &str, cx: &mut Context<Self>) {
        if let Some(key) = setups::at(&self.setups, base_url).map(Setup::key)
            && matches!(self.running.get(&key), Some(Run::Ready(_)))
        {
            // Dropping an engine stops it.
            self.running.remove(&key);
            cx.notify();
        }
    }

    /// What removing the setup listening at `base_url` deletes, when Pistachio set one up there.
    pub fn installed_at(&self, base_url: &str) -> Option<Installed> {
        let setup = setups::at(&self.setups, base_url)?;
        let catalog = catalog();
        let others: Vec<&Setup> = self
            .setups
            .iter()
            .filter(|other| other.port != setup.port)
            .collect();
        let model = catalog.model(&setup.model);
        let build = model.and_then(|model| model.build(&setup.build));
        let shared_model = others
            .iter()
            .any(|other| other.model == setup.model && other.build == setup.build);
        let shared_engine = others.iter().any(|other| other.engine == setup.engine);
        Some(Installed {
            model: (!shared_model).then(|| {
                let name = model.map_or(setup.model.as_str(), |model| &model.name);
                match build {
                    Some(build) => format!("{name} · {}", build.quant),
                    None => name.to_owned(),
                }
            }),
            engine: (!shared_engine).then(|| {
                catalog
                    .engine(&setup.engine)
                    .map_or_else(|| setup.engine.clone(), |engine| engine.name.clone())
            }),
            size: build
                .filter(|_| !shared_model)
                .map_or(0, |build| build.size),
        })
    }

    /// Stops and forgets the setup listening at `base_url`, and deletes its model and engine
    /// (unless another setup uses them) in the background. `None` when there is none there.
    pub fn remove(
        &mut self,
        base_url: &str,
        cx: &mut Context<Self>,
    ) -> Option<Task<Result<(), String>>> {
        let key = setups::at(&self.setups, base_url)?.key();
        let ix = self.setups.iter().position(|setup| setup.key() == key)?;
        let setup = self.setups.remove(ix);
        // Dropping an engine stops it.
        self.running.remove(&key);
        if let Err(error) = setups::save(&self.root, &self.setups) {
            log::warn!("saving the local models: {error}");
        }
        cx.notify();
        let (root, mut others) = (self.root.clone(), self.setups.clone());
        // What is being downloaded stays too.
        if let Some(job) = &self.job
            && let [model, build, engine] = job.key.splitn(3, '/').collect::<Vec<_>>()[..]
        {
            others.push(Setup {
                model: model.to_owned(),
                build: build.to_owned(),
                engine: engine.to_owned(),
                port: 0,
                context: 0,
            });
        }
        Some(cx.background_spawn(async move {
            install::uninstall(&root, catalog(), &setup, &others)
                .map_err(|error| format!("Could not delete the model's files: {error}"))
        }))
    }

    /// Asks whether to download what `choice` needs, showing exactly what that is, and on a yes
    /// downloads it and starts it. Starts it right away when nothing is missing.
    pub fn ask_and_set_up(this: &Entity<Self>, choice: &Choice, window: &mut Window, cx: &mut App) {
        let root = this.read(cx).root.clone();
        let parts = install::missing(&root, choice);
        if parts.is_empty() {
            this.update(cx, |this, cx| this.set_up(choice.clone(), cx));
            return;
        }
        let details = details(&root, choice, &parts);
        let name = format!("{} with {}", choice.model.name, choice.engine.name);
        let (this, choice) = (this.downgrade(), choice.clone());
        ask_to_download_and_run(
            &name,
            details,
            move |cx| {
                let _ = this.update(cx, |this, cx| this.set_up(choice.clone(), cx));
            },
            window,
            cx,
        );
    }

    /// Downloads what `choice` needs, records it as set up, and starts it.
    fn set_up(&mut self, choice: Choice, cx: &mut Context<Self>) {
        if self.job.is_some() {
            return;
        }
        self.error = None;
        let key = choice.key();
        let cancel = Arc::new(AtomicBool::new(false));
        let (tx, rx) = async_channel::unbounded();
        let (root, thread_cancel) = (self.root.clone(), cancel.clone());
        let path = login_path(cx);
        let thread_choice = choice.clone();
        let spawned = std::thread::Builder::new()
            .name("local-model-install".into())
            .spawn(move || {
                let result = install::install(
                    &root,
                    &thread_choice,
                    path.as_deref(),
                    &thread_cancel,
                    &|progress| {
                        let _ = tx.send_blocking(Ok(progress));
                    },
                );
                let _ = tx.send_blocking(Err(result));
            });
        if let Err(error) = spawned {
            return self.fail(
                format!("{} could not be downloaded: {error}", choice.model.name),
                cx,
            );
        }
        let task = cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            while let Ok(message) = rx.recv().await {
                let _ = this.update(cx, |this, cx| match message {
                    Ok(progress) => {
                        if let Some(job) = &mut this.job {
                            job.progress = Some(progress);
                        }
                        cx.notify();
                    }
                    Err(result) => this.installed(&choice, result, cx),
                });
            }
        });
        self.job = Some(Job {
            key,
            progress: None,
            cancel,
            _task: task,
        });
        cx.notify();
    }

    fn installed(
        &mut self,
        choice: &Choice,
        result: Result<(), InstallError>,
        cx: &mut Context<Self>,
    ) {
        self.job = None;
        match result {
            Ok(()) => {
                let key = choice.key();
                if self.setup(&key).is_none() {
                    let Some(port) = setups::free_port(&self.setups) else {
                        return self.fail("No free port for the model.".into(), cx);
                    };
                    self.setups.push(Setup {
                        model: choice.model.id.clone(),
                        build: choice.build.id.clone(),
                        engine: choice.engine.id.clone(),
                        port,
                        context: choice.context,
                    });
                    if let Err(error) = setups::save(&self.root, &self.setups) {
                        log::warn!("saving the local models: {error}");
                    }
                }
                self.start(&key, true, cx);
            }
            Err(InstallError::Cancelled) => {}
            Err(InstallError::Failed(error)) => {
                self.fail(
                    format!("{} could not be set up: {error}", choice.model.name),
                    cx,
                );
            }
        }
        cx.notify();
    }

    /// Stops the download at once: it can be started again right away. The thread doing it
    /// stops at its next step (a stalled connection may hold it a while) and is not heard from.
    pub fn cancel(&mut self, cx: &mut Context<Self>) {
        if let Some(job) = self.job.take() {
            job.cancel.store(true, Ordering::Relaxed);
            cx.notify();
        }
    }

    /// Starts `choice`'s setup; announces it (to connect pi) once it answers.
    pub fn run(&mut self, choice: &Choice, cx: &mut Context<Self>) {
        self.start(&choice.key(), true, cx);
    }

    /// A task that ends when the setup `key` runs, starting it when it does not.
    fn wait_for(&mut self, key: &str, cx: &mut Context<Self>) -> Option<Task<bool>> {
        if self.runs(key) {
            return None;
        }
        let (tx, rx) = async_channel::bounded(1);
        self.start(key, false, cx);
        if let Some(Run::Starting(waiters)) = self.running.get_mut(key) {
            waiters.push(tx);
        } else {
            return None;
        }
        Some(cx.background_spawn(async move { rx.recv().await.unwrap_or(false) }))
    }

    /// Whether the setup `key` runs; forgets it when it stopped on its own.
    fn runs(&mut self, key: &str) -> bool {
        let alive = match self.running.get_mut(key) {
            Some(Run::Ready(server)) => server.is_running(),
            _ => return false,
        };
        if !alive {
            self.running.remove(key);
        }
        alive
    }

    fn start(&mut self, key: &str, announce: bool, cx: &mut Context<Self>) {
        if matches!(self.running.get(key), Some(Run::Starting(_))) {
            return;
        }
        if self.runs(key) {
            if announce && let Some(setup) = self.setup(key) {
                cx.emit(LocalModelsEvent::Ready {
                    url: setup.base_url(),
                });
            }
            return;
        }
        let Some(setup) = self.setup(key).cloned() else {
            return;
        };
        self.error = None;
        self.running
            .insert(key.to_owned(), Run::Starting(Vec::new()));
        cx.notify();
        let root = self.root.clone();
        let env = cx.global::<AppConfig>().child_env.clone();
        let key = key.to_owned();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn({
                    let setup = setup.clone();
                    async move {
                        let path = env
                            .vars()
                            .iter()
                            .find(|(name, _)| name == "PATH")
                            .map(|(_, value)| value.clone());
                        Server::start(&root, catalog(), &setup, path.as_deref())
                    }
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                let waiters = match this.running.remove(&key) {
                    Some(Run::Starting(waiters)) => waiters,
                    _ => Vec::new(),
                };
                let ok = result.is_ok();
                match result {
                    Ok(server) => {
                        this.running.insert(key.clone(), Run::Ready(server));
                        if announce {
                            cx.emit(LocalModelsEvent::Ready {
                                url: setup.base_url(),
                            });
                        }
                    }
                    Err(error) => {
                        this.fail(format!("The local model could not start: {error}"), cx);
                    }
                }
                for waiter in waiters {
                    let _ = waiter.try_send(ok);
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn fail(&mut self, error: String, cx: &mut Context<Self>) {
        log::warn!("{error}");
        self.error = Some(error);
        cx.notify();
    }
}

/// What the download dialog says about `choice`.
#[allow(clippy::cast_precision_loss)]
fn details(root: &std::path::Path, choice: &Choice, parts: &[Part]) -> Details {
    let size: u64 = parts.iter().map(|part| part.size).sum();
    let speed = match choice.measured {
        Some(speed) => format!(
            "about {:.0} tokens a second (measured on {})",
            choice.tps, speed.hardware
        ),
        None => format!("about {:.0} tokens a second here (estimated)", choice.tps),
    };
    let mut intro = format!(
        "{} ({} {}) run by {}: {speed}, using about {:.1} GB of memory. It runs on this computer \
         only, and Pistachio starts it when a chat uses it.",
        choice.model.name,
        choice.build.quant,
        choice.build.format_label(),
        choice.engine.name,
        choice.memory as f64 / 1e9,
    );
    if choice.streams {
        intro.push_str(" The model is larger than the memory: the engine reads parts of it from the disk as it goes.");
    }
    if !choice.is_easy() {
        intro.push_str(
            " The engine is built from source on this computer, which takes several minutes.",
        );
        if !choice.missing_tools.is_empty() {
            intro.push_str(" It needs ");
            intro.push_str(&choice.missing_tools.join(" and "));
            intro.push_str(", which this computer does not have yet; install it first.");
        }
    }
    let parts = parts
        .iter()
        .map(|part| DownloadPart {
            icon: if part.engine {
                AppIcon::Server.into()
            } else {
                AppIcon::Brain.into()
            },
            name: part.name.clone(),
            note: part.note.clone(),
            size: part.size,
            link: if part.engine {
                choice.engine.home_page()
            } else {
                Some(choice.build.page())
            },
        })
        .collect();
    // A build makes more than it downloads.
    let disk_size = if choice.is_easy() {
        size
    } else {
        size + 2_000_000_000
    };
    Details {
        intro,
        parts,
        size,
        disk_size,
        note: format!(
            "Into {}. License: {} for the model, {} for {}.",
            root.display(),
            choice.model.license,
            choice.engine.license,
            choice.engine.name
        ),
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::{SCAN_TIME, scan_progress};

    /// The bar fills in the scan's time, from empty, and stands full only once the scan ended.
    #[test]
    fn a_scan_fills_its_bar_in_its_time() {
        let at = |fraction: f32| SCAN_TIME.mul_f32(fraction);
        assert!(scan_progress(Duration::ZERO, false).abs() < 1e-6);
        assert!((scan_progress(at(0.5), false) - 0.5).abs() < 1e-6);
        assert!((scan_progress(at(0.5), true) - 0.5).abs() < 1e-6);
        assert!((scan_progress(at(1.), true) - 1.).abs() < 1e-6);
        assert!((scan_progress(at(3.), true) - 1.).abs() < 1e-6);
        // Still scanning: never full, always moving on.
        let mut last = 0.;
        for tenth in 1..100u16 {
            let now = scan_progress(at(f32::from(tenth) / 10.), false);
            assert!(now >= last && now < 1., "{tenth}: {now}");
            last = now;
        }
    }
}
