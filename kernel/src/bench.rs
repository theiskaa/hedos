//! What a model does on this machine: one run's measurement, the summary over
//! several, and the order and bar widths a table of them draws with.
//!
//! Nothing here runs a model or reads a clock. The driver takes the marks and
//! hands them over; this decides what the figures mean, which of them the
//! backend actually reported, and how they rank.

use crate::capabilities::GenerationStats;

/// Characters per token, for a reply whose runtime counted none. Coarse on
/// purpose: a figure it produces is marked estimated wherever it is shown.
const CHARS_PER_TOKEN: f64 = 4.0;

/// Where a run's timing came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimingSource {
    /// The backend reported the phase itself.
    Backend,
    /// Measured here, from the stream's own arrival times.
    WallClock,
}

impl TimingSource {
    /// The stable string form: `backend` or `wall_clock`.
    pub fn as_str(self) -> &'static str {
        match self {
            TimingSource::Backend => "backend",
            TimingSource::WallClock => "wall_clock",
        }
    }
}

/// The marks the driver takes around one run, in milliseconds: when the first
/// token arrived and when the stream ended, both measured from the request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WallClock {
    /// Request to first token.
    pub ttft_ms: i64,
    /// Request to the end of the stream.
    pub total_ms: i64,
}

/// One run of one model.
#[derive(Debug, Clone, PartialEq)]
pub struct Sample {
    /// Tokens generated, reported or estimated.
    pub completion_tokens: i64,
    /// Tokens of prompt, when the backend counted them.
    pub prompt_tokens: Option<i64>,
    /// Time to the first token. Always wall clock: it is what a caller waits.
    pub ttft_ms: i64,
    /// Time spent generating, the prompt excluded.
    pub decode_ms: i64,
    /// Time spent on the prompt, when the backend reports the phases apart.
    pub prompt_ms: Option<i64>,
    /// Whether the token count was counted from the text rather than reported.
    pub estimated_tokens: bool,
    /// Where `decode_ms` came from.
    pub source: TimingSource,
}

impl Sample {
    /// Fold one run's reported stats and wall-clock marks into a sample.
    /// `generated` is the text the run produced, which stands in for a token
    /// count the runtime did not report.
    pub fn new(stats: Option<&GenerationStats>, generated: &str, wall: WallClock) -> Self {
        let reported = stats
            .and_then(|stats| stats.completion_tokens)
            .filter(|tokens| *tokens > 0);
        let estimated_tokens =
            reported.is_none() || stats.is_some_and(|stats| stats.token_counts_estimated);
        let completion_tokens = reported.unwrap_or_else(|| estimate_tokens(generated));

        // The decode phase is the backend's own figure where it reports one,
        // else what is left of the stream after the first token: a rate over
        // the whole run would charge the prompt to the generation.
        let (decode_ms, source) = match stats.and_then(|stats| stats.eval_ms).filter(|ms| *ms > 0) {
            Some(ms) => (ms, TimingSource::Backend),
            None => (
                (wall.total_ms - wall.ttft_ms).max(0),
                TimingSource::WallClock,
            ),
        };

        Self {
            completion_tokens,
            prompt_tokens: stats
                .and_then(|stats| stats.prompt_tokens)
                .filter(|tokens| *tokens > 0),
            ttft_ms: wall.ttft_ms.max(0),
            decode_ms,
            prompt_ms: stats.and_then(|stats| stats.prompt_ms).filter(|ms| *ms > 0),
            estimated_tokens,
            source,
        }
    }

    /// Tokens generated a second, when the run took any measurable time.
    ///
    /// A wall-clock run is timed from the first token to the last, so the
    /// tokens that span is worth are all of them but the first; a backend
    /// times its own decode against every token it produced. Counting each the
    /// way it was measured is what lets the two sit in one ranked column.
    pub fn tokens_per_second(&self) -> Option<f64> {
        let counted = match self.source {
            TimingSource::Backend => self.completion_tokens,
            TimingSource::WallClock => self.completion_tokens - 1,
        };
        (self.decode_ms > 0 && counted > 0).then(|| counted as f64 * 1000.0 / self.decode_ms as f64)
    }

    /// Prompt tokens processed a second, only where the backend timed the
    /// prompt phase; there is nothing to divide otherwise.
    pub fn prompt_tokens_per_second(&self) -> Option<f64> {
        let tokens = self.prompt_tokens?;
        let ms = self.prompt_ms?;
        (ms > 0 && tokens > 0).then(|| tokens as f64 * 1000.0 / ms as f64)
    }
}

/// A figure over several runs: the middle one, and the two ends it varied
/// between.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Measure {
    /// The middle run, which is the figure a row is read by.
    pub median: f64,
    /// The slowest of the runs.
    pub min: f64,
    /// The fastest of them.
    pub max: f64,
}

impl Measure {
    /// The measure over `values`, or `None` when there are none. An even count
    /// takes the mean of the middle two, so three runs and four runs are read
    /// the same way.
    pub fn of(values: &[f64]) -> Option<Self> {
        if values.is_empty() {
            return None;
        }
        let mut sorted = values.to_vec();
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let middle = sorted.len() / 2;
        let median = if sorted.len().is_multiple_of(2) {
            (sorted[middle - 1] + sorted[middle]) / 2.0
        } else {
            sorted[middle]
        };
        Some(Self {
            median,
            min: sorted[0],
            max: sorted[sorted.len() - 1],
        })
    }

    /// Whether the ends differ enough to be worth printing beside the median.
    pub fn has_spread(&self) -> bool {
        (self.max - self.min) >= 0.05
    }
}

/// What the cold run measured, or why it did not.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ColdStart {
    /// The model was cleared from memory first, and this is the first token
    /// that followed: what a caller waits for on a cold machine.
    Measured(i64),
    /// It could not be cleared, so nothing here was cold. Carries who held it.
    Held(String),
    /// Residency was left alone, so no cold run was made.
    NotMeasured,
}

impl ColdStart {
    /// The measured milliseconds, when there are any.
    pub fn millis(&self) -> Option<i64> {
        match self {
            ColdStart::Measured(ms) => Some(*ms),
            _ => None,
        }
    }
}

/// What a finished model's runs came to.
#[derive(Debug, Clone, PartialEq)]
pub struct Figures {
    /// The cold run, or why there was none.
    pub cold_start: ColdStart,
    /// Tokens a second over the warm runs.
    pub tokens_per_second: Option<Measure>,
    /// Time to first token over the warm runs.
    pub ttft_ms: Option<Measure>,
    /// Prompt tokens a second, where the backend timed the prompt.
    pub prompt_tokens_per_second: Option<Measure>,
    /// Whether any warm run's token count was estimated.
    pub estimated_tokens: bool,
    /// Where the decode timing came from; wall clock if any run fell back.
    pub source: TimingSource,
    /// The warm runs themselves, for the detail and the JSON.
    pub runs: Vec<Sample>,
}

impl Figures {
    /// Summarize the warm `runs` of one model, with `cold` saying what the run
    /// before them measured, or why there was not one.
    pub fn summarize(cold: ColdStart, runs: Vec<Sample>) -> Self {
        let rates: Vec<f64> = runs.iter().filter_map(Sample::tokens_per_second).collect();
        let ttfts: Vec<f64> = runs.iter().map(|run| run.ttft_ms as f64).collect();
        let prompt_rates: Vec<f64> = runs
            .iter()
            .filter_map(Sample::prompt_tokens_per_second)
            .collect();
        Self {
            cold_start: cold,
            tokens_per_second: Measure::of(&rates),
            ttft_ms: Measure::of(&ttfts),
            prompt_tokens_per_second: Measure::of(&prompt_rates),
            estimated_tokens: runs.iter().any(|run| run.estimated_tokens),
            // One wall-clock run makes the row's rate a wall-clock reading;
            // saying "backend" would overclaim for the whole column, and so
            // would saying it over no runs at all.
            source: if !runs.is_empty()
                && runs.iter().all(|run| run.source == TimingSource::Backend)
            {
                TimingSource::Backend
            } else {
                TimingSource::WallClock
            },
            runs,
        }
    }

    /// The rate the row is ranked and drawn by.
    pub fn rate(&self) -> Option<f64> {
        self.tokens_per_second.map(|measure| measure.median)
    }
}

/// Which phase of a model's turn is under way.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    /// The first run after eviction, whose first token is the cold start.
    ColdStart,
    /// One of the warm runs, numbered from one.
    Warm { run: usize, of: usize },
}

/// Where a model is in the bench.
#[derive(Debug, Clone, PartialEq)]
pub enum Status {
    /// Queued, not started.
    Waiting,
    /// Running, with the tokens it has produced in this run so far.
    Running { phase: Phase, tokens: i64 },
    /// Measured.
    Done(Box<Figures>),
    /// Tried and failed, with the reason.
    Failed(String),
    /// Never tried, with the reason.
    Skipped(String),
    /// The bench was stopped before this model's turn came, or during it.
    Stopped,
}

impl Status {
    /// The figures, for a model that finished.
    pub fn figures(&self) -> Option<&Figures> {
        match self {
            Status::Done(figures) => Some(figures),
            _ => None,
        }
    }

    /// The rate a finished model measured.
    pub fn rate(&self) -> Option<f64> {
        self.figures().and_then(Figures::rate)
    }
}

/// One row of the bench: the model, what runs it, and how it is doing.
#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    /// The record id, for addressing the model again.
    pub id: String,
    /// The model's display name.
    pub name: String,
    /// The runtime that serves it, when it resolves to one.
    pub runtime: Option<String>,
    /// The quantization its weights carry, when it is known.
    pub quantization: Option<String>,
    /// Where it is in the bench.
    pub status: Status,
}

impl Row {
    /// A queued row for a model that will be benched.
    pub fn waiting(
        id: impl Into<String>,
        name: impl Into<String>,
        runtime: Option<String>,
        quantization: Option<String>,
    ) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            runtime,
            quantization,
            status: Status::Waiting,
        }
    }

    /// A row for a model the bench will not try, and why.
    pub fn skipped(
        id: impl Into<String>,
        name: impl Into<String>,
        runtime: Option<String>,
        quantization: Option<String>,
        reason: impl Into<String>,
    ) -> Self {
        let mut row = Self::waiting(id, name, runtime, quantization);
        row.status = Status::Skipped(reason.into());
        row
    }
}

/// The fastest rate any row measured, which the bars are drawn against.
pub fn fastest(rows: &[Row]) -> Option<f64> {
    rows.iter()
        .filter_map(|row| row.status.rate())
        .fold(None, |best: Option<f64>, rate| {
            Some(best.map_or(rate, |best| best.max(rate)))
        })
}

/// `rows` in the order the finished table draws them: measured rows fastest
/// first, then everything that produced no figure, in the order it came.
pub fn rank(rows: &[Row]) -> Vec<&Row> {
    let mut ordered: Vec<&Row> = rows.iter().collect();
    ordered.sort_by(|a, b| match (a.status.rate(), b.status.rate()) {
        (Some(left), Some(right)) => right
            .partial_cmp(&left)
            .unwrap_or(std::cmp::Ordering::Equal),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => std::cmp::Ordering::Equal,
    });
    ordered
}

/// How many of a bar's `width` cells a rate of `rate` lights, against the
/// `fastest` row. A rate that measured anything at all keeps one cell, so a
/// slow model reads as slow rather than as nothing measured.
pub fn filled_cells(rate: f64, fastest: f64, width: usize) -> usize {
    if width == 0 || rate <= 0.0 || fastest <= 0.0 {
        return 0;
    }
    let cells = (rate / fastest * width as f64).round() as usize;
    cells.clamp(1, width)
}

/// Tokens a reply of `text` is worth, when nothing counted them: its characters
/// over [`CHARS_PER_TOKEN`], and never zero for text that exists.
pub fn estimate_tokens(text: &str) -> i64 {
    let characters = text.chars().count();
    if characters == 0 {
        return 0;
    }
    ((characters as f64 / CHARS_PER_TOKEN).round() as i64).max(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stats(completion: Option<i64>, eval_ms: Option<i64>) -> GenerationStats {
        GenerationStats {
            completion_tokens: completion,
            eval_ms,
            ..GenerationStats::default()
        }
    }

    fn wall(ttft_ms: i64, total_ms: i64) -> WallClock {
        WallClock { ttft_ms, total_ms }
    }

    fn done(id: &str, rate_tokens: i64, decode_ms: i64) -> Row {
        let sample = Sample::new(
            Some(&stats(Some(rate_tokens), Some(decode_ms))),
            "",
            wall(100, 100 + decode_ms),
        );
        let mut row = Row::waiting(id, id, None, None);
        row.status = Status::Done(Box::new(Figures::summarize(
            ColdStart::NotMeasured,
            vec![sample],
        )));
        row
    }

    #[test]
    fn a_backend_that_times_the_decode_is_preferred_to_the_wall_clock() {
        let sample = Sample::new(Some(&stats(Some(64), Some(1000))), "", wall(200, 1600));
        assert_eq!(sample.source, TimingSource::Backend);
        assert_eq!(sample.decode_ms, 1000);
        assert_eq!(sample.tokens_per_second(), Some(64.0));
        // The wall clock still carries the wait, which no backend reports.
        assert_eq!(sample.ttft_ms, 200);
    }

    #[test]
    fn without_a_backend_figure_the_decode_is_the_stream_after_the_first_token() {
        let sample = Sample::new(Some(&stats(Some(31), None)), "", wall(500, 2000));
        assert_eq!(sample.source, TimingSource::WallClock);
        assert_eq!(sample.decode_ms, 1500);
        // The span begins at the first token, so it is worth the other thirty.
        assert_eq!(sample.tokens_per_second(), Some(20.0));
    }

    #[test]
    fn a_single_token_reply_measured_here_has_no_rate_to_give() {
        let sample = Sample::new(Some(&stats(Some(1), None)), "", wall(500, 2000));
        assert_eq!(sample.tokens_per_second(), None);
    }

    #[test]
    fn no_runs_at_all_never_claims_the_runtimes_own_timing() {
        let figures = Figures::summarize(ColdStart::NotMeasured, Vec::new());
        assert_eq!(figures.source, TimingSource::WallClock);
        assert!(figures.rate().is_none());
    }

    #[test]
    fn a_reply_nothing_counted_is_estimated_from_its_text() {
        let sample = Sample::new(None, "12345678", wall(10, 1010));
        assert_eq!(sample.completion_tokens, 2);
        assert!(sample.estimated_tokens);
    }

    #[test]
    fn a_backend_that_admits_its_counts_are_estimates_is_believed_and_marked() {
        let mut stats = stats(Some(40), Some(1000));
        stats.token_counts_estimated = true;
        let sample = Sample::new(Some(&stats), "", wall(10, 1010));
        assert_eq!(
            sample.completion_tokens, 40,
            "the count is still the one reported"
        );
        assert!(sample.estimated_tokens, "and it is marked as an estimate");
    }

    #[test]
    fn estimating_counts_characters_rather_than_bytes() {
        // Nine Georgian letters are twenty-seven bytes; counting bytes would
        // call this seven tokens instead of two.
        assert_eq!(estimate_tokens("გამარჯობა"), 2);
        assert_eq!(estimate_tokens(""), 0);
        assert_eq!(
            estimate_tokens("a"),
            1,
            "text that exists is never zero tokens"
        );
    }

    #[test]
    fn a_prompt_rate_needs_both_a_count_and_a_timed_prompt_phase() {
        let mut stats = stats(Some(10), Some(1000));
        stats.prompt_tokens = Some(300);
        let untimed = Sample::new(Some(&stats), "", wall(50, 1050));
        assert_eq!(untimed.prompt_tokens_per_second(), None);
        stats.prompt_ms = Some(150);
        let timed = Sample::new(Some(&stats), "", wall(50, 1050));
        assert_eq!(timed.prompt_tokens_per_second(), Some(2000.0));
    }

    #[test]
    fn a_measure_takes_the_middle_of_an_odd_count_and_the_mean_of_an_even_one() {
        let odd = Measure::of(&[10.0, 30.0, 20.0]).expect("three values");
        assert_eq!((odd.median, odd.min, odd.max), (20.0, 10.0, 30.0));
        let even = Measure::of(&[10.0, 20.0, 30.0, 40.0]).expect("four values");
        assert_eq!(even.median, 25.0);
        assert!(Measure::of(&[]).is_none());
    }

    #[test]
    fn one_wall_clock_run_makes_the_whole_row_wall_clock() {
        let backend = Sample::new(Some(&stats(Some(20), Some(1000))), "", wall(10, 1010));
        let measured_here = Sample::new(Some(&stats(Some(20), None)), "", wall(10, 1010));
        let figures =
            Figures::summarize(ColdStart::NotMeasured, vec![backend.clone(), measured_here]);
        assert_eq!(figures.source, TimingSource::WallClock);
        assert_eq!(
            Figures::summarize(ColdStart::NotMeasured, vec![backend]).source,
            TimingSource::Backend
        );
    }

    #[test]
    fn the_cold_start_is_the_first_token_of_the_run_before_the_warm_ones() {
        let cold = Sample::new(Some(&stats(Some(5), Some(500))), "", wall(3000, 3500));
        let warm = Sample::new(Some(&stats(Some(5), Some(500))), "", wall(200, 700));
        let figures = Figures::summarize(ColdStart::Measured(cold.ttft_ms), vec![warm]);
        assert_eq!(figures.cold_start.millis(), Some(3000));
        assert_eq!(
            figures.ttft_ms.expect("a warm ttft").median,
            200.0,
            "the cold run is not one of the warm figures"
        );
    }

    #[test]
    fn ranking_puts_the_fastest_first_and_the_figureless_last() {
        let mut failed = Row::waiting("f", "f", None, None);
        failed.status = Status::Failed("llama-server is not on the PATH".to_owned());
        let rows = vec![
            done("slow", 10, 1000),
            failed,
            done("fast", 60, 1000),
            Row::skipped("big", "big", None, None, "too big"),
        ];
        let order: Vec<&str> = rank(&rows).iter().map(|row| row.id.as_str()).collect();
        assert_eq!(order, vec!["fast", "slow", "f", "big"]);
        assert_eq!(fastest(&rows), Some(60.0));
    }

    #[test]
    fn the_fastest_row_fills_its_bar_and_a_slow_one_keeps_a_cell() {
        assert_eq!(filled_cells(60.0, 60.0, 20), 20);
        assert_eq!(filled_cells(30.0, 60.0, 20), 10);
        // Half a cell's worth still shows: a measured row is never blank.
        assert_eq!(filled_cells(0.4, 60.0, 20), 1);
        assert_eq!(filled_cells(0.0, 60.0, 20), 0, "and nothing measured is");
        assert_eq!(filled_cells(10.0, 0.0, 20), 0);
        assert_eq!(filled_cells(10.0, 60.0, 0), 0);
    }

    #[test]
    fn a_spread_is_only_reported_when_the_ends_actually_differ() {
        assert!(
            !Measure::of(&[40.0, 40.02])
                .expect("two values")
                .has_spread()
        );
        assert!(Measure::of(&[40.0, 44.0]).expect("two values").has_spread());
    }
}
