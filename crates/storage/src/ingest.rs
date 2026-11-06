//! Turning a classified gap into stored spans.

use crate::*;
use gap_classifier::{Assumptions, GapKind, GapReport};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IngestResult {
    pub spans: usize,
    /// The same gap was stored before; nothing was added.
    pub already_ingested: bool,
}

fn kind_str(k: GapKind) -> &'static str {
    match k {
        GapKind::OnUntracked => "on_untracked",
        GapKind::Sleep => "sleep",
        GapKind::Hibernate => "hibernate",
        GapKind::Shutdown => "shutdown",
        GapKind::Restart => "restart",
        GapKind::Crash => "crash",
        GapKind::Unknown => "unknown",
    }
}

/// Round to whole seconds consistently so neighbouring segments still tile exactly.
fn whole(t: DateTime<Utc>) -> DateTime<Utc> {
    ts_to_dt(t.timestamp_millis().div_euclid(1000) + i64::from(t.timestamp_millis().rem_euclid(1000) >= 500))
}

impl Storage {
    /// Estimate and store every segment of a gap report.
    ///
    /// `on_ac_default`: whether to assume mains power for segments with no AC/battery evidence
    /// (use the last heartbeat's source). Sleep segments carry their own `PowerStateAc`.
    pub fn ingest_gap(
        &mut self,
        report: &GapReport,
        estimator: &Estimator,
        assumptions: &Assumptions,
        on_ac_default: bool,
        tariffs: &TariffSet,
    ) -> Result<IngestResult> {
        let tx = self.conn.transaction()?;
        let already: i64 = tx.query_row(
            "SELECT COUNT(*) FROM gap WHERE start_ts=?1 AND end_ts=?2",
            params![report.start.timestamp(), report.end.timestamp()],
            |r| r.get(0),
        )?;
        if already > 0 {
            return Ok(IngestResult { spans: 0, already_ingested: true });
        }

        let mut spans = 0;
        for seg in &report.segments {
            let (start, end) = (whole(seg.start), whole(seg.end));
            tx.execute(
                "INSERT OR IGNORE INTO gap(start_ts,end_ts,kind,confidence,evidence_json,note) VALUES(?1,?2,?3,?4,?5,?6)",
                params![
                    start.timestamp(),
                    end.timestamp(),
                    kind_str(seg.kind),
                    format!("{:?}", seg.confidence).to_lowercase(),
                    serde_json::to_string(&seg.evidence)?,
                    seg.note
                ],
            )?;
            if end <= start {
                continue;
            }
            let state = seg.kind.power_state(assumptions);
            let mut inputs = Inputs::new(state);
            if state == PowerState::Sleep {
                inputs.sleep_active_permille = seg.sleep.and_then(|d| d.active_permille());
            }
            let est = estimator.estimate(&inputs);
            let on_ac = seg.sleep.and_then(|d| d.on_ac).unwrap_or(on_ac_default);
            // Only mains energy is billed (battery discharge is not; charging afterwards is).
            let billable = on_ac;
            let span = Span { start, end, inputs, watts: est.watts, layer: est.layer, billable, source: SpanSource::Gap };
            insert_span(&tx, &span)?;
            price_into(&tx, &span.sample(), tariffs)?;
            spans += 1;
        }
        // Make the whole-gap marker unique even when it had no segments.
        tx.execute(
            "INSERT OR IGNORE INTO gap(start_ts,end_ts,kind,confidence,evidence_json,note) VALUES(?1,?2,'gap_marker','high','[]','ingested')",
            params![report.start.timestamp(), report.end.timestamp()],
        )?;
        tx.commit()?;
        Ok(IngestResult { spans, already_ingested: false })
    }

    /// Number of classified gap segments stored (excluding the per-gap markers).
    pub fn count_gaps(&self) -> Result<i64> {
        Ok(self.conn.query_row("SELECT COUNT(*) FROM gap WHERE kind<>'gap_marker'", [], |r| r.get(0))?)
    }
}
