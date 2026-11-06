//! SQLite storage. Source of truth is `usage_span` (UTC spans with the estimator inputs and
//! the resulting watts); `usage_cost` (1-minute priced intervals) is derived and rebuildable;
//! `agg_hourly` / `agg_daily` hold compacted history. See docs/DECISIONS.md D14-D16.
//!
//! * All timestamps are UTC unix seconds (power events: milliseconds).
//! * Money/energy are integer nano-units (see cost-engine).
//! * Compaction turns whole LOCAL days older than a cutoff into hourly + daily aggregates and
//!   deletes their minute rows and spans; after that those days can no longer be re-derived.

mod compaction;
mod ingest;
mod queries;
mod queries_layers;
mod schema;

pub use compaction::CompactionReport;
pub use ingest::IngestResult;
pub use queries::{Bucket, BucketRow, CostRow};

use chrono::{DateTime, NaiveDate, TimeZone, Utc};
use chrono_tz::Tz;
use cost_engine::{Summary, Totals, TariffSet, UsageSample};
use estimator::{Estimator, Inputs, Layer};
use gap_classifier::heartbeat::{Heartbeat, PowerSource};
use gap_classifier::PowerEvent;
use pc_core::PowerState;
use rusqlite::{params, Connection, OptionalExtension};
use rust_decimal::prelude::*;
use rust_decimal::Decimal;
use serde::{de::DeserializeOwned, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

#[derive(Debug, thiserror::Error)]
pub enum StorageError {
    #[error("database error: {0}")]
    Db(#[from] rusqlite::Error),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("cost error: {0}")]
    Cost(#[from] cost_engine::CostError),
    #[error("invalid stored data: {0}")]
    Corrupt(String),
    #[error("database was created by a newer version (schema {0})")]
    TooNew(i64),
}

pub type Result<T> = std::result::Result<T, StorageError>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpanSource {
    Live,
    Gap,
    Simulated,
}

impl SpanSource {
    fn as_str(self) -> &'static str {
        match self {
            SpanSource::Live => "live",
            SpanSource::Gap => "gap",
            SpanSource::Simulated => "sim",
        }
    }
    fn parse(s: &str) -> SpanSource {
        match s {
            "gap" => SpanSource::Gap,
            "sim" => SpanSource::Simulated,
            _ => SpanSource::Live,
        }
    }
}

/// A stretch of constant estimated power, plus what it was estimated from.
#[derive(Debug, Clone, PartialEq)]
pub struct Span {
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
    pub inputs: Inputs,
    pub watts: Decimal,
    pub layer: Layer,
    /// False when running on battery.
    pub billable: bool,
    pub source: SpanSource,
}

impl Span {
    pub fn sample(&self) -> UsageSample {
        UsageSample { start: self.start, end: self.end, state: self.inputs.state, watts: self.watts, billable: self.billable }
    }
}

pub(crate) fn state_str(s: PowerState) -> &'static str {
    match s {
        PowerState::Active => "active",
        PowerState::Idle => "idle",
        PowerState::Sleep => "sleep",
        PowerState::Hibernate => "hibernate",
        PowerState::Off => "off",
    }
}

pub(crate) fn parse_state(s: &str) -> Result<PowerState> {
    Ok(match s {
        "active" => PowerState::Active,
        "idle" => PowerState::Idle,
        "sleep" => PowerState::Sleep,
        "hibernate" => PowerState::Hibernate,
        "off" => PowerState::Off,
        other => return Err(StorageError::Corrupt(format!("unknown state '{other}'"))),
    })
}

fn layer_str(l: Layer) -> &'static str {
    match l {
        Layer::Sensors => "sensors",
        Layer::Calibrated => "calibrated",
        Layer::Default => "default",
    }
}

fn parse_layer(s: &str) -> Layer {
    match s {
        "sensors" => Layer::Sensors,
        "calibrated" => Layer::Calibrated,
        _ => Layer::Default,
    }
}

pub(crate) fn watts_centi(w: Decimal) -> i64 {
    (w * Decimal::from(100)).round().to_i64().unwrap_or(0)
}

pub(crate) fn ts_to_dt(ts: i64) -> DateTime<Utc> {
    Utc.timestamp_opt(ts, 0).single().expect("valid unix timestamp")
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RecomputeReport {
    pub spans: usize,
    pub minutes_deleted: usize,
    /// Part of the requested range was already compacted and could not be recomputed.
    pub clipped_by_compaction: bool,
}

pub struct Storage {
    pub(crate) conn: Connection,
}

impl Storage {
    pub fn open(path: &Path) -> Result<Storage> {
        let conn = Connection::open(path)?;
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL; PRAGMA foreign_keys=ON;")?;
        Self::init(conn)
    }

    pub fn open_in_memory() -> Result<Storage> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> Result<Storage> {
        let mut s = Storage { conn };
        schema::migrate(&mut s.conn)?;
        Ok(s)
    }

    // ------------------------------------------------------------ settings / KV

    pub fn set_setting<T: Serialize>(&self, key: &str, value: &T) -> Result<()> {
        self.conn.execute(
            "INSERT INTO settings(key,value) VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value",
            params![key, serde_json::to_string(value)?],
        )?;
        Ok(())
    }

    pub fn get_setting<T: DeserializeOwned>(&self, key: &str) -> Result<Option<T>> {
        let raw: Option<String> =
            self.conn.query_row("SELECT value FROM settings WHERE key=?1", params![key], |r| r.get(0)).optional()?;
        raw.map(|r| serde_json::from_str(&r)).transpose().map_err(Into::into)
    }

    pub(crate) fn compacted_until(&self) -> Result<i64> {
        Ok(self.get_setting::<i64>("compacted_until_ts")?.unwrap_or(i64::MIN))
    }

    // ------------------------------------------------------------ heartbeat

    pub fn write_heartbeat(&self, hb: &Heartbeat) -> Result<()> {
        self.conn.prepare_cached(
            "INSERT OR REPLACE INTO heartbeat(ts,session_id,state,source) VALUES(?1,?2,?3,?4)",
        )?
        .execute(params![hb.time.timestamp(), hb.session_id, state_str(hb.state), source_str(hb.source)])?;
        Ok(())
    }

    pub fn last_heartbeat(&self) -> Result<Option<Heartbeat>> {
        let row = self
            .conn
            .query_row(
                "SELECT ts,session_id,state,source FROM heartbeat ORDER BY ts DESC LIMIT 1",
                [],
                |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, String>(3)?)),
            )
            .optional()?;
        row.map(|(ts, session_id, state, source)| {
            Ok(Heartbeat { time: ts_to_dt(ts), session_id, state: parse_state(&state)?, source: parse_source(&source) })
        })
        .transpose()
    }

    /// Heartbeats only need to survive until the next start; keep a short tail.
    pub fn prune_heartbeats(&self, before: DateTime<Utc>) -> Result<usize> {
        Ok(self.conn.execute("DELETE FROM heartbeat WHERE ts < ?1", params![before.timestamp()])?)
    }

    // ------------------------------------------------------------ spans + costs

    /// Store a span and its priced 1-minute intervals (one transaction).
    pub fn record_span(&mut self, span: &Span, tariffs: &TariffSet) -> Result<()> {
        let tx = self.conn.transaction()?;
        insert_span(&tx, span)?;
        price_into(&tx, &span.sample(), tariffs)?;
        tx.commit()?;
        Ok(())
    }

    /// Many spans in one transaction (history import, simulator, daily batches).
    pub fn record_spans(&mut self, spans: &[Span], tariffs: &TariffSet) -> Result<()> {
        let tx = self.conn.transaction()?;
        for s in spans {
            insert_span(&tx, s)?;
            price_into(&tx, &s.sample(), tariffs)?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn spans_between(&self, from: DateTime<Utc>, to: DateTime<Utc>) -> Result<Vec<Span>> {
        spans_overlapping(&self.conn, from.timestamp(), to.timestamp())?.into_iter().map(|(_, s)| Ok(s)).collect()
    }

    /// Cost/energy by state and tariff period for `[from, to)`, from minute rows plus compacted
    /// hourly rows. Exact for hour-aligned ranges inside compacted history.
    pub fn summary(&self, from: DateTime<Utc>, to: DateTime<Utc>) -> Result<Summary> {
        let (f, t) = (from.timestamp(), to.timestamp());
        let mut out = Summary::default();
        for sql in [
            "SELECT state,period_id,SUM(seconds),SUM(kwh_nano),SUM(cost_nano) FROM usage_cost WHERE minute_ts>=?1 AND minute_ts<?2 GROUP BY state,period_id",
            "SELECT state,period_id,SUM(seconds),SUM(kwh_nano),SUM(cost_nano) FROM agg_hourly WHERE hour_ts>=?1 AND hour_ts<?2 GROUP BY state,period_id",
        ] {
            let mut st = self.conn.prepare_cached(sql)?;
            let rows = st.query_map(params![f, t], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, i64>(2)?, r.get::<_, i64>(3)?, r.get::<_, i64>(4)?))
            })?;
            for row in rows {
                let (state, period, secs, kwh, cost) = row?;
                let totals = Totals { seconds: secs, kwh_nano: kwh, cost_nano: cost };
                add(&mut out.total, &totals);
                add(out.by_state.entry(parse_state(&state)?).or_default(), &totals);
                add(out.by_period.entry(period).or_default(), &totals);
            }
        }
        Ok(out)
    }

    /// Totals per LOCAL day (for the timeline chart): minute rows grouped in Rust (DST-safe),
    /// plus compacted `agg_daily`.
    pub fn daily_totals(&self, from: DateTime<Utc>, to: DateTime<Utc>, tz: Tz) -> Result<BTreeMap<NaiveDate, Totals>> {
        let mut days: BTreeMap<NaiveDate, Totals> = BTreeMap::new();
        {
            let mut st = self.conn.prepare_cached(
                "SELECT minute_ts,seconds,kwh_nano,cost_nano FROM usage_cost WHERE minute_ts>=?1 AND minute_ts<?2",
            )?;
            let rows = st.query_map(params![from.timestamp(), to.timestamp()], |r| {
                Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?, r.get::<_, i64>(2)?, r.get::<_, i64>(3)?))
            })?;
            for row in rows {
                let (m, secs, kwh, cost) = row?;
                let day = ts_to_dt(m).with_timezone(&tz).date_naive();
                add(days.entry(day).or_default(), &Totals { seconds: secs, kwh_nano: kwh, cost_nano: cost });
            }
        }
        let (d0, d1) = (from.with_timezone(&tz).date_naive(), to.with_timezone(&tz).date_naive());
        let mut st = self.conn.prepare_cached(
            "SELECT day,SUM(seconds),SUM(kwh_nano),SUM(cost_nano) FROM agg_daily WHERE day>=?1 AND day<=?2 GROUP BY day",
        )?;
        let rows = st.query_map(params![d0.to_string(), d1.to_string()], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?, r.get::<_, i64>(2)?, r.get::<_, i64>(3)?))
        })?;
        for row in rows {
            let (day, secs, kwh, cost) = row?;
            let day = NaiveDate::parse_from_str(&day, "%Y-%m-%d").map_err(|e| StorageError::Corrupt(e.to_string()))?;
            add(days.entry(day).or_default(), &Totals { seconds: secs, kwh_nano: kwh, cost_nano: cost });
        }
        Ok(days)
    }

    /// Re-derive priced intervals for `[from, to)` after a tariff correction.
    pub fn recompute_costs(&mut self, from: DateTime<Utc>, to: DateTime<Utc>, tariffs: &TariffSet) -> Result<RecomputeReport> {
        let floor = self.compacted_until()?;
        let a = minute_floor(from.timestamp()).max(floor);
        let b = minute_ceil(to.timestamp());
        let mut report = RecomputeReport { clipped_by_compaction: from.timestamp() < floor, ..Default::default() };
        if b <= a {
            return Ok(report);
        }
        let tx = self.conn.transaction()?;
        report.minutes_deleted = tx.execute("DELETE FROM usage_cost WHERE minute_ts>=?1 AND minute_ts<?2", params![a, b])?;
        for (_, span) in spans_overlapping(&tx, a, b)? {
            let mut sample = span.sample();
            sample.start = sample.start.max(ts_to_dt(a));
            sample.end = sample.end.min(ts_to_dt(b));
            if sample.end > sample.start {
                price_into(&tx, &sample, tariffs)?;
                report.spans += 1;
            }
        }
        tx.commit()?;
        Ok(report)
    }

    /// Re-estimate watts from the stored inputs (e.g. after a calibration change) and reprice.
    pub fn reestimate(
        &mut self,
        from: DateTime<Utc>,
        to: DateTime<Utc>,
        estimator: &Estimator,
        tariffs: &TariffSet,
    ) -> Result<RecomputeReport> {
        let floor = self.compacted_until()?;
        let a = minute_floor(from.timestamp()).max(floor);
        let b = minute_ceil(to.timestamp());
        if b <= a {
            return Ok(RecomputeReport { clipped_by_compaction: from.timestamp() < floor, ..Default::default() });
        }
        let (mut lo, mut hi) = (a, b);
        {
            let tx = self.conn.transaction()?;
            for (id, span) in spans_overlapping(&tx, a, b)? {
                let est = estimator.estimate(&span.inputs);
                tx.execute(
                    "UPDATE usage_span SET watts_centi=?1, layer=?2 WHERE id=?3",
                    params![watts_centi(est.watts), layer_str(est.layer), id],
                )?;
                lo = lo.min(minute_floor(span.start.timestamp()));
                hi = hi.max(minute_ceil(span.end.timestamp()));
            }
            tx.commit()?;
        }
        let mut report = self.recompute_costs(ts_to_dt(lo.max(floor)), ts_to_dt(hi), tariffs)?;
        report.clipped_by_compaction |= from.timestamp() < floor;
        Ok(report)
    }

    // ------------------------------------------------------------ power events (D11)

    /// Persist power events so they outlive the ~5-day Windows System log. Duplicates are ignored.
    pub fn save_power_events(&mut self, events: &[PowerEvent]) -> Result<usize> {
        let tx = self.conn.transaction()?;
        let mut n = 0;
        {
            let mut st = tx.prepare_cached(
                "INSERT OR IGNORE INTO power_event(ts_ms,provider,event_id,data_json) VALUES(?1,?2,?3,?4)",
            )?;
            for e in events {
                n += st.execute(params![e.time.timestamp_millis(), e.provider, e.id, serde_json::to_string(&e.data)?])?;
            }
        }
        tx.commit()?;
        Ok(n)
    }

    pub fn power_events_between(&self, from: DateTime<Utc>, to: DateTime<Utc>) -> Result<Vec<PowerEvent>> {
        let mut st = self.conn.prepare_cached(
            "SELECT ts_ms,provider,event_id,data_json FROM power_event WHERE ts_ms>=?1 AND ts_ms<=?2 ORDER BY ts_ms",
        )?;
        let rows = st.query_map(params![from.timestamp_millis(), to.timestamp_millis()], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, u32>(2)?, r.get::<_, String>(3)?))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (ms, provider, id, data) = row?;
            out.push(PowerEvent {
                time: Utc.timestamp_millis_opt(ms).single().ok_or_else(|| StorageError::Corrupt("bad event time".into()))?,
                provider,
                id,
                data: serde_json::from_str(&data)?,
            });
        }
        Ok(out)
    }

    pub fn db_size_bytes(&self) -> Result<i64> {
        let pages: i64 = self.conn.query_row("PRAGMA page_count", [], |r| r.get(0))?;
        let size: i64 = self.conn.query_row("PRAGMA page_size", [], |r| r.get(0))?;
        Ok(pages * size)
    }
}

// ---------------------------------------------------------------- helpers

fn source_str(s: PowerSource) -> &'static str {
    match s {
        PowerSource::Ac => "ac",
        PowerSource::Battery => "battery",
        PowerSource::Unknown => "unknown",
    }
}

fn parse_source(s: &str) -> PowerSource {
    match s {
        "ac" => PowerSource::Ac,
        "battery" => PowerSource::Battery,
        _ => PowerSource::Unknown,
    }
}

fn add(t: &mut Totals, o: &Totals) {
    t.seconds += o.seconds;
    t.kwh_nano += o.kwh_nano;
    t.cost_nano += o.cost_nano;
}

pub(crate) fn minute_floor(ts: i64) -> i64 {
    ts - ts.rem_euclid(60)
}

pub(crate) fn minute_ceil(ts: i64) -> i64 {
    minute_floor(ts + 59)
}

pub(crate) fn insert_span(conn: &Connection, s: &Span) -> Result<()> {
    conn.prepare_cached(
        "INSERT INTO usage_span(start_ts,end_ts,state,watts_centi,layer,billable,source,inputs_json) VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
    )?
    .execute(params![
        s.start.timestamp(),
        s.end.timestamp(),
        state_str(s.inputs.state),
        watts_centi(s.watts),
        layer_str(s.layer),
        s.billable as i64,
        s.source.as_str(),
        serde_json::to_string(&s.inputs)?
    ])?;
    Ok(())
}

pub(crate) fn spans_overlapping(conn: &Connection, a: i64, b: i64) -> Result<Vec<(i64, Span)>> {
    let mut st = conn.prepare_cached(
        "SELECT id,start_ts,end_ts,watts_centi,layer,billable,source,inputs_json FROM usage_span WHERE start_ts<?2 AND end_ts>?1 ORDER BY start_ts",
    )?;
    let rows = st.query_map(params![a, b], |r| {
        Ok((
            r.get::<_, i64>(0)?,
            r.get::<_, i64>(1)?,
            r.get::<_, i64>(2)?,
            r.get::<_, i64>(3)?,
            r.get::<_, String>(4)?,
            r.get::<_, i64>(5)?,
            r.get::<_, String>(6)?,
            r.get::<_, String>(7)?,
        ))
    })?;
    let mut out = Vec::new();
    for row in rows {
        let (id, s, e, wc, layer, billable, source, inputs) = row?;
        out.push((
            id,
            Span {
                start: ts_to_dt(s),
                end: ts_to_dt(e),
                inputs: serde_json::from_str(&inputs)?,
                watts: Decimal::new(wc, 2),
                layer: parse_layer(&layer),
                billable: billable != 0,
                source: SpanSource::parse(&source),
            },
        ));
    }
    Ok(out)
}

/// Price a sample and add its minute rows (summing when a minute already has a row).
pub(crate) fn price_into(conn: &Connection, sample: &UsageSample, tariffs: &TariffSet) -> Result<()> {
    let intervals = cost_engine::price_sample(tariffs, sample)?;
    let mut st = conn.prepare_cached(
        "INSERT INTO usage_cost(minute_ts,state,billable,seconds,kwh_nano,tariff_id,tariff_version,period_id,price_nano,cost_nano)
         VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)
         ON CONFLICT(minute_ts,state,billable) DO UPDATE SET
           seconds=seconds+excluded.seconds, kwh_nano=kwh_nano+excluded.kwh_nano, cost_nano=cost_nano+excluded.cost_nano",
    )?;
    for i in intervals {
        st.execute(params![
            i.minute_start.timestamp(),
            state_str(i.state),
            i.billable as i64,
            i.seconds,
            i.kwh_nano,
            i.tariff_id,
            i.tariff_version,
            i.period_id,
            i.price_nano_per_kwh,
            i.cost_nano
        ])?;
    }
    Ok(())
}


