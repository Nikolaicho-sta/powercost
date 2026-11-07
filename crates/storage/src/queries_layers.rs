use crate::*;

impl Storage {
    /// Seconds of stored spans per estimator layer within `[from, to)` (spans clipped to the range).
    /// Only covers history that is not compacted.
    pub fn layer_seconds(&self, from: DateTime<Utc>, to: DateTime<Utc>) -> Result<BTreeMap<String, i64>> {
        let mut st = self.conn.prepare_cached(
            "SELECT layer, SUM(MIN(end_ts,?2) - MAX(start_ts,?1)) FROM usage_span
             WHERE start_ts<?2 AND end_ts>?1 AND state IN ('active','idle','sleep') GROUP BY layer",
        )?;
        let rows = st.query_map(params![from.timestamp(), to.timestamp()], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?;
        let mut out = BTreeMap::new();
        for row in rows {
            let (layer, secs) = row?;
            out.insert(layer, secs);
        }
        Ok(out)
    }

    /// The earliest moment for which anything is stored (minute rows, compacted hours or spans).
    pub fn earliest_data(&self) -> Result<Option<DateTime<Utc>>> {
        let ts: Option<i64> = self.conn.query_row(
            "SELECT MIN(v) FROM (
                SELECT MIN(minute_ts) AS v FROM usage_cost
                UNION ALL SELECT MIN(hour_ts) FROM agg_hourly
                UNION ALL SELECT MIN(start_ts) FROM usage_span)",
            [],
            |r| r.get(0),
        )?;
        Ok(ts.map(ts_to_dt))
    }
}