//! Minimal in-process metrics, exposed as Prometheus text on `GET /metrics`
//! (see `http.rs`).
//!
//! No `metrics` / `metrics-exporter-prometheus` crate: at this project's
//! scale (a personal/small-group bot) the label sets are small and fixed
//! (ask outcome, ingestion job kind), so a handful of named atomics is
//! simpler than pulling in a full metrics framework and everything it
//! brings with it. Upgrade to those crates if this ever needs real
//! histograms/percentiles or a dynamic label set — this only tracks
//! sum+count ("what's the average") not full distributions ("what's the p99").

use std::fmt::Write as _;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

#[derive(Default)]
struct Counter(AtomicU64);

impl Counter {
    fn inc(&self) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }
    fn get(&self) -> u64 {
        self.0.load(Ordering::Relaxed)
    }
}

/// Sum + count of a duration series — enough for an average, not a
/// histogram. Good enough to notice "slower than usual"; not meant to
/// answer "what's the p99".
#[derive(Default)]
struct DurationTotals {
    millis_sum: AtomicU64,
    count: AtomicU64,
}

impl DurationTotals {
    fn record(&self, elapsed: Duration) {
        self.millis_sum
            .fetch_add(elapsed.as_millis() as u64, Ordering::Relaxed);
        self.count.fetch_add(1, Ordering::Relaxed);
    }
    fn seconds_sum(&self) -> f64 {
        self.millis_sum.load(Ordering::Relaxed) as f64 / 1000.0
    }
    fn count(&self) -> u64 {
        self.count.load(Ordering::Relaxed)
    }
}

#[derive(Default)]
struct KindMetrics {
    done: Counter,
    failed: Counter,
    duration: DurationTotals,
}

/// Outcome of an `/ask` or `@mention` query — `arnheid_ask_total{outcome=...}`.
/// `Answered` vs `NoResults` is a heuristic (cited sources present or not);
/// good enough for a health signal, not meant to be exact.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AskOutcome {
    Answered,
    NoResults,
    Error,
}

/// The four `ingestion_jobs.kind` values. Anything else (there shouldn't be
/// anything else) folds into `Note` rather than panicking or silently
/// dropping the sample.
fn kind_index(kind: &str) -> usize {
    match kind {
        "link" => 0,
        "voice" => 2,
        "image" => 3,
        _ => 1, // "note" and any future/unknown kind
    }
}
const KIND_NAMES: [&str; 4] = ["link", "note", "voice", "image"];

#[derive(Default)]
pub struct Metrics {
    ask_answered: Counter,
    ask_no_results: Counter,
    ask_error: Counter,
    ask_latency: DurationTotals,

    ingestion: [KindMetrics; 4],

    telegram_send_failures: Counter,
    relevance_dm_sent: Counter,
    agentic_fallback: Counter,
}

impl Metrics {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn record_ask(&self, outcome: AskOutcome, elapsed: Duration) {
        match outcome {
            AskOutcome::Answered => self.ask_answered.inc(),
            AskOutcome::NoResults => self.ask_no_results.inc(),
            AskOutcome::Error => self.ask_error.inc(),
        }
        self.ask_latency.record(elapsed);
    }

    pub fn record_ingestion(&self, kind: &str, ok: bool, elapsed: Duration) {
        let k = &self.ingestion[kind_index(kind)];
        if ok {
            k.done.inc();
        } else {
            k.failed.inc();
        }
        k.duration.record(elapsed);
    }

    pub fn inc_telegram_send_failure(&self) {
        self.telegram_send_failures.inc();
    }

    pub fn inc_relevance_dm_sent(&self) {
        self.relevance_dm_sent.inc();
    }

    /// Agentic mode (`RAG_MODE=agentic`) was tried and fell back to the
    /// fixed pipeline. The fallback keeps `/ask` working either way, but a
    /// climbing rate here means the experimental path is unreliable and
    /// worth investigating (or turning back off).
    pub fn inc_agentic_fallback(&self) {
        self.agentic_fallback.inc();
    }

    /// Render as Prometheus text exposition format. `queue_depth` is passed
    /// in (rather than cached on `self`) so it's always the live count at
    /// scrape time, not whatever it was at the last cron tick — the query
    /// behind it is cheap enough to run per scrape.
    pub fn render(&self, queue_depth: Option<i64>) -> String {
        let mut out = String::new();

        writeln!(
            out,
            "# HELP arnheid_ask_total /ask and @mention queries by outcome."
        )
        .ok();
        writeln!(out, "# TYPE arnheid_ask_total counter").ok();
        writeln!(
            out,
            "arnheid_ask_total{{outcome=\"answered\"}} {}",
            self.ask_answered.get()
        )
        .ok();
        writeln!(
            out,
            "arnheid_ask_total{{outcome=\"no_results\"}} {}",
            self.ask_no_results.get()
        )
        .ok();
        writeln!(
            out,
            "arnheid_ask_total{{outcome=\"error\"}} {}",
            self.ask_error.get()
        )
        .ok();

        writeln!(
            out,
            "# HELP arnheid_ask_latency_seconds Time to answer an /ask or @mention query."
        )
        .ok();
        writeln!(out, "# TYPE arnheid_ask_latency_seconds summary").ok();
        writeln!(
            out,
            "arnheid_ask_latency_seconds_sum {}",
            self.ask_latency.seconds_sum()
        )
        .ok();
        writeln!(
            out,
            "arnheid_ask_latency_seconds_count {}",
            self.ask_latency.count()
        )
        .ok();

        writeln!(
            out,
            "# HELP arnheid_ingestion_jobs_total Ingestion jobs processed, by kind and outcome."
        )
        .ok();
        writeln!(out, "# TYPE arnheid_ingestion_jobs_total counter").ok();
        for (kind, k) in KIND_NAMES.iter().zip(self.ingestion.iter()) {
            writeln!(
                out,
                "arnheid_ingestion_jobs_total{{kind=\"{kind}\",outcome=\"done\"}} {}",
                k.done.get()
            )
            .ok();
            writeln!(
                out,
                "arnheid_ingestion_jobs_total{{kind=\"{kind}\",outcome=\"failed\"}} {}",
                k.failed.get()
            )
            .ok();
        }

        writeln!(
            out,
            "# HELP arnheid_ingestion_job_duration_seconds Time to process one ingestion job."
        )
        .ok();
        writeln!(out, "# TYPE arnheid_ingestion_job_duration_seconds summary").ok();
        for (kind, k) in KIND_NAMES.iter().zip(self.ingestion.iter()) {
            writeln!(
                out,
                "arnheid_ingestion_job_duration_seconds_sum{{kind=\"{kind}\"}} {}",
                k.duration.seconds_sum()
            )
            .ok();
            writeln!(
                out,
                "arnheid_ingestion_job_duration_seconds_count{{kind=\"{kind}\"}} {}",
                k.duration.count()
            )
            .ok();
        }

        if let Some(depth) = queue_depth {
            writeln!(
                out,
                "# HELP arnheid_ingestion_queue_depth Ingestion jobs pending or claimed right now."
            )
            .ok();
            writeln!(out, "# TYPE arnheid_ingestion_queue_depth gauge").ok();
            writeln!(out, "arnheid_ingestion_queue_depth {depth}").ok();
        }

        writeln!(
            out,
            "# HELP arnheid_telegram_send_failures_total Telegram sends that exhausted retries."
        )
        .ok();
        writeln!(out, "# TYPE arnheid_telegram_send_failures_total counter").ok();
        writeln!(
            out,
            "arnheid_telegram_send_failures_total {}",
            self.telegram_send_failures.get()
        )
        .ok();

        writeln!(
            out,
            "# HELP arnheid_relevance_dm_sent_total Relevance-interrupt DMs actually delivered."
        )
        .ok();
        writeln!(out, "# TYPE arnheid_relevance_dm_sent_total counter").ok();
        writeln!(
            out,
            "arnheid_relevance_dm_sent_total {}",
            self.relevance_dm_sent.get()
        )
        .ok();

        writeln!(
            out,
            "# HELP arnheid_agentic_fallback_total Agentic /ask attempts that fell back to the fixed pipeline."
        )
        .ok();
        writeln!(out, "# TYPE arnheid_agentic_fallback_total counter").ok();
        writeln!(
            out,
            "arnheid_agentic_fallback_total {}",
            self.agentic_fallback.get()
        )
        .ok();

        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_ask_outcomes_independently() {
        let m = Metrics::new();
        m.record_ask(AskOutcome::Answered, Duration::from_millis(100));
        m.record_ask(AskOutcome::Answered, Duration::from_millis(300));
        m.record_ask(AskOutcome::NoResults, Duration::from_millis(50));
        m.record_ask(AskOutcome::Error, Duration::from_millis(10));

        let rendered = m.render(None);
        assert!(rendered.contains("arnheid_ask_total{outcome=\"answered\"} 2"));
        assert!(rendered.contains("arnheid_ask_total{outcome=\"no_results\"} 1"));
        assert!(rendered.contains("arnheid_ask_total{outcome=\"error\"} 1"));
        assert!(rendered.contains("arnheid_ask_latency_seconds_count 4"));
        assert!(rendered.contains("arnheid_ask_latency_seconds_sum 0.46"));
    }

    #[test]
    fn ingestion_kinds_tracked_separately() {
        let m = Metrics::new();
        m.record_ingestion("link", true, Duration::from_secs(2));
        m.record_ingestion("link", false, Duration::from_secs(1));
        m.record_ingestion("voice", true, Duration::from_millis(500));

        let rendered = m.render(None);
        assert!(rendered.contains("arnheid_ingestion_jobs_total{kind=\"link\",outcome=\"done\"} 1"));
        assert!(rendered.contains("arnheid_ingestion_jobs_total{kind=\"link\",outcome=\"failed\"} 1"));
        assert!(rendered.contains("arnheid_ingestion_jobs_total{kind=\"voice\",outcome=\"done\"} 1"));
        assert!(rendered.contains("arnheid_ingestion_jobs_total{kind=\"image\",outcome=\"done\"} 0"));
        assert!(rendered.contains("arnheid_ingestion_job_duration_seconds_sum{kind=\"link\"} 3"));
    }

    #[test]
    fn unknown_kind_folds_into_note_without_panicking() {
        let m = Metrics::new();
        m.record_ingestion("mystery", true, Duration::from_millis(1));
        assert!(m
            .render(None)
            .contains("arnheid_ingestion_jobs_total{kind=\"note\",outcome=\"done\"} 1"));
    }

    #[test]
    fn queue_depth_omitted_when_unavailable() {
        let m = Metrics::new();
        assert!(!m.render(None).contains("arnheid_ingestion_queue_depth"));
        assert!(m.render(Some(7)).contains("arnheid_ingestion_queue_depth 7"));
    }

    #[test]
    fn counters_start_at_zero() {
        let m = Metrics::new();
        let rendered = m.render(None);
        assert!(rendered.contains("arnheid_telegram_send_failures_total 0"));
        assert!(rendered.contains("arnheid_relevance_dm_sent_total 0"));
        assert!(rendered.contains("arnheid_agentic_fallback_total 0"));
    }

    #[test]
    fn agentic_fallback_increments() {
        let m = Metrics::new();
        m.inc_agentic_fallback();
        m.inc_agentic_fallback();
        assert!(m.render(None).contains("arnheid_agentic_fallback_total 2"));
    }
}
