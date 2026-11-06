use crate::{Result, StorageError};
use rusqlite::Connection;

const CURRENT: i64 = 1;

/// Forward-only migrations keyed by PRAGMA user_version.
pub(crate) fn migrate(conn: &mut Connection) -> Result<()> {
    let version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    if version > CURRENT {
        return Err(StorageError::TooNew(version));
    }
    if version < 1 {
        let tx = conn.transaction()?;
        tx.execute_batch(V1)?;
        tx.execute_batch(&format!("PRAGMA user_version = {CURRENT};"))?;
        tx.commit()?;
    }
    Ok(())
}

const V1: &str = "
CREATE TABLE settings(key TEXT PRIMARY KEY, value TEXT NOT NULL);

CREATE TABLE heartbeat(
  ts INTEGER PRIMARY KEY, session_id TEXT NOT NULL, state TEXT NOT NULL, source TEXT NOT NULL);

-- Source of truth: constant-power spans with the inputs they were estimated from.
CREATE TABLE usage_span(
  id INTEGER PRIMARY KEY,
  start_ts INTEGER NOT NULL, end_ts INTEGER NOT NULL,
  state TEXT NOT NULL, watts_centi INTEGER NOT NULL, layer TEXT NOT NULL,
  billable INTEGER NOT NULL, source TEXT NOT NULL, inputs_json TEXT NOT NULL,
  CHECK (end_ts > start_ts));
CREATE INDEX idx_span_start ON usage_span(start_ts);
CREATE INDEX idx_span_end ON usage_span(end_ts);

-- Derived 1-minute priced intervals (rebuildable from usage_span + tariffs).
CREATE TABLE usage_cost(
  minute_ts INTEGER NOT NULL, state TEXT NOT NULL, billable INTEGER NOT NULL,
  seconds INTEGER NOT NULL, kwh_nano INTEGER NOT NULL,
  tariff_id TEXT NOT NULL, tariff_version INTEGER NOT NULL, period_id TEXT NOT NULL,
  price_nano INTEGER NOT NULL, cost_nano INTEGER NOT NULL,
  PRIMARY KEY(minute_ts, state, billable)) WITHOUT ROWID;

-- Compacted history.
CREATE TABLE agg_hourly(
  hour_ts INTEGER NOT NULL, state TEXT NOT NULL, period_id TEXT NOT NULL,
  seconds INTEGER NOT NULL, kwh_nano INTEGER NOT NULL, cost_nano INTEGER NOT NULL,
  PRIMARY KEY(hour_ts, state, period_id)) WITHOUT ROWID;
CREATE TABLE agg_daily(
  day TEXT NOT NULL, state TEXT NOT NULL, period_id TEXT NOT NULL,
  seconds INTEGER NOT NULL, kwh_nano INTEGER NOT NULL, cost_nano INTEGER NOT NULL,
  PRIMARY KEY(day, state, period_id)) WITHOUT ROWID;

CREATE TABLE gap(
  id INTEGER PRIMARY KEY, start_ts INTEGER NOT NULL, end_ts INTEGER NOT NULL,
  kind TEXT NOT NULL, confidence TEXT NOT NULL, evidence_json TEXT NOT NULL,
  note TEXT, user_override TEXT,
  UNIQUE(start_ts, end_ts, kind));

-- Copies of Windows power events; the System log only keeps ~5 days.
CREATE TABLE power_event(
  ts_ms INTEGER NOT NULL, provider TEXT NOT NULL, event_id INTEGER NOT NULL, data_json TEXT NOT NULL,
  PRIMARY KEY(ts_ms, provider, event_id)) WITHOUT ROWID;
";
