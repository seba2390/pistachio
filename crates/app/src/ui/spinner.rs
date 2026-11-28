//! The "working" indicator: the little π with dots orbiting it, its glow and a shimmer on the
//! "Working…" label, in the style chosen in Settings → Appearance.
//!
//! Every style is a pure function of time ([`frame`]), so motion stays smooth however the
//! frames fall and every π on screen moves in step. Orbits are not uniform: like loading
//! spinners, a lap starts slowly at the top, whips round the bottom and slows again. Views
//! showing an indicator repaint every [`FRAME`] while an agent works, and not otherwise.

use std::sync::OnceLock;
use std::time::{Duration, Instant};

use gpui_kit::component::{Icon, IconName};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use serde::{Deserialize, Serialize};

use super::AppIcon;

/// How often a view showing an indicator repaints while an agent works.
pub const FRAME: Duration = Duration::from_millis(33);

/// The indicator styles offered in Settings.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum IndicatorStyle {
    Pulse,
    Eased,
    Comet,
    Chase,
    Atom,
    Hops,
    ShimmerComet,
    #[default]
    ShimmerChase,
    ShimmerSpin,
}

impl IndicatorStyle {
    pub const ALL: [Self; 9] = [
        Self::ShimmerChase,
        Self::ShimmerComet,
        Self::ShimmerSpin,
        Self::Eased,
        Self::Comet,
        Self::Chase,
        Self::Atom,
        Self::Hops,
        Self::Pulse,
    ];

    pub fn id(self) -> &'static str {
        match self {
            Self::Pulse => "pulse",
            Self::Eased => "eased",
            Self::Comet => "comet",
            Self::Chase => "chase",
            Self::Atom => "atom",
            Self::Hops => "hops",
            Self::ShimmerComet => "shimmer-comet",
            Self::ShimmerChase => "shimmer-chase",
            Self::ShimmerSpin => "shimmer-spin",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Pulse => "Pulse",
            Self::Eased => "Orbiting dot",
            Self::Comet => "Comet tail",
            Self::Chase => "Two dots chasing",
            Self::Atom => "Atom ring",
            Self::Hops => "Half-turn hops",
            Self::ShimmerComet => "Shimmer, glow and comet",
            Self::ShimmerChase => "Shimmer, glow and chasing dots",
            Self::ShimmerSpin => "Shimmer, glow and spinning π",
        }
    }
}

/// A dot, in π units (the π is 12 units wide) from the π's centre.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Dot {
    pub x: f32,
    pub y: f32,
    pub radius: f32,
    pub opacity: f32,
    /// Passes behind the π (drawn first).
    pub behind: bool,
}

/// How an indicator looks at one moment.
#[derive(Debug, Clone, PartialEq)]
pub struct Frame {
    /// The π's opacity (its glow).
    pub pi_opacity: f32,
    /// The π's turn, in radians.
    pub pi_turn: f32,
    /// The π's scale.
    pub pi_scale: f32,
    pub dots: Vec<Dot>,
    /// Where the shimmer's band is across the label (0 = left edge, 1 = right edge; it starts
    /// and ends off the text), if the style shimmers.
    pub shimmer: Option<f32>,
}

const TAU: f32 = std::f32::consts::TAU;
/// The orbit's radius and the dots' size, in π units.
const ORBIT: f32 = 10.5;
const DOT: f32 = 1.7;
/// The indicator's box, in π units: the orbit and a dot on either side.
const BOX: f32 = 2. * (ORBIT + DOT + 0.5);

fn ease_in_out_cubic(p: f32) -> f32 {
    if p < 0.5 {
        4. * p * p * p
    } else {
        1. - (-2. * p + 2.).powi(3) / 2.
    }
}

fn ease_in_out_sine(p: f32) -> f32 {
    (1. - (std::f32::consts::PI * p).cos()) / 2.
}

/// The angle `seconds` in, for one eased lap per `period`, starting at the top.
fn eased_angle(seconds: f32, period: f32) -> f32 {
    let laps = seconds / period;
    -TAU / 4. + TAU * (laps.floor() + ease_in_out_cubic(laps.fract()))
}

/// 0 → 1 → 0 over `period`, eased.
fn breath(seconds: f32, period: f32) -> f32 {
    (1. - (seconds / period * TAU).cos()) / 2.
}

fn on_orbit(angle: f32, radius: f32, opacity: f32) -> Dot {
    Dot {
        x: ORBIT * angle.cos(),
        y: ORBIT * angle.sin(),
        radius,
        opacity,
        behind: false,
    }
}

/// A dot with a fading tail; the tail stretches where the dot is fast.
fn comet(seconds: f32, period: f32, length: usize) -> Vec<Dot> {
    #[allow(clippy::cast_precision_loss)]
    (0..=length)
        .rev()
        .map(|k| {
            let k = k as f32;
            on_orbit(
                eased_angle(seconds - k * 0.022, period),
                DOT + 0.1 - k * 0.2,
                1. - k * 0.15,
            )
        })
        .collect()
}

/// Two dots on the same eased lap a beat apart: they bunch up at the top and spread out
/// through the fast part.
fn chase(seconds: f32, period: f32) -> Vec<Dot> {
    vec![
        on_orbit(eased_angle(seconds - 0.12, period), DOT - 0.4, 0.6),
        on_orbit(eased_angle(seconds, period), DOT + 0.1, 1.),
    ]
}

/// The indicator in `style`, `seconds` into its animation.
pub fn frame(style: IndicatorStyle, seconds: f32) -> Frame {
    let mut frame = Frame {
        pi_opacity: 1.,
        pi_turn: 0.,
        pi_scale: 1.,
        dots: Vec::new(),
        shimmer: None,
    };
    let glow = |period| 0.55 + 0.45 * breath(seconds, period);
    let shimmer = |period: f32| Some((seconds / period).fract());
    match style {
        IndicatorStyle::Pulse => frame.pi_scale = 0.72 + 0.36 * breath(seconds, 1.4),
        IndicatorStyle::Eased => frame.dots = vec![on_orbit(eased_angle(seconds, 1.3), DOT, 1.)],
        IndicatorStyle::Comet => frame.dots = comet(seconds, 1.3, 6),
        IndicatorStyle::Chase => frame.dots = chase(seconds, 1.5),
        IndicatorStyle::Atom => {
            // A tilted ring seen from the side: the dot passes behind the π.
            let laps = seconds / 1.4;
            let angle = TAU * (laps.floor() + ease_in_out_sine(laps.fract()));
            let tilt = -25f32.to_radians();
            let (x, y) = (11.5 * angle.cos(), 4.2 * angle.sin());
            let depth = f32::midpoint(angle.sin(), 1.);
            frame.dots = vec![Dot {
                x: x * tilt.cos() - y * tilt.sin(),
                y: x * tilt.sin() + y * tilt.cos(),
                radius: 1.2 + 0.7 * depth,
                opacity: 0.45 + 0.55 * depth,
                behind: angle.sin() < 0.,
            }];
        }
        IndicatorStyle::Hops => {
            // Two quick half turns, then a rest.
            let period = 1.6;
            let p = (seconds / period).fract();
            let half = if p < 0.35 {
                0.5 * ease_in_out_cubic(p / 0.35)
            } else if p < 0.7 {
                0.5 + 0.5 * ease_in_out_cubic((p - 0.35) / 0.35)
            } else {
                1.
            };
            let angle = -TAU / 4. + TAU * ((seconds / period).floor() + half);
            frame.dots = vec![on_orbit(angle, DOT, 1.)];
        }
        IndicatorStyle::ShimmerComet => {
            frame.pi_opacity = glow(1.3);
            frame.dots = comet(seconds, 1.3, 5);
            frame.shimmer = shimmer(2.6);
        }
        IndicatorStyle::ShimmerChase => {
            frame.pi_opacity = glow(1.5);
            frame.dots = chase(seconds, 1.5);
            frame.shimmer = shimmer(3.0);
        }
        IndicatorStyle::ShimmerSpin => {
            frame.pi_opacity = glow(1.4);
            frame.pi_turn = eased_angle(seconds, 1.4) + TAU / 4.;
            frame.shimmer = shimmer(2.8);
        }
    }
    frame
}

/// Opacity for something that breathes while running (a running tool call's icon), in step
/// with the indicators.
pub fn running_opacity() -> f32 {
    0.45 + 0.55 * breath(clock(), 1.4)
}

/// Seconds since the first indicator was drawn: the shared clock of every indicator.
fn clock() -> f32 {
    static START: OnceLock<Instant> = OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_secs_f32()
}

/// The indicator as it is now, for a π `size` wide, in `color`. Its box is a little larger
/// than the π (room for the orbit) and keeps its size, so nothing around it moves.
pub fn working_indicator(style: IndicatorStyle, size: Pixels, color: Hsla) -> Div {
    indicator_at(style, clock(), size, color)
}

fn indicator_at(style: IndicatorStyle, seconds: f32, size: Pixels, color: Hsla) -> Div {
    let frame = frame(style, seconds);
    let unit = size / 12.;
    let side = unit * BOX;
    let centre = side / 2.;
    let dot = |dot: &Dot| {
        let radius = unit * dot.radius.max(0.2);
        div()
            .absolute()
            .left(centre + unit * dot.x - radius)
            .top(centre + unit * dot.y - radius)
            .size(radius * 2.)
            .rounded_full()
            .bg(color.opacity(dot.opacity.clamp(0., 1.)))
    };
    let transform = Transformation::scale(gpui_kit::size(frame.pi_scale, frame.pi_scale))
        .with_rotation(radians(frame.pi_turn));
    div()
        .relative()
        .flex_none()
        .size(side)
        .children(frame.dots.iter().filter(|d| d.behind).map(dot))
        .child(
            div()
                .absolute()
                .left(centre - size / 2.)
                .top(centre - size / 2.)
                .size(size)
                .child(
                    Icon::from(AppIcon::PiLogo)
                        .size(size)
                        .text_color(color.opacity(frame.pi_opacity))
                        .when(
                            frame.pi_turn.abs() > 1e-4 || (frame.pi_scale - 1.).abs() > 1e-4,
                            |icon| icon.transform(transform),
                        ),
                ),
        )
        .children(frame.dots.iter().filter(|d| !d.behind).map(dot))
}

/// A loader turning, as it is now, for something being waited on (a scan, a download). It turns
/// also with reduced motion: the wait is real, and a still loader looks stuck. Views showing it
/// repaint every [`FRAME`] while they wait.
pub fn turning_loader(color: Hsla) -> Icon {
    const LAP: f32 = 0.8;
    let turn = TAU * ease_in_out_cubic((clock() / LAP).fract());
    Icon::new(IconName::Loader)
        .text_color(color)
        .transform(Transformation::rotate(radians(turn)))
}

/// A ring with an arc going round it, as it is now: something is being read. Views showing it
/// repaint every [`FRAME`], except with reduced motion, where it stands still.
pub fn ring(size: Pixels, color: Hsla, track: Hsla, cx: &App) -> Div {
    const STROKE: Pixels = px(2.);
    const LAP: f32 = 0.9;
    let start = if cx.reduce_motion() {
        -TAU / 4.
    } else {
        (clock() / LAP).fract() * TAU
    };
    div()
        .relative()
        .flex_none()
        .size(size)
        .rounded_full()
        .border(STROKE)
        .border_color(track)
        .child(
            canvas(
                |_, _, _| {},
                move |bounds, (), window, _| {
                    // The canvas is inside the border, which the arc covers.
                    let radius = bounds.size.width / 2. + STROKE / 2.;
                    let at = |angle: f32| {
                        bounds.center() + point(radius * angle.cos(), radius * angle.sin())
                    };
                    let mut arc = PathBuilder::stroke(STROKE);
                    arc.move_to(at(start));
                    arc.arc_to(
                        point(radius, radius),
                        px(0.),
                        false,
                        true,
                        at(start + TAU * 0.3),
                    );
                    if let Ok(arc) = arc.build() {
                        window.paint_path(arc, color);
                    }
                },
            )
            .size_full(),
        )
}

/// The "Working…" label: plain `color`, or with the style's shimmer passing over it.
pub fn working_label(style: IndicatorStyle, text: SharedString, color: Hsla) -> ShimmerLabel {
    label_at(style, clock(), text, color)
}

fn label_at(style: IndicatorStyle, seconds: f32, text: SharedString, color: Hsla) -> ShimmerLabel {
    let colors = match frame(style, seconds).shimmer {
        Some(band) => shimmer_colors(&text, band, color),
        None => vec![(0..text.len(), color)],
    };
    ShimmerLabel { text, colors }
}

/// One line of text whose colours change without moving it: shaped as a single run and
/// coloured only when painted. (A `StyledText` shapes every colour run apart, so each colour
/// change the shimmer made dropped the kerning there and the letters after it shifted.)
pub struct ShimmerLabel {
    text: SharedString,
    /// Colours over byte ranges covering the whole text, in order.
    colors: Vec<(std::ops::Range<usize>, Hsla)>,
}

impl ShimmerLabel {
    fn layout(&self, window: &Window) -> (std::sync::Arc<LineLayout>, Pixels) {
        let style = window.text_style();
        let rem = window.rem_size();
        let run = style.to_run(self.text.len());
        let line = window.text_system().layout_line(
            &self.text,
            style.font_size.to_pixels(rem),
            &[run],
            None,
        );
        (line, style.line_height_in_pixels(rem))
    }
}

impl Element for ShimmerLabel {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        let (line, line_height) = self.layout(window);
        let style = Style {
            size: size(line.width.into(), line_height.into()),
            flex_shrink: 0.,
            ..Style::default()
        };
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        (): &mut (),
        _: &mut Window,
        _: &mut App,
    ) {
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        (): &mut (),
        (): &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        let (line, line_height) = self.layout(window);
        let runs: Vec<DecorationRun> = self
            .colors
            .iter()
            .map(|(range, color)| DecorationRun {
                #[allow(clippy::cast_possible_truncation)]
                len: range.len() as u32,
                color: *color,
                background_color: None,
                underline: None,
                strikethrough: None,
            })
            .collect();
        let _ = line.paint(
            bounds.origin,
            line_height,
            TextAlign::Left,
            None,
            &runs,
            window,
            cx,
        );
    }
}

/// Per-character colours for a shimmer band at `band` (see [`Frame::shimmer`]) over `text`.
fn shimmer_colors(text: &str, band: f32, color: Hsla) -> Vec<(std::ops::Range<usize>, Hsla)> {
    let bright = Hsla {
        l: (color.l + 0.38).min(0.9),
        s: color.s * 0.8,
        ..color
    };
    // The band sweeps from just before the text to just after it.
    let centre = -0.35 + 1.7 * band;
    #[allow(clippy::cast_precision_loss)]
    let count = text.chars().count().max(1) as f32;
    text.char_indices()
        .enumerate()
        .map(|(index, (start, c))| {
            #[allow(clippy::cast_precision_loss)]
            let position = (index as f32 + 0.5) / count;
            let light = (1. - (position - centre).abs() / 0.22).clamp(0., 1.);
            let mix = |a: f32, b: f32| a + (b - a) * light;
            let tint = Hsla {
                h: color.h,
                s: mix(color.s, bright.s),
                l: mix(color.l, bright.l),
                a: color.a,
            };
            (start..start + c.len_utf8(), tint)
        })
        .collect()
}

/// A still π, for places that mark finished work ("Worked for 12s").
pub fn resting_pi(size: Pixels, color: Hsla) -> Div {
    div()
        .flex_none()
        .size(size)
        .child(Icon::from(AppIcon::PiLogo).size(size).text_color(color))
}

#[cfg(test)]
mod tests {
    use super::{IndicatorStyle, ORBIT, TAU, eased_angle, frame, shimmer_colors};

    #[test]
    fn laps_are_eased_slow_at_the_top_fast_at_the_bottom() {
        let period = 1.3;
        let step = 0.033;
        let speed = |t: f32| (eased_angle(t + step, period) - eased_angle(t, period)).abs();
        // Near the start of a lap (the top) it barely moves; mid-lap it is fastest.
        assert!(speed(0.) < speed(period / 2. - step / 2.) / 5.);
        // A full lap per period, always forwards.
        assert!((eased_angle(period, period) - eased_angle(0., period) - TAU).abs() < 1e-4);
        for i in 0..80 {
            #[allow(clippy::cast_precision_loss)]
            let t = i as f32 * step;
            assert!(eased_angle(t + step, period) >= eased_angle(t, period));
        }
    }

    #[test]
    fn dots_stay_on_the_orbit_and_inside_the_box() {
        for style in IndicatorStyle::ALL {
            for i in 0..120 {
                #[allow(clippy::cast_precision_loss)]
                let f = frame(style, i as f32 * 0.05);
                for dot in &f.dots {
                    let distance = dot.x.hypot(dot.y);
                    assert!(distance <= ORBIT + 1.5, "{style:?}: {distance}");
                    assert!((0. ..=1.).contains(&dot.opacity));
                }
                assert!((0.5..=1.).contains(&f.pi_opacity), "{style:?}");
            }
        }
    }

    #[test]
    fn styles_do_what_they_say() {
        assert_eq!(frame(IndicatorStyle::Chase, 0.4).dots.len(), 2);
        assert!(frame(IndicatorStyle::Comet, 0.4).dots.len() > 3);
        assert!(frame(IndicatorStyle::ShimmerChase, 0.4).shimmer.is_some());
        assert!(frame(IndicatorStyle::Chase, 0.4).shimmer.is_none());
        let spin = frame(IndicatorStyle::ShimmerSpin, 0.7);
        assert!(spin.dots.is_empty() && spin.pi_turn != 0.);
        // The atom's dot passes behind the π for part of each lap.
        let behind = (0..28).any(|i| {
            #[allow(clippy::cast_precision_loss)]
            let t = i as f32 * 0.05;
            frame(IndicatorStyle::Atom, t).dots[0].behind
        });
        assert!(behind);
        // Ids round-trip through settings.json.
        for style in IndicatorStyle::ALL {
            let json = serde_json::to_string(&style).unwrap_or_default();
            assert_eq!(json, format!("\"{}\"", style.id()));
        }
    }

    #[test]
    fn the_shimmer_lights_the_characters_under_its_band() {
        let green = gpui_kit::hsla(0.33, 0.65, 0.4, 1.);
        let colors = shimmer_colors("Working…", 0.5, green);
        assert_eq!(colors.len(), "Working…".chars().count());
        let lightness: Vec<f32> = colors.iter().map(|(_, color)| color.l).collect();
        let brightest = lightness
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.total_cmp(b.1))
            .map_or(0, |(i, _)| i);
        assert!((3..=5).contains(&brightest), "middle is lit: {lightness:?}");
        // Before the sweep starts nothing is lit.
        let idle = shimmer_colors("Working…", 0., green);
        assert!(idle.iter().all(|(_, color)| *color == green));
        // Byte ranges follow multi-byte characters (the ellipsis).
        assert_eq!(colors.last().map(|(r, _)| r.len()), Some('…'.len_utf8()));
    }
}
