//! Compaction of old minute rows into hourly + daily aggregates.

use crate::*;
use chrono::{Duration, NaiveTime};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CompactionReport {
    pub minute_rows_removed: usize,
    pub spans_removed: usize,
    pub hourly_rows_written: usize,
    pub daily_rows_written: usize,
    /// Everything before this instant is now aggregates only.
    pub compacted_until: Option<DateTime<Utc>>,
}

/// Start of the local day containing `t`, as UTC (handles zones where midnight does not exist).
fn local_midnight_utc(t: DateTime<Utc>, tz: Tz) -> DateTime<Utc> {
    let date = t.with_timezone(&tz).date_naive();
    let mut naive = date.and_time(NaiveTime::MIN);
    loop {
        if let Some(dt) = tz.from_local_datetime(&naive).earliest() {
            return dt.with_timezone(&Utc);
        }
        naive += Duration::minutes(30); // skipped by a DST jump at midnight
    }
}

impl Storage {
    /// Aggregate and delete everything before the start of the local day containing `before`.
    /// Idempotent; safe to call every day.
    pub fn compact(&mut self, before: DateTime<Utc>, tz: Tz) -> Result<CompactionReport> {
        let cutoff = local_midnight_utc(before, tz).timestamp();
        if cutoff <= self.compacted_until()? {
            return Ok(CompactionReport::default());
        }
        let tx = self.conn.transaction()?;
        // hourly
        let hourly_rows_written = tx.execute(
            "INSERT INTO agg_hourly(hour_ts,state,period_id,seconds,kwh_nano,cost_nano)
             SELECT minute_ts - (minute_ts % 3600), state, period_id, SUM(seconds), SUM(kwh_nano), SUM(cost_nano)
             FROM usage_cost WHERE minute_ts < ?1 GROUP BY 1,2,3
             ON CONFLICT(hour_ts,state,period_id) DO UPDATE SET
               seconds=seconds+excluded.seconds, kwh_nano=kwh_nano+excluded.kwh_nano, cost_nano=cost_nano+excluded.cost_nano",
            params![cutoff],
        )?;
        let mut report = CompactionReport { hourly_rows_written, ..Default::default() };

        // daily by LOCAL date, grouped in Rust so DST days are right
        let mut days: BTreeMap<(String, String, String), (i64, i64, i64)> = BTreeMap::new();
        {
            let mut st = tx.prepare(
                "SELECT minute_ts,state,period_id,seconds,kwh_nano,cost_nano FROM usage_cost WHERE minute_ts < ?1",
            )?;
            let rows = st.query_map(params![cutoff], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, i64>(3)?,
                    r.get::<_, i64>(4)?,
                    r.get::<_, i64>(5)?,
                ))
            })?;
            for row in rows {
                let (m, state, period, secs, kwh, cost) = row?;
                let day = ts_to_dt(m).with_timezone(&tz).date_naive().to_string();
                let e = days.entry((day, state, period)).or_default();
                e.0 += secs;
                e.1 += kwh;
                e.2 += cost;
            }
        }
        for ((day, state, period), (secs, kwh, cost)) in &days {
            tx.execute(
                "INSERT INTO agg_daily(day,state,period_id,seconds,kwh_nano,cost_nano) VALUES(?1,?2,?3,?4,?5,?6)
                 ON CONFLICT(day,state,period_id) DO UPDATE SET
                   seconds=seconds+excluded.seconds, kwh_nano=kwh_nano+excluded.kwh_nano, cost_nano=cost_nano+excluded.cost_nano",
                params![day, state, period, secs, kwh, cost],
            )?;
            report.daily_rows_written += 1;
        }

        report.minute_rows_removed = tx.execute("DELETE FROM usage_cost WHERE minute_ts < ?1", params![cutoff])?;
        report.spans_removed = tx.execute("DELETE FROM usage_span WHERE end_ts <= ?1", params![cutoff])?;
        tx.execute(
            "DELETE FROM power_event WHERE ts_ms < ?1",
            params![(cutoff - 400 * 86_400) * 1000],
        )?;
        tx.execute(
            "INSERT INTO settings(key,value) VALUES('compacted_until_ts',?1) ON CONFLICT(key) DO UPDATE SET value=excluded.value",
            params![cutoff.to_string()],
        )?;
        tx.commit()?;
        report.compacted_until = Some(ts_to_dt(cutoff));
        Ok(report)
    }

    /// Reclaim file space after a large compaction.
    pub fn vacuum(&self) -> Result<()> {
        self.conn.execute_batch("VACUUM")?;
        Ok(())
    }
}

