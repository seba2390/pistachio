//! Tokens per second while a reply streams, and the exact figure when it ends.
//!
//! pi streams a reply's text, thinking and tool-call arguments, and with each update the output
//! tokens the provider has reported so far. Most providers report that count only at the end
//! (Gemini updates it as it goes), so while a reply streams its tokens are mostly estimated from
//! the text: [`units`] counts text the way BPE tokenizers split it (words, numbers, punctuation,
//! whitespace, CJK characters), and a scale learned per model from its finished replies
//! ([`RateCalibration`]) turns those units into that model's own tokens. Each model has its own
//! tokenizer, so the scale is per model; the first replies use [`DEFAULT_SCALE`].
//!
//! Text that never arrives can't be estimated: a model that thinks without streaming it, or
//! streams only a summary, writes far more tokens than its text accounts for. A finished reply
//! like that marks the model, at that thinking setting (off or on), as hidden, and no estimate is
//! shown for it. No estimate is shown either before one of its replies has checked out; only the
//! exact figure at the end, once it does. A provider that reports its count as it goes is exact
//! throughout.
//!
//! A reply that calls a tool is not judged: a tool call costs tokens that never stream (its
//! name, the markup around its arguments), so a short one looks hidden when it isn't. The latest
//! reply that is judged decides, so one odd reply doesn't hide a model for good. A server that
//! reports no count at all (some local engines) has its rate estimated with [`DEFAULT_SCALE`],
//! also at the end, and says so.

use std::collections::{BTreeMap, VecDeque};
use std::time::{Duration, Instant};

use pi_rpc::protocol::{
    AgentMessage, AssistantMessage, AssistantMessageEvent, ContentBlock, Event, StopReason,
    ThinkingLevel, Usage,
};
use serde::{Deserialize, Serialize};

/// Model tokens per [`units`] before a model has finished replies to learn from: `OpenAI`'s
/// `o200k_base` tokenizer on code, Markdown, JSON and replies in several languages.
pub const DEFAULT_SCALE: f64 = 0.87;
/// The typical error of an estimate while a model has fewer than [`MIN_ERRORS`] replies to judge
/// it by: about what its replies were off by right after their first calibration.
pub const EARLY_ERROR: f64 = 0.15;
/// The rate is measured over this much of the latest streaming.
const WINDOW: Duration = Duration::from_secs(2);
/// No rate until a reply has streamed this long (a burst or two says little).
const MIN_WINDOW: Duration = Duration::from_millis(500);
/// The rate history: measured each step since a reply's first piece, the latest this many.
pub const HISTORY_STEP: Duration = Duration::from_millis(250);
pub const HISTORY_LEN: usize = 40;
/// The longest gap between two pieces that the second's tokens are spread over.
const MAX_SPREAD: Duration = Duration::from_secs(1);
/// Points older than the history and its window are let go.
const KEEP: Duration = Duration::from_secs(13);
/// Shorter replies are too noisy to calibrate with or judge a model by.
const MIN_REPLY_TOKENS: u64 = 30;
/// The exact figure is shown for replies at least this long.
const MIN_DONE_TOKENS: u64 = 16;
const MIN_DONE_TIME: Duration = Duration::from_millis(500);
/// Replies a model's scale and typical error are taken from (the latest).
const MAX_SAMPLES: usize = 20;
/// Replies judged before a model's own typical error is shown.
pub const MIN_ERRORS: usize = 3;
/// More tokens than the text accounts for by this factor means some of the reply was hidden;
/// looser while the model's scale is still the default.
const HIDDEN_RATIO: f64 = 1.6;
const HIDDEN_RATIO_UNCALIBRATED: f64 = 2.2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Class {
    Space,
    /// Han, kana and Hangul: about a token per character.
    Cjk,
    Digit,
    /// ASCII letters.
    Latin,
    /// Other letters (accented Latin, Cyrillic, Greek, …), which tokenizers split more finely.
    Letter,
    Symbol,
}

fn class(c: char) -> Class {
    let code = u32::from(c);
    if c.is_whitespace() {
        Class::Space
    } else if (0x3040..=0x30ff).contains(&code)
        || (0x3400..=0x9fff).contains(&code)
        || (0xac00..=0xd7af).contains(&code)
        || (0xf900..=0xfaff).contains(&code)
    {
        Class::Cjk
    } else if c.is_numeric() {
        Class::Digit
    } else if c.is_ascii_alphabetic() {
        Class::Latin
    } else if c.is_alphabetic() {
        Class::Letter
    } else {
        Class::Symbol
    }
}

/// Where the last run of one class starts in `text` (each CJK character is a run of its own).
fn last_run_start(text: &str) -> usize {
    let mut start = 0;
    let mut previous = None;
    for (at, c) in text.char_indices() {
        let class = class(c);
        if previous != Some(class) || class == Class::Cjk {
            start = at;
        }
        previous = Some(class);
    }
    start
}

#[allow(clippy::cast_precision_loss)]
fn run_units(class: Class, run: &str) -> f64 {
    let len = run.chars().count() as f64;
    match class {
        // Common words are one token; longer ones split every few letters.
        Class::Latin => 1. + (len - 6.).max(0.) / 4.,
        Class::Letter => 1. + (len - 1.) / 3.,
        // Numbers split into groups of up to three digits.
        Class::Digit => (len / 3.).ceil(),
        Class::Symbol => 1. + (len - 1.) / 2.,
        // A single space joins the word after it.
        Class::Space if run == " " => 0.,
        Class::Space => 1.,
        Class::Cjk => len,
    }
}

/// How many tokens `text` is, in tokenizer-neutral units: words, numbers, punctuation and
/// whitespace runs, and CJK characters, counted the way BPE tokenizers tend to split them.
/// Multiplied by a model's scale ([`ModelRate::scale`]) it estimates that model's tokens.
/// Additive at run boundaries, so text can be counted as it streams ([`Counter`]).
pub fn units(text: &str) -> f64 {
    let mut total = 0.;
    let mut rest = text;
    while !rest.is_empty() {
        let first = rest.chars().next().map_or(Class::Symbol, class);
        let end = if first == Class::Cjk {
            rest.chars().next().map_or(rest.len(), char::len_utf8)
        } else {
            rest.char_indices()
                .find(|(_, c)| class(*c) != first)
                .map_or(rest.len(), |(at, _)| at)
        };
        total += run_units(first, &rest[..end]);
        rest = &rest[end..];
    }
    total
}

/// [`units`] of text that arrives in pieces: a run cut between two pieces is counted once.
#[derive(Debug, Default, Clone)]
struct Counter {
    counted: f64,
    /// The last run, which the next piece may continue.
    tail: String,
}

impl Counter {
    fn push(&mut self, piece: &str) {
        self.tail.push_str(piece);
        let start = last_run_start(&self.tail);
        if start > 0 {
            self.counted += units(&self.tail[..start]);
            self.tail.drain(..start);
        }
    }

    fn units(&self) -> f64 {
        self.counted + units(&self.tail)
    }
}

/// What the app has learned about each model's replies, kept between launches.
#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct RateCalibration {
    /// By `provider/model`.
    pub models: BTreeMap<String, ModelRate>,
}

impl RateCalibration {
    pub fn model(&self, key: &str) -> Option<&ModelRate> {
        self.models.get(key)
    }
}

/// One model's calibration.
#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ModelRate {
    /// Its latest replies that showed all they wrote (newest last).
    samples: VecDeque<Sample>,
    /// How far off the estimate of each of its latest replies was, relative to the exact count,
    /// with the scale from the replies before it (newest last).
    errors: VecDeque<f64>,
    /// Its provider reports output tokens as the reply streams: no estimate needed.
    pub streams_usage: bool,
    /// Its server reported no output tokens for a reply that wrote some: only estimates.
    pub uncounted: bool,
    /// Whether its replies show all they write, by thinking setting (`off`, `on`); missing until
    /// a reply tells.
    visible: BTreeMap<String, bool>,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
struct Sample {
    tokens: u64,
    units: f64,
}

impl ModelRate {
    /// Its tokens per [`units`], from its latest replies (weighted by length).
    #[allow(clippy::cast_precision_loss)]
    pub fn scale(&self) -> f64 {
        let units: f64 = self.samples.iter().map(|sample| sample.units).sum();
        if units <= 0. {
            return DEFAULT_SCALE;
        }
        let tokens: u64 = self.samples.iter().map(|sample| sample.tokens).sum();
        tokens as f64 / units
    }

    /// The average error of its latest estimates and how many replies that is from, once there
    /// are [`MIN_ERRORS`].
    #[allow(clippy::cast_precision_loss)]
    pub fn typical_error(&self) -> Option<(f64, usize)> {
        (self.errors.len() >= MIN_ERRORS).then(|| {
            (
                self.errors.iter().sum::<f64>() / self.errors.len() as f64,
                self.errors.len(),
            )
        })
    }

    /// Whether its replies at this thinking setting are known to show all they write.
    pub fn is_visible(&self, thinking: bool) -> bool {
        self.visible.get(thinking_key(thinking)) == Some(&true)
    }

    fn shows(&self, thinking: bool) -> bool {
        self.streams_usage || self.uncounted || self.is_visible(thinking)
    }
}

fn thinking_key(thinking: bool) -> &'static str {
    if thinking { "on" } else { "off" }
}

fn push_capped<T>(list: &mut VecDeque<T>, value: T) {
    list.push_back(value);
    while list.len() > MAX_SAMPLES {
        list.pop_front();
    }
}

/// How sure a live rate is.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Accuracy {
    /// Counted by the provider.
    Exact,
    /// Estimated from the text: `typical_error` (a fraction) is the average error of this
    /// model's latest estimates, measured on `replies` replies; 0 replies: not measured yet, the
    /// error is [`EARLY_ERROR`].
    Estimate { typical_error: f64, replies: usize },
}

/// A finished reply's rate: exact, unless its server counts nothing.
#[derive(Debug, Clone, PartialEq)]
pub struct Done {
    /// When the reply ended.
    pub at: Instant,
    /// Estimated from its text: its server reports no count.
    pub estimated: bool,
    pub per_second: f64,
    /// Output tokens from its first streamed token to its end, and how long that took.
    pub tokens: u64,
    pub seconds: f64,
    /// Its rate over its last seconds.
    pub history: History,
}

/// A reply's rate over its last seconds, for a line that scrolls smoothly: the values at whole
/// steps never change once measured, so only its newest end moves.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct History {
    /// The rate at each [`HISTORY_STEP`] since its first piece, the latest [`HISTORY_LEN`]
    /// (oldest first).
    pub steps: Vec<f64>,
    /// How far past the last of `steps` the newest moment is, as a fraction of a step.
    pub phase: f64,
    /// The rate at the newest moment (live: now; finished: its end).
    pub latest: Option<f64>,
}

/// What to show.
#[derive(Debug, Clone, PartialEq)]
pub enum RateView {
    Live {
        per_second: f64,
        history: History,
        accuracy: Accuracy,
    },
    Done(Done),
}

/// Measures the conversation's replies (not those of tools that run models themselves); feed it
/// every event ([`RateMeter::apply`]).
#[derive(Debug, Default)]
pub struct RateMeter {
    reply: Option<Reply>,
    done: Option<Done>,
}

#[derive(Debug)]
struct Reply {
    model: String,
    thinking: bool,
    /// When its first text, thinking or tool-call piece arrived.
    first_at: Option<Instant>,
    /// By content index: whether it is thinking, and its text so far.
    blocks: BTreeMap<usize, (bool, Counter)>,
    /// What had arrived at each piece (oldest first); one point older than [`KEEP`] stays, as
    /// the base of the oldest window.
    points: VecDeque<Point>,
    /// The output tokens reported at its first piece, and the latest.
    usage_start: u64,
    usage: u64,
    /// How often the reported output grew while it streamed.
    usage_moves: u32,
    /// It calls a tool.
    calls_tools: bool,
}

#[derive(Debug, Clone, Copy)]
struct Point {
    at: Instant,
    units: f64,
    usage: u64,
}

impl Reply {
    fn new(message: &AssistantMessage) -> Self {
        Self {
            model: model_key(message),
            thinking: !matches!(message.thinking_level, None | Some(ThinkingLevel::Off)),
            first_at: None,
            blocks: BTreeMap::new(),
            points: VecDeque::new(),
            usage_start: message.usage.output,
            usage: message.usage.output,
            usage_moves: 0,
            calls_tools: false,
        }
    }

    fn update(&mut self, usage: Option<&Usage>, event: &AssistantMessageEvent, now: Instant) {
        let (index, thinking, piece) = match event {
            AssistantMessageEvent::TextDelta {
                content_index,
                delta,
            } => (*content_index, false, delta),
            AssistantMessageEvent::ToolcallDelta {
                content_index,
                delta,
            } => {
                self.calls_tools = true;
                (*content_index, false, delta)
            }
            AssistantMessageEvent::ThinkingDelta {
                content_index,
                delta,
            } => (*content_index, true, delta),
            _ => return,
        };
        let reported = usage.map_or(self.usage, |usage| usage.output);
        if self.first_at.is_none() {
            self.first_at = Some(now);
            self.usage_start = reported;
        } else if reported > self.usage {
            self.usage_moves += 1;
        }
        self.usage = self.usage.max(reported);
        self.blocks
            .entry(index)
            .or_insert_with(|| (thinking, Counter::default()))
            .1
            .push(piece);
        self.points.push_back(Point {
            at: now,
            units: self.units(),
            usage: self.usage,
        });
        while self
            .points
            .get(1)
            .is_some_and(|second| second.at + KEEP < now)
        {
            self.points.pop_front();
        }
    }

    fn units(&self) -> f64 {
        self.blocks
            .values()
            .map(|(_, counter)| counter.units())
            .sum()
    }

    fn thinking_units(&self) -> f64 {
        self.blocks
            .values()
            .filter(|(thinking, _)| *thinking)
            .map(|(_, counter)| counter.units())
            .sum()
    }

    /// Two reported counts that grew while it streamed: the provider counts as it goes.
    fn counts_live(&self, model: Option<&ModelRate>) -> bool {
        self.usage_moves >= 2 || model.is_some_and(|model| model.streams_usage)
    }

    /// Tokens streamed by `at` (none before the first piece). A piece's tokens were written
    /// since the piece before it, so they are spread over that gap (up to [`MAX_SPREAD`]):
    /// pieces come in bursts, which would otherwise make the rate jump.
    #[allow(clippy::cast_precision_loss)]
    fn tokens_at(&self, at: Instant, scale: f64, exact: bool) -> f64 {
        let value = |point: &Point| {
            if exact {
                point.usage.saturating_sub(self.usage_start) as f64
            } else {
                point.units * scale
            }
        };
        let next = self.points.partition_point(|point| point.at <= at);
        let Some(before) = next.checked_sub(1).and_then(|index| self.points.get(index)) else {
            return 0.;
        };
        match self.points.get(next) {
            Some(after) if after.at.duration_since(before.at) <= MAX_SPREAD => {
                let gap = after.at.duration_since(before.at).as_secs_f64();
                let part = at.duration_since(before.at).as_secs_f64() / gap;
                value(before) + (value(after) - value(before)) * part
            }
            _ => value(before),
        }
    }

    /// Tokens per second over the [`WINDOW`] before `at`.
    fn rate_at(&self, at: Instant, scale: f64, exact: bool) -> Option<f64> {
        let first = self.first_at?;
        let span = at.checked_duration_since(first)?.min(WINDOW);
        if span < MIN_WINDOW {
            return None;
        }
        let before = at.checked_sub(span)?;
        let tokens = self.tokens_at(at, scale, exact) - self.tokens_at(before, scale, exact);
        Some((tokens / span.as_secs_f64()).max(0.))
    }

    #[allow(clippy::cast_possible_truncation)] // steps of a reply: far below u32::MAX
    fn history(&self, now: Instant, scale: f64, exact: bool) -> History {
        let Some(first) = self.first_at else {
            return History::default();
        };
        let since = now.saturating_duration_since(first);
        let steps = (since.as_nanos() / HISTORY_STEP.as_nanos()) as u32;
        let phase =
            since.saturating_sub(HISTORY_STEP * steps).as_secs_f64() / HISTORY_STEP.as_secs_f64();
        let oldest = steps.saturating_sub(HISTORY_LEN as u32 - 1);
        History {
            steps: (oldest..=steps)
                .filter_map(|step| self.rate_at(first + HISTORY_STEP * step, scale, exact))
                .collect(),
            phase,
            latest: self.rate_at(now, scale, exact),
        }
    }
}

fn model_key(message: &AssistantMessage) -> String {
    format!("{}/{}", message.provider, message.model)
}

impl RateMeter {
    /// Takes `event` in; at a reply's end, learns from it in `calibration` and says whether that
    /// changed (to be saved).
    pub fn apply(
        &mut self,
        event: &Event,
        now: Instant,
        calibration: &mut RateCalibration,
    ) -> bool {
        match event {
            Event::MessageStart {
                message: AgentMessage::Assistant(message),
                parent_tool_call_id: None,
            } => {
                self.reply = Some(Reply::new(message));
                false
            }
            Event::MessageUpdate {
                usage,
                assistant_message_event,
                parent_tool_call_id: None,
            } => {
                if let Some(reply) = &mut self.reply {
                    reply.update(usage.as_ref(), assistant_message_event, now);
                }
                false
            }
            Event::MessageEnd {
                message: AgentMessage::Assistant(message),
                parent_tool_call_id: None,
            } => self
                .reply
                .take()
                .is_some_and(|reply| self.finish(&reply, message, now, calibration)),
            Event::AgentEnd { .. } | Event::AgentSettled { .. } => {
                self.reply = None;
                false
            }
            _ => false,
        }
    }

    #[allow(clippy::cast_precision_loss)]
    fn finish(
        &mut self,
        reply: &Reply,
        message: &AssistantMessage,
        now: Instant,
        calibration: &mut RateCalibration,
    ) -> bool {
        let Some(first_at) = reply.first_at else {
            return false;
        };
        if !matches!(
            message.stop_reason,
            StopReason::Stop | StopReason::ToolUse | StopReason::Length
        ) {
            return false;
        }
        let model = calibration.models.entry(reply.model.clone()).or_default();
        let mut changed = false;
        if reply.usage_moves >= 2 && !model.streams_usage {
            model.streams_usage = true;
            changed = true;
        }
        let tokens = message.usage.output;
        let units = reply.units();
        let key = thinking_key(reply.thinking);
        let calls_tools = reply.calls_tools
            || message
                .content
                .iter()
                .any(|block| matches!(block, ContentBlock::ToolCall(_)));
        // Wrote something, and its server counted none of it.
        let uncounted = tokens == 0 && units * DEFAULT_SCALE >= MIN_REPLY_TOKENS as f64;
        if uncounted != model.uncounted && (uncounted || tokens > 0) {
            model.uncounted = uncounted;
            changed = true;
        }
        if tokens >= MIN_REPLY_TOKENS && units > 0. && !calls_tools {
            let scale = model.scale();
            let ratio = tokens as f64 / (units * scale);
            let limit = if model.samples.is_empty() {
                HIDDEN_RATIO_UNCALIBRATED
            } else {
                HIDDEN_RATIO
            };
            // Thinking the provider counted but that didn't arrive (or only summarized).
            let reasoning = message.usage.reasoning as f64;
            let hidden_thinking =
                reasoning > 0. && reply.thinking_units() * scale < 0.5 * reasoning;
            if ratio > limit || hidden_thinking {
                changed |= model.visible.insert(key.to_owned(), false) != Some(false);
            } else if ratio >= 1. / limit {
                if !model.samples.is_empty() {
                    push_capped(&mut model.errors, (1. / ratio - 1.).abs());
                }
                push_capped(&mut model.samples, Sample { tokens, units });
                // The latest judged reply decides.
                model.visible.insert(key.to_owned(), true);
                changed = true;
            }
        }

        let estimated = model.uncounted;
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // a reply's tokens
        let produced = if estimated {
            (units * model.scale()).round() as u64
        } else {
            tokens.saturating_sub(reply.usage_start)
        };
        let elapsed = now.saturating_duration_since(first_at);
        if model.shows(reply.thinking) && produced >= MIN_DONE_TOKENS && elapsed >= MIN_DONE_TIME {
            let exact = !estimated && reply.counts_live(Some(model));
            self.done = Some(Done {
                at: now,
                estimated,
                per_second: produced as f64 / elapsed.as_secs_f64(),
                tokens: produced,
                seconds: elapsed.as_secs_f64(),
                history: reply.history(now, model.scale(), exact),
            });
        }
        changed
    }

    /// What to show at `now`: the streaming reply's rate when its model can be measured,
    /// otherwise the last finished reply's.
    pub fn view(&self, now: Instant, calibration: &RateCalibration) -> Option<RateView> {
        if let Some(live) = self.live(now, calibration) {
            return Some(live);
        }
        self.done.clone().map(RateView::Done)
    }

    fn live(&self, now: Instant, calibration: &RateCalibration) -> Option<RateView> {
        let reply = self.reply.as_ref()?;
        let model = calibration.model(&reply.model);
        let uncounted = model.is_some_and(|model| model.uncounted);
        let exact = !uncounted && reply.counts_live(model);
        if !exact && !uncounted && !model.is_some_and(|model| model.is_visible(reply.thinking)) {
            return None;
        }
        let scale = model.map_or(DEFAULT_SCALE, ModelRate::scale);
        let per_second = reply.rate_at(now, scale, exact)?;
        let accuracy = if exact {
            Accuracy::Exact
        } else {
            let (typical_error, replies) = model
                .and_then(ModelRate::typical_error)
                .unwrap_or((EARLY_ERROR, 0));
            Accuracy::Estimate {
                typical_error,
                replies,
            }
        };
        Some(RateView::Live {
            per_second,
            history: reply.history(now, scale, exact),
            accuracy,
        })
    }

    /// Whether a reply is streaming (its rate may change without an event).
    pub fn is_streaming(&self) -> bool {
        self.reply
            .as_ref()
            .is_some_and(|reply| reply.first_at.is_some())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use serde_json::json;

    fn start(model: &str, thinking: &str) -> Event {
        serde_json::from_value(json!({
            "type": "message_start",
            "message": {"role": "assistant", "content": [], "provider": "p", "model": model,
                        "thinkingLevel": thinking, "usage": {"output": 1}}
        }))
        .unwrap()
    }

    fn delta(kind: &str, text: &str, output: u64) -> Event {
        serde_json::from_value(json!({
            "type": "message_update",
            "usage": {"output": output},
            "assistantMessageEvent": {"type": kind, "contentIndex": 0, "delta": text}
        }))
        .unwrap()
    }

    fn end(model: &str, output: u64, reasoning: u64) -> Event {
        serde_json::from_value(json!({
            "type": "message_end",
            "message": {"role": "assistant", "content": [], "provider": "p", "model": model,
                        "stopReason": "stop", "usage": {"output": output, "reasoning": reasoning}}
        }))
        .unwrap()
    }

    /// Streams `pieces` pieces of prose, one each 100 ms, and ends the reply with `output`
    /// tokens; returns when it ended.
    #[allow(clippy::too_many_arguments)]
    fn reply(
        meter: &mut RateMeter,
        calibration: &mut RateCalibration,
        at: Instant,
        model: &str,
        thinking: &str,
        pieces: usize,
        output: u64,
        live_usage: bool,
    ) -> Instant {
        meter.apply(&start(model, thinking), at, calibration);
        let mut now = at;
        for piece in 0..pieces {
            now += Duration::from_millis(100);
            let usage = if live_usage { 1 + 4 * piece as u64 } else { 1 };
            meter.apply(
                &delta("text_delta", "the quick fox ", usage),
                now,
                calibration,
            );
        }
        meter.apply(&end(model, output, 0), now, calibration);
        now
    }

    fn assert_close(actual: f64, expected: f64) {
        assert!((actual - expected).abs() < 1e-9, "{actual} vs {expected}");
    }

    #[test]
    fn units_follow_how_tokenizers_split_text() {
        assert_close(units("hello world"), 2.);
        assert_close(units("internationalization"), 1. + 14. / 4.);
        assert_close(units("1234567"), 3.);
        assert_close(units("a, b"), 3.);
        assert_close(units("\n\n    x"), 2.);
        assert_close(units("日本語"), 3.);
    }

    #[test]
    fn streamed_pieces_count_like_the_whole_text() {
        let text = "fn main() {\n    let answer = 42_000; // the 日本 answer\n    println!(\"{answer}\");\n}\n";
        for size in 1..12 {
            let mut counter = Counter::default();
            let chars: Vec<char> = text.chars().collect();
            for piece in chars.chunks(size) {
                counter.push(&piece.iter().collect::<String>());
            }
            assert!(
                (counter.units() - units(text)).abs() < 1e-9,
                "pieces of {size}: {} vs {}",
                counter.units(),
                units(text)
            );
        }
    }

    #[test]
    fn a_new_model_shows_only_its_exact_rate_and_then_its_estimate() {
        let mut meter = RateMeter::default();
        let mut calibration = RateCalibration::default();
        let at = Instant::now();
        // 30 pieces of "the quick fox " (3 units each) in 3 s; 80 tokens is a plausible count.
        meter.apply(&start("m", "off"), at, &mut calibration);
        meter.apply(
            &delta("text_delta", "the quick fox ", 1),
            at,
            &mut calibration,
        );
        // Unknown model: no estimate while it streams.
        assert_eq!(meter.view(at + Duration::from_secs(1), &calibration), None);
        let mut now = at;
        for _ in 1..30 {
            now += Duration::from_millis(100);
            meter.apply(
                &delta("text_delta", "the quick fox ", 1),
                now,
                &mut calibration,
            );
        }
        assert!(meter.apply(&end("m", 81, 0), now, &mut calibration));
        let Some(RateView::Done(done)) = meter.view(now, &calibration) else {
            panic!("no exact rate");
        };
        assert_eq!(done.tokens, 80);
        assert!((done.per_second - 80. / 2.9).abs() < 1e-6);
        let model = calibration.model("p/m").unwrap();
        assert!(model.is_visible(false));
        assert!((model.scale() - 81. / 90.).abs() < 1e-9);

        // The next reply is estimated with that scale, as it streams.
        let next = now + Duration::from_secs(5);
        meter.apply(&start("m", "off"), next, &mut calibration);
        let mut now = next;
        for _ in 0..20 {
            now += Duration::from_millis(100);
            meter.apply(
                &delta("text_delta", "the quick fox ", 1),
                now,
                &mut calibration,
            );
        }
        let Some(RateView::Live {
            per_second,
            accuracy,
            history,
        }) = meter.view(now, &calibration)
        else {
            panic!("no estimate");
        };
        // 3 units per 100 ms at 0.9 tokens per unit.
        assert!((per_second - 27.).abs() < 1e-6, "{per_second}");
        assert_ne!(history.steps.len(), 0);
        assert_eq!(history.latest, Some(per_second));
        assert_eq!(
            accuracy,
            Accuracy::Estimate {
                typical_error: EARLY_ERROR,
                replies: 0
            }
        );
    }

    #[test]
    fn typical_error_comes_from_the_models_own_replies() {
        let mut meter = RateMeter::default();
        let mut calibration = RateCalibration::default();
        let mut at = Instant::now();
        for output in [90, 99, 81, 90] {
            at = reply(
                &mut meter,
                &mut calibration,
                at,
                "m",
                "off",
                30,
                output,
                false,
            ) + Duration::from_secs(1);
        }
        let (error, replies) = calibration.model("p/m").unwrap().typical_error().unwrap();
        assert_eq!(replies, 3);
        // 99 vs 90, 81 vs ~94.5, 90 vs ~90: about 9%, 14% and 0%.
        assert!((0.05..0.12).contains(&error), "{error}");
    }

    #[test]
    fn hidden_thinking_stops_the_estimate_for_that_setting_only() {
        let mut meter = RateMeter::default();
        let mut calibration = RateCalibration::default();
        let at = Instant::now();
        let at = reply(&mut meter, &mut calibration, at, "m", "off", 30, 90, false);
        // Thinking on: ten times the tokens the text accounts for.
        let at = reply(
            &mut meter,
            &mut calibration,
            at,
            "m",
            "high",
            30,
            900,
            false,
        );
        let model = calibration.model("p/m").unwrap();
        assert!(model.is_visible(false));
        assert!(!model.is_visible(true));
        assert_eq!(model.samples.len(), 1);

        meter.apply(&start("m", "high"), at, &mut calibration);
        for step in 1..20 {
            meter.apply(
                &delta("text_delta", "the quick fox ", 1),
                at + Duration::from_millis(100 * step),
                &mut calibration,
            );
        }
        // Nothing live; the last exact figure (from the visible reply) stays.
        assert!(matches!(
            meter.view(at + Duration::from_secs(2), &calibration),
            Some(RateView::Done(_))
        ));
    }

    #[test]
    fn reported_reasoning_that_did_not_stream_hides_the_estimate() {
        let mut meter = RateMeter::default();
        let mut calibration = RateCalibration::default();
        let at = Instant::now();
        meter.apply(&start("o", "medium"), at, &mut calibration);
        for step in 1..30 {
            meter.apply(
                &delta("text_delta", "the quick fox ", 1),
                at + Duration::from_millis(100 * step),
                &mut calibration,
            );
        }
        // 160 tokens: within the uncalibrated limit, but 80 of them were reasoning that never
        // arrived.
        meter.apply(
            &end("o", 160, 80),
            at + Duration::from_secs(3),
            &mut calibration,
        );
        assert!(!calibration.model("p/o").unwrap().is_visible(true));
    }

    #[test]
    fn a_provider_that_counts_as_it_goes_is_exact_from_the_start() {
        let mut meter = RateMeter::default();
        let mut calibration = RateCalibration::default();
        let at = Instant::now();
        meter.apply(&start("g", "high"), at, &mut calibration);
        let mut now = at;
        for piece in 0..20 {
            now += Duration::from_millis(100);
            // 4 tokens per 100 ms.
            meter.apply(
                &delta("text_delta", "the quick fox ", 10 + 4 * piece),
                now,
                &mut calibration,
            );
        }
        let Some(RateView::Live {
            per_second,
            accuracy,
            ..
        }) = meter.view(now, &calibration)
        else {
            panic!("no live rate");
        };
        assert_eq!(accuracy, Accuracy::Exact);
        assert!((per_second - 40.).abs() < 1e-6, "{per_second}");
        meter.apply(&end("g", 86, 0), now, &mut calibration);
        assert!(calibration.model("p/g").unwrap().streams_usage);
    }

    /// Streams a reply that only calls a tool: a few units of arguments, and `output` tokens
    /// (the call's markup and name never stream).
    fn tool_call(
        meter: &mut RateMeter,
        calibration: &mut RateCalibration,
        at: Instant,
        output: u64,
    ) -> Instant {
        meter.apply(&start("m", "off"), at, calibration);
        let mut now = at;
        for piece in ["{\"query\": ", "\"weather in ", "Copenhagen\"}"] {
            now += Duration::from_millis(100);
            meter.apply(&delta("toolcall_delta", piece, 1), now, calibration);
        }
        let end: Event = serde_json::from_value(json!({
            "type": "message_end",
            "message": {"role": "assistant", "provider": "p", "model": "m",
                        "content": [{"type": "toolCall", "id": "1", "name": "web_search",
                                     "arguments": {"query": "weather in Copenhagen"}}],
                        "stopReason": "toolUse", "usage": {"output": output}}
        }))
        .unwrap();
        meter.apply(&end, now, calibration);
        now
    }

    /// A short tool call costs many more tokens than its arguments account for; that is not
    /// hidden text, and the estimate goes on (as with a local model searching the web).
    #[test]
    fn a_tool_call_does_not_hide_the_estimate() {
        let mut meter = RateMeter::default();
        let mut calibration = RateCalibration::default();
        let at = Instant::now();
        let at = reply(&mut meter, &mut calibration, at, "m", "off", 30, 90, false);
        let at = tool_call(&mut meter, &mut calibration, at, 40);
        let model = calibration.model("p/m").unwrap();
        assert!(model.is_visible(false));
        assert_eq!(model.samples.len(), 1, "a tool call is not a sample");
        let at = reply(&mut meter, &mut calibration, at, "m", "off", 30, 90, false);
        meter.apply(&start("m", "off"), at, &mut calibration);
        for step in 1..20 {
            meter.apply(
                &delta("text_delta", "the quick fox ", 1),
                at + Duration::from_millis(100 * step),
                &mut calibration,
            );
        }
        assert!(matches!(
            meter.view(at + Duration::from_secs(2), &calibration),
            Some(RateView::Live { .. })
        ));
    }

    /// One reply that looked hidden doesn't hide the model for good: the next one that checks
    /// out shows it again.
    #[test]
    fn the_latest_judged_reply_decides() {
        let mut meter = RateMeter::default();
        let mut calibration = RateCalibration::default();
        let at = Instant::now();
        let at = reply(&mut meter, &mut calibration, at, "m", "off", 30, 900, false);
        assert!(!calibration.model("p/m").unwrap().is_visible(false));
        reply(&mut meter, &mut calibration, at, "m", "off", 30, 90, false);
        assert!(calibration.model("p/m").unwrap().is_visible(false));
    }

    /// A server that counts nothing (output 0) is estimated, live and at the end, and says so.
    #[test]
    fn a_server_that_counts_nothing_is_estimated() {
        let mut meter = RateMeter::default();
        let mut calibration = RateCalibration::default();
        let at = Instant::now();
        let now = reply(&mut meter, &mut calibration, at, "m", "off", 30, 0, false);
        assert!(calibration.model("p/m").unwrap().uncounted);
        let Some(RateView::Done(done)) = meter.view(now, &calibration) else {
            panic!("no figure at the end");
        };
        assert!(done.estimated);
        assert!(done.tokens > 0);
        let at = now + Duration::from_secs(1);
        meter.apply(&start("m", "off"), at, &mut calibration);
        for step in 1..20 {
            meter.apply(
                &delta("text_delta", "the quick fox ", 0),
                at + Duration::from_millis(100 * step),
                &mut calibration,
            );
        }
        let Some(RateView::Live { accuracy, .. }) =
            meter.view(at + Duration::from_secs(2), &calibration)
        else {
            panic!("no estimate");
        };
        assert!(matches!(accuracy, Accuracy::Estimate { .. }));
        // It counts after all: exact again.
        reply(&mut meter, &mut calibration, at, "m", "off", 30, 90, false);
        assert!(!calibration.model("p/m").unwrap().uncounted);
    }

    #[test]
    fn tools_running_models_and_failed_replies_are_left_out() {
        let mut meter = RateMeter::default();
        let mut calibration = RateCalibration::default();
        let at = Instant::now();
        meter.apply(&start("m", "off"), at, &mut calibration);
        for step in 1..30 {
            meter.apply(
                &delta("text_delta", "the quick fox ", 1),
                at + Duration::from_millis(100 * step),
                &mut calibration,
            );
        }
        let aborted: Event = serde_json::from_value(json!({
            "type": "message_end",
            "message": {"role": "assistant", "content": [], "provider": "p", "model": "m",
                        "stopReason": "aborted", "usage": {"output": 90}}
        }))
        .unwrap();
        assert!(!meter.apply(&aborted, at + Duration::from_secs(3), &mut calibration));
        assert_eq!(calibration, RateCalibration::default());
        assert_eq!(meter.view(at + Duration::from_secs(3), &calibration), None);
    }
}
