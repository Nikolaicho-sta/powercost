//! Read-side queries for the dashboard: raw minute rows (what-if) and bucketed timelines.

use crate::*;
use chrono::{Datelike, Duration, NaiveDateTime};

/// One priced minute row (only history that is not compacted yet).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CostRow {
    pub minute: DateTime<Utc>,
    pub state: PowerState,
    pub billable: bool,
    pub seconds: i64,
    pub kwh_nano: i64,
    pub period_id: String,
    pub price_nano: i64,
    pub cost_nano: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Bucket {
    Hour,
    Day,
    Month,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BucketRow {
    /// UTC instant at which the bucket starts.
    pub start: DateTime<Utc>,
    /// Same instant on the local wall clock (for labels).
    pub local_start: NaiveDateTime,
    pub by_state: BTreeMap<PowerState, Totals>,
}

impl BucketRow {
    pub fn total(&self) -> Totals {
        let mut t = Totals::default();
        for v in self.by_state.values() {
            add(&mut t, v);
        }
        t
    }
}

fn local_midnight(date: NaiveDate, tz: Tz) -> DateTime<Utc> {
    let mut naive = date.and_hms_opt(0, 0, 0).unwrap();
    loop {
        if let Some(dt) = tz.from_local_datetime(&naive).earliest() {
            return dt.with_timezone(&Utc);
        }
        naive += Duration::minutes(30);
    }
}

impl Storage {
    pub fn cost_rows(&self, from: DateTime<Utc>, to: DateTime<Utc>) -> Result<Vec<CostRow>> {
        let mut st = self.conn.prepare_cached(
            "SELECT minute_ts,state,billable,seconds,kwh_nano,period_id,price_nano,cost_nano FROM usage_cost
             WHERE minute_ts>=?1 AND minute_ts<?2 ORDER BY minute_ts",
        )?;
        let rows = st.query_map(params![from.timestamp(), to.timestamp()], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, i64>(2)?,
                r.get::<_, i64>(3)?,
                r.get::<_, i64>(4)?,
                r.get::<_, String>(5)?,
                r.get::<_, i64>(6)?,
                r.get::<_, i64>(7)?,
            ))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (m, state, billable, seconds, kwh_nano, period_id, price_nano, cost_nano) = row?;
            out.push(CostRow {
                minute: ts_to_dt(m),
                state: parse_state(&state)?,
                billable: billable != 0,
                seconds,
                kwh_nano,
                period_id,
                price_nano,
                cost_nano,
            });
        }
        Ok(out)
    }

    /// Energy and cost per bucket and state for `[from, to)`. Only buckets with data are
    /// returned. Hour buckets are UTC hours (labelled in local time); Day/Month use LOCAL dates,
    /// so DST days keep their true length. Compacted history comes from the aggregate tables.
    pub fn timeline(&self, from: DateTime<Utc>, to: DateTime<Utc>, tz: Tz, bucket: Bucket) -> Result<Vec<BucketRow>> {
        let mut acc: BTreeMap<DateTime<Utc>, BTreeMap<PowerState, Totals>> = BTreeMap::new();
        let mut put = |start: DateTime<Utc>, state: PowerState, t: Totals| {
            add(acc.entry(start).or_default().entry(state).or_default(), &t);
        };
        let key_of = |t: DateTime<Utc>| -> DateTime<Utc> {
            match bucket {
                Bucket::Hour => ts_to_dt(t.timestamp() - t.timestamp().rem_euclid(3600)),
                Bucket::Day => local_midnight(t.with_timezone(&tz).date_naive(), tz),
                Bucket::Month => {
                    let d = t.with_timezone(&tz).date_naive();
                    local_midnight(NaiveDate::from_ymd_opt(d.year(), d.month(), 1).unwrap(), tz)
                }
            }
        };

        for r in self.cost_rows(from, to)? {
            put(key_of(r.minute), r.state, Totals { seconds: r.seconds, kwh_nano: r.kwh_nano, cost_nano: r.cost_nano });
        }

        match bucket {
            Bucket::Hour => {
                let mut st = self.conn.prepare_cached(
                    "SELECT hour_ts,state,SUM(seconds),SUM(kwh_nano),SUM(cost_nano) FROM agg_hourly
                     WHERE hour_ts>=?1 AND hour_ts<?2 GROUP BY hour_ts,state",
                )?;
                let rows = st.query_map(params![from.timestamp(), to.timestamp()], |r| {
                    Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, i64>(2)?, r.get::<_, i64>(3)?, r.get::<_, i64>(4)?))
                })?;
                for row in rows {
                    let (h, state, s, k, c) = row?;
                    put(ts_to_dt(h), parse_state(&state)?, Totals { seconds: s, kwh_nano: k, cost_nano: c });
                }
            }
            Bucket::Day | Bucket::Month => {
                let (d0, d1) = (from.with_timezone(&tz).date_naive(), to.with_timezone(&tz).date_naive());
                let mut st = self.conn.prepare_cached(
                    "SELECT day,state,SUM(seconds),SUM(kwh_nano),SUM(cost_nano) FROM agg_daily
                     WHERE day>=?1 AND day<=?2 GROUP BY day,state",
                )?;
                let rows = st.query_map(params![d0.to_string(), d1.to_string()], |r| {
                    Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, i64>(2)?, r.get::<_, i64>(3)?, r.get::<_, i64>(4)?))
                })?;
                for row in rows {
                    let (day, state, s, k, c) = row?;
                    let date = NaiveDate::parse_from_str(&day, "%Y-%m-%d").map_err(|e| StorageError::Corrupt(e.to_string()))?;
                    put(key_of(local_midnight(date, tz)), parse_state(&state)?, Totals { seconds: s, kwh_nano: k, cost_nano: c });
                }
            }
        }

        Ok(acc
            .into_iter()
            .map(|(start, by_state)| BucketRow { start, local_start: start.with_timezone(&tz).naive_local(), by_state })
            .collect())
    }
}

/// One export row: a bucket x state x tariff period.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BreakdownRow {
    pub start: DateTime<Utc>,
    pub local_start: NaiveDateTime,
    pub state: PowerState,
    pub period_id: String,
    pub totals: Totals,
}

impl Storage {
    /// Rows for exports: per hour or per LOCAL day, split by state and tariff period, combining
    /// minute rows with compacted aggregates (hourly rows for `Hour`, daily rows for `Day`).
    pub fn breakdown_rows(&self, from: DateTime<Utc>, to: DateTime<Utc>, tz: Tz, bucket: Bucket) -> Result<Vec<BreakdownRow>> {
        assert!(matches!(bucket, Bucket::Hour | Bucket::Day), "exports are hourly or daily");
        let mut acc: BTreeMap<(DateTime<Utc>, String, String), Totals> = BTreeMap::new();
        let key_of = |t: DateTime<Utc>| match bucket {
            Bucket::Day => local_midnight(t.with_timezone(&tz).date_naive(), tz),
            _ => ts_to_dt(t.timestamp() - t.timestamp().rem_euclid(3600)),
        };
        for r in self.cost_rows(from, to)? {
            add(acc.entry((key_of(r.minute), state_str(r.state).to_string(), r.period_id)).or_default(), &Totals { seconds: r.seconds, kwh_nano: r.kwh_nano, cost_nano: r.cost_nano });
        }
        if bucket == Bucket::Hour {
            let mut st = self.conn.prepare_cached(
                "SELECT hour_ts,state,period_id,seconds,kwh_nano,cost_nano FROM agg_hourly WHERE hour_ts>=?1 AND hour_ts<?2",
            )?;
            let rows = st.query_map(params![from.timestamp(), to.timestamp()], |r| {
                Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, i64>(3)?, r.get::<_, i64>(4)?, r.get::<_, i64>(5)?))
            })?;
            for row in rows {
                let (h, s, p, secs, k, c) = row?;
                add(acc.entry((ts_to_dt(h), s, p)).or_default(), &Totals { seconds: secs, kwh_nano: k, cost_nano: c });
            }
        } else {
            let (d0, d1) = (from.with_timezone(&tz).date_naive(), to.with_timezone(&tz).date_naive());
            let mut st = self.conn.prepare_cached(
                "SELECT day,state,period_id,seconds,kwh_nano,cost_nano FROM agg_daily WHERE day>=?1 AND day<=?2",
            )?;
            let rows = st.query_map(params![d0.to_string(), d1.to_string()], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, i64>(3)?, r.get::<_, i64>(4)?, r.get::<_, i64>(5)?))
            })?;
            for row in rows {
                let (day, s, p, secs, k, c) = row?;
                let date = NaiveDate::parse_from_str(&day, "%Y-%m-%d").map_err(|e| StorageError::Corrupt(e.to_string()))?;
                add(acc.entry((local_midnight(date, tz), s, p)).or_default(), &Totals { seconds: secs, kwh_nano: k, cost_nano: c });
            }
        }
        acc.into_iter()
            .map(|((start, state, period_id), totals)| {
                Ok(BreakdownRow { start, local_start: start.with_timezone(&tz).naive_local(), state: parse_state(&state)?, period_id, totals })
            })
            .collect()
    }
}