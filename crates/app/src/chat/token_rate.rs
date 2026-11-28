//! The tokens-per-second pill at the top of the chat: while a reply streams, a line of its rate
//! over the last seconds and the rate now; when it ends, the exact figure, in gray, for a few
//! seconds. It slides down from under the top bar and back up. The measuring is in
//! [`transcript::rate`]; what it learns about each model is kept in
//! `<agent dir>/pistachio/token-rates.json`.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{ActiveTheme as _, h_flex};
use gpui_kit::*;
use transcript::rate::{Accuracy, HISTORY_LEN, History, RateCalibration, RateView};

use crate::theme::palette;

/// How long a finished reply's rate stays.
pub const HOLD: Duration = Duration::from_secs(4);
const SLIDE_IN: Duration = Duration::from_millis(420);
const SLIDE_OUT: Duration = Duration::from_millis(320);
/// Where the pill rests, and how far above that it hides (under the top bar).
const REST: f32 = 8.;
const TRAVEL: f32 = 38.;
const LINE: Size<Pixels> = size(px(60.), px(14.));
/// How quickly the line's vertical range follows the rate (the time to get most of the way).
const RANGE_EASE: Duration = Duration::from_millis(350);

/// What the app has learned about each model's replies.
#[derive(Default)]
pub struct TokenRates(pub RateCalibration);

impl Global for TokenRates {}

impl TokenRates {
    pub fn path() -> Option<PathBuf> {
        if cfg!(test) {
            return None;
        }
        pi_sessions::app_dir().map(|dir| dir.join("token-rates.json"))
    }

    pub fn load(path: Option<&Path>) -> Self {
        Self(path.map_or_default(crate::persist::load))
    }

    /// Saves it in the background.
    pub fn save(cx: &App) {
        let Some(path) = Self::path() else { return };
        let after = cx.global::<Self>().0.clone();
        crate::persist::save_in_background(cx, path, move |path| {
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir)?;
            }
            let tmp = path.with_extension(format!("json.{}.tmp", std::process::id()));
            std::fs::write(
                &tmp,
                serde_json::to_vec(&after).map_err(std::io::Error::other)?,
            )?;
            std::fs::rename(tmp, path)
        });
    }
}

/// The pill's state in a chat view.
#[derive(Default)]
pub struct Pill {
    /// Showing (or sliding in); false while it slides out.
    shown: bool,
    /// Counts its appearances, so each slides in (and out) once.
    appearance: u64,
    /// What it shows, kept while it slides out.
    last: Option<RateView>,
    /// Wakes the view when a finished rate has been shown for [`HOLD`].
    timer: Option<Task<()>>,
    /// The line's vertical range (low, high), eased toward the rate's so it doesn't jump, and
    /// when it was.
    range: Option<((f64, f64), Instant)>,
}

impl Pill {
    /// Brings it up to date with `view` at `now`. Returns how long until it should leave, when
    /// only time will make it.
    pub fn update(&mut self, view: Option<RateView>, now: Instant) -> Option<Duration> {
        let (visible, leave_in) = match &view {
            Some(RateView::Live { .. }) => (true, None),
            Some(RateView::Done(done)) => {
                let left = HOLD.saturating_sub(now.saturating_duration_since(done.at));
                (!left.is_zero(), Some(left).filter(|left| !left.is_zero()))
            }
            None => (false, None),
        };
        if visible {
            if !self.shown {
                self.shown = true;
                self.appearance += 1;
                self.range = None;
            }
            if let Some(target) = view.as_ref().and_then(|view| range(history(view))) {
                let eased = match self.range {
                    Some(((low, high), at)) => {
                        let elapsed = now.saturating_duration_since(at).as_secs_f64();
                        let k = 1. - (-elapsed / RANGE_EASE.as_secs_f64()).exp();
                        (low + (target.0 - low) * k, high + (target.1 - high) * k)
                    }
                    None => target,
                };
                self.range = Some((eased, now));
            }
            self.last = view;
        } else {
            self.shown = false;
        }
        leave_in
    }

    /// Wakes `view` after `wait`, unless it will be woken already.
    pub fn wake<V: 'static>(
        &mut self,
        wait: Duration,
        cx: &mut Context<V>,
        get: fn(&mut V) -> &mut Self,
    ) {
        if self.timer.is_some() {
            return;
        }
        self.timer = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(wait).await;
            let _ = this.update(cx, |view, cx| {
                get(view).timer = None;
                cx.notify();
            });
        }));
    }

    /// The pill, in a strip across the top of the transcript that hides it while it is above.
    pub fn render(&self, cx: &App) -> Option<AnyElement> {
        let view = self.last.as_ref()?;
        let theme = cx.theme();
        let (live, text, tip) = describe(view);
        let history = history(view).clone();
        let range = self.range.map(|(range, _)| range);
        let color = if live {
            palette(cx).accent
        } else {
            theme.muted_foreground
        };
        let pill = h_flex()
            .id("token-rate")
            .relative()
            .items_center()
            .gap_2()
            .px_2p5()
            .py(px(3.))
            .rounded_full()
            .bg(theme.muted)
            .text_xs()
            .child(sparkline(history, range, color))
            .child(div().min_w(px(62.)).text_color(color).child(text))
            .tooltip(move |window, cx| Tooltip::new(tip.clone()).build(window, cx));
        let pill = if self.shown {
            pill.with_animation(
                ElementId::Name(format!("token-rate-in-{}", self.appearance).into()),
                Animation::new(SLIDE_IN).with_easing(back_out),
                |this, delta| {
                    this.opacity(delta.clamp(0., 1.))
                        .top(px(REST - TRAVEL * (1. - delta)))
                },
            )
            .into_any_element()
        } else {
            pill.with_animation(
                ElementId::Name(format!("token-rate-out-{}", self.appearance).into()),
                Animation::new(SLIDE_OUT).with_easing(|t| t * t * t),
                |this, delta| this.opacity(1. - delta).top(px(REST - TRAVEL * delta)),
            )
            .into_any_element()
        };
        Some(
            h_flex()
                .absolute()
                .top_0()
                .left_0()
                .right_0()
                .h(px(REST + 28.))
                .overflow_hidden()
                .justify_center()
                .items_start()
                .child(pill)
                .into_any_element(),
        )
    }
}

/// Ease out with a small overshoot, so it settles like it dropped into place.
fn back_out(t: f32) -> f32 {
    const OVER: f32 = 1.2;
    let t = t - 1.;
    1. + (OVER + 1.) * t * t * t + OVER * t * t
}

fn history(view: &RateView) -> &History {
    match view {
        RateView::Live { history, .. } => history,
        RateView::Done(done) => &done.history,
    }
}

/// The range of values a line of `history` spans: small wobbles stay small, since it is at
/// least half the top.
fn range(history: &History) -> Option<(f64, f64)> {
    let values = || history.steps.iter().chain(&history.latest).copied();
    let high = values().reduce(f64::max)?;
    let low = values().reduce(f64::min)?;
    let span = (high - low).max(high / 2.).max(1.);
    Some(((high - span).max(0.), high))
}

/// Whether it is live, its label and its tooltip.
fn describe(view: &RateView) -> (bool, String, String) {
    match view {
        RateView::Live {
            per_second,
            accuracy,
            ..
        } => {
            let rate = per_second.round();
            let (text, tip) = match accuracy {
                Accuracy::Exact => (
                    format!("{rate} tok/s"),
                    "Exact: the provider counts the tokens as the model writes.".to_owned(),
                ),
                Accuracy::Estimate {
                    typical_error,
                    replies: 0,
                } => (
                    format!("~{rate} tok/s"),
                    format!(
                        "Only an estimate, from the text as it streams. Still learning this \
                         model's tokenizer: expect it to be off by about ±{:.0}%. The exact \
                         figure is shown when the reply ends.",
                        typical_error * 100.
                    ),
                ),
                Accuracy::Estimate {
                    typical_error,
                    replies,
                } => (
                    format!("~{rate} tok/s"),
                    format!(
                        "Only an estimate, from the text as it streams. For this model it has \
                         been off by ±{:.0}% on average (its last {replies} replies). The exact \
                         figure is shown when the reply ends.",
                        (typical_error * 100.).max(1.)
                    ),
                ),
            };
            (true, text, tip)
        }
        RateView::Done(done) if done.estimated => (
            false,
            format!("avg. ~{} tok/s", done.per_second.round()),
            format!(
                "The reply's average, estimated from its text: about {} tokens in {:.1} s. \
                 This model's server doesn't count them.",
                done.tokens, done.seconds
            ),
        ),
        RateView::Done(done) => (
            false,
            format!("avg. {} tok/s", done.per_second.round()),
            format!(
                "The reply's average, exact: {} tokens in {:.1} s, counted by the provider.",
                done.tokens, done.seconds
            ),
        ),
    }
}

/// The rate over the last seconds, the newest at the right edge. Values at whole steps stay
/// where they are and scroll left as time passes (`phase`), so only the newest end moves.
fn sparkline(history: History, range: Option<(f64, f64)>, color: Hsla) -> impl IntoElement {
    canvas(
        |_, _, _| {},
        #[allow(clippy::cast_precision_loss, clippy::cast_possible_truncation)] // small numbers
        move |bounds, (), window, _| {
            let Some((low, high)) = range else { return };
            let span = (high - low).max(1.);
            let step = bounds.size.width / (HISTORY_LEN - 1) as f32;
            let stroke = px(1.4);
            let height = bounds.size.height - stroke * 2.;
            let y =
                |value: f64| height * (1. - ((value - low) / span).clamp(0., 1.) as f32) + stroke;
            // Oldest first: the steps, then the newest moment at the right edge.
            let steps = history.steps.len();
            let mut points: Vec<(Pixels, Pixels)> = history
                .steps
                .iter()
                .enumerate()
                .map(|(index, value)| {
                    let back = (steps - 1 - index) as f32 + history.phase as f32;
                    (bounds.size.width - step * back, y(*value))
                })
                .collect();
            if let Some(latest) = history.latest {
                points.push((bounds.size.width, y(latest)));
            }
            // The oldest end slides out at the left edge.
            while points.len() > 2 && points[1].0 <= px(0.) {
                points.remove(0);
            }
            if points.len() < 2 {
                return;
            }
            if points[0].0 < px(0.) {
                let ((x0, y0), (x1, y1)) = (points[0], points[1]);
                let t = (px(0.) - x0) / (x1 - x0);
                points[0] = (px(0.), y0 + (y1 - y0) * t);
            }
            let mut line = PathBuilder::stroke(stroke);
            for (index, (x, y)) in points.into_iter().enumerate() {
                let at = bounds.origin + point(x, y);
                if index == 0 {
                    line.move_to(at);
                } else {
                    line.line_to(at);
                }
            }
            if let Ok(line) = line.build() {
                window.paint_path(line, color);
            }
        },
    )
    .w(LINE.width)
    .h(LINE.height)
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use super::{Accuracy, HOLD, Pill, RateView, describe, range};
    use transcript::rate::{Done, History};

    fn done(at: Instant) -> RateView {
        RateView::Done(Done {
            at,
            estimated: false,
            per_second: 40.,
            tokens: 400,
            seconds: 10.,
            history: History {
                steps: vec![38., 40., 42.],
                phase: 0.5,
                latest: Some(41.),
            },
        })
    }

    #[test]
    fn it_comes_once_per_appearance_and_leaves_after_the_hold() {
        let mut pill = Pill::default();
        let now = Instant::now();
        let live = RateView::Live {
            per_second: 40.,
            history: History::default(),
            accuracy: Accuracy::Exact,
        };
        assert_eq!(pill.update(Some(live.clone()), now), None);
        assert_eq!(pill.update(Some(live), now), None);
        assert_eq!((pill.shown, pill.appearance), (true, 1));
        // The reply ended: its figure stays for the hold, then the pill leaves.
        assert_eq!(pill.update(Some(done(now)), now), Some(HOLD));
        assert!(pill.shown);
        assert_eq!(pill.update(Some(done(now)), now + HOLD), None);
        assert!(!pill.shown);
        assert!(pill.last.is_some(), "kept while it slides out");
        assert_eq!(pill.update(Some(done(now)), now + HOLD * 2), None);
        assert_eq!(pill.appearance, 1);
    }

    #[test]
    fn the_tooltip_says_how_good_the_estimate_is() {
        let view = RateView::Live {
            per_second: 47.6,
            history: History::default(),
            accuracy: Accuracy::Estimate {
                typical_error: 0.083,
                replies: 12,
            },
        };
        let (live, text, tip) = describe(&view);
        assert!(live);
        assert_eq!(text, "~48 tok/s");
        assert!(tip.contains("Only an estimate"), "{tip}");
        assert!(
            tip.contains("±8% on average (its last 12 replies)"),
            "{tip}"
        );
        let (live, text, tip) = describe(&done(Instant::now()));
        assert!(!live);
        assert_eq!(text, "avg. 40 tok/s");
        assert!(tip.contains("exact: 400 tokens in 10.0 s"), "{tip}");
    }

    #[test]
    fn the_lines_range_keeps_small_wobbles_small_and_eases() {
        let history = |steps: Vec<f64>| History {
            steps,
            phase: 0.,
            latest: None,
        };
        assert_eq!(range(&history(vec![])), None);
        assert_eq!(range(&history(vec![58., 60.])), Some((30., 60.)));
        assert_eq!(range(&history(vec![10., 60.])), Some((10., 60.)));

        // A jump in the rate moves the range only part of the way at first.
        let live = |steps: Vec<f64>| RateView::Live {
            per_second: 0.,
            history: history(steps),
            accuracy: Accuracy::Exact,
        };
        let mut pill = Pill::default();
        let now = Instant::now();
        pill.update(Some(live(vec![58., 60.])), now);
        pill.update(
            Some(live(vec![58., 60., 120.])),
            now + std::time::Duration::from_millis(33),
        );
        let ((_, high), _) = pill.range.unwrap();
        assert!(60. < high && high < 70., "{high}");
    }
}
