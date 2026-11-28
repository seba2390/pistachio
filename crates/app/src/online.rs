//! Whether the internet can be reached, so that downloads are offered only when it can.
//!
//! Nothing watches the network. It is asked when views that offer downloads render (at most
//! every [`FRESH`], or [`FRESH_OFFLINE`] while it could not be reached) and again right before a
//! download is asked for. Windows are redrawn when the answer changes.

use std::time::{Duration, Instant};

use gpui_kit::{App, Global, Window};

/// How long the question may take.
const TIMEOUT: Duration = Duration::from_secs(4);
/// How long an answer holds.
const FRESH: Duration = Duration::from_secs(30);
/// How long "not reachable" holds: the internet coming back is noticed sooner.
const FRESH_OFFLINE: Duration = Duration::from_secs(5);

/// What a download offers instead while the internet cannot be reached.
pub const OFFLINE_LABEL: &str = "No internet";
/// Says why, and that clicking asks again.
pub const OFFLINE_NOTE: &str =
    "Pistachio can't reach the internet, so nothing can be downloaded. Click to check again.";

type Waiter = Box<dyn FnOnce(bool, &mut App)>;

#[derive(Default)]
struct Net {
    offline: bool,
    checked: Option<Instant>,
    checking: bool,
    /// What waits for the answer being asked for.
    waiting: Vec<Waiter>,
}

impl Global for Net {}

impl Net {
    fn fresh(&self) -> bool {
        let holds = if self.offline { FRESH_OFFLINE } else { FRESH };
        self.checked.is_some_and(|at| at.elapsed() < holds)
    }
}

/// Whether the internet could not be reached when last asked; not known yet counts as reachable.
pub fn is_offline(cx: &App) -> bool {
    cx.try_global::<Net>().is_some_and(|net| net.offline)
}

/// Asks whether the internet can be reached, unless that was asked lately. For views that offer
/// downloads, as they render.
pub fn refresh(cx: &mut App) {
    if !cx.default_global::<Net>().fresh() {
        ask(cx, Box::new(|_, _| {}));
    }
}

/// Runs `then` once the internet is found reachable: right away when it was lately, else after
/// asking. When it is not, nothing runs, and what offers downloads says so.
pub fn when_online(
    window: &Window,
    cx: &mut App,
    then: impl FnOnce(&mut Window, &mut App) + 'static,
) {
    let net = cx.default_global::<Net>();
    if !net.offline && net.fresh() {
        let handle = window.window_handle();
        // Not in the middle of the click that asked.
        cx.defer(move |cx| {
            let _ = handle.update(cx, |_, window, cx| then(window, cx));
        });
        return;
    }
    let handle = window.window_handle();
    ask(
        cx,
        Box::new(move |online, cx| {
            if online {
                let _ = handle.update(cx, |_, window, cx| then(window, cx));
            }
        }),
    );
}

fn ask(cx: &mut App, waiter: Waiter) {
    let net = cx.default_global::<Net>();
    net.waiting.push(waiter);
    if net.checking {
        return;
    }
    net.checking = true;
    cx.spawn(async move |cx| {
        let online = cx.background_executor().spawn(async { probe() }).await;
        cx.update(|cx| answered(online, cx));
    })
    .detach();
}

fn answered(online: bool, cx: &mut App) {
    let net = cx.default_global::<Net>();
    let changed = net.offline == online || net.checked.is_none();
    net.offline = !online;
    net.checked = Some(Instant::now());
    net.checking = false;
    let waiting = std::mem::take(&mut net.waiting);
    for waiter in waiting {
        waiter(online, cx);
    }
    if changed {
        cx.refresh_windows();
    }
}

#[cfg(not(test))]
fn probe() -> bool {
    python_env::fetch::reachable(&python_env::fetch::REACHABLE, TIMEOUT)
}

#[cfg(test)]
thread_local! {
    /// Tests never ask the network: they say what it answers (GPUI's tests run on one thread).
    pub(crate) static REACHABLE: std::cell::Cell<bool> = const { std::cell::Cell::new(true) };
}

#[cfg(test)]
fn probe() -> bool {
    let _ = TIMEOUT;
    REACHABLE.with(std::cell::Cell::get)
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::rc::Rc;

    use gpui_kit::TestAppContext;

    use super::{REACHABLE, is_offline, when_online};
    use crate::ui::dialog::testing::window;

    /// A download asked for while the internet cannot be reached does not start, and starts
    /// once it can.
    #[gpui_kit::test]
    fn downloads_wait_for_the_internet(cx: &mut TestAppContext) {
        let cx = window(cx);
        let ran = Rc::new(Cell::new(0));
        let try_once = |reachable: bool, cx: &mut gpui_kit::VisualTestContext| {
            REACHABLE.with(|answer| answer.set(reachable));
            let ran = ran.clone();
            cx.update(|window, cx| {
                when_online(window, cx, move |_, _| ran.set(ran.get() + 1));
            });
            cx.run_until_parked();
        };
        try_once(false, cx);
        assert_eq!(ran.get(), 0);
        assert!(cx.update(|_, cx| is_offline(cx)));
        try_once(true, cx);
        assert_eq!(ran.get(), 1);
        assert!(!cx.update(|_, cx| is_offline(cx)));
        // Found reachable lately: not asked again.
        REACHABLE.with(|answer| answer.set(false));
        cx.update(|window, cx| {
            let ran = ran.clone();
            when_online(window, cx, move |_, _| ran.set(ran.get() + 1));
        });
        cx.run_until_parked();
        assert_eq!(ran.get(), 2);
        REACHABLE.with(|answer| answer.set(true));
    }
}
