//! "What if" scenarios, computed from the stored priced minutes (so they use the prices that
//! actually applied). They are estimates built on estimates: the UI says so.

use crate::*;
use chrono::Timelike;
use pc_core::NANO;
use rust_decimal::prelude::*;
use rust_decimal::Decimal;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WhatIfParams {
    /// Night window in local hours; wraps midnight when start > end (23 -> 7).
    pub night_start_hour: u32,
    pub night_end_hour: u32,
    /// Standby watts of a shut-down PC (0 if you switch it off at the wall).
    pub off_watts: Decimal,
    /// Share of daytime ACTIVE energy moved to the cheapest tariff period, 0..=100.
    pub shift_percent: u32,
}

impl Default for WhatIfParams {
    fn default() -> Self {
        Self { night_start_hour: 23, night_end_hour: 7, off_watts: Decimal::ONE, shift_percent: 50 }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Scenario {
    pub current_cost_nano: i64,
    pub new_cost_nano: i64,
    pub saving_nano: i64,
    /// `saving_nano` extrapolated to a year from the length of the analysed window (rough).
    pub saving_per_year_nano: i64,
    pub kwh_saved_nano: i64,
    /// Hours of usage the scenario touches.
    pub hours_affected: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WhatIfResult {
    pub from: DateTime<Utc>,
    pub to: DateTime<Utc>,
    /// Cost if the PC were shut down during the night window instead of idling/sleeping.
    pub shutdown_at_night: Scenario,
    /// Cost if `shift_percent` of heavy daytime use ran in the cheapest period instead.
    pub shift_to_cheap_period: Scenario,
    pub cheapest_period: String,
    pub params: WhatIfParams,
    /// Compacted history is excluded because minute data no longer exists for it.
    pub note: String,
}

fn in_window(hour: u32, start: u32, end: u32) -> bool {
    if start == end {
        false
    } else if start < end {
        hour >= start && hour < end
    } else {
        hour >= start || hour < end
    }
}

fn nano(d: Decimal) -> i64 {
    d.round().to_i64().unwrap_or(0)
}

pub fn what_if(
    store: &Storage,
    tariff: &Tariff,
    params: &WhatIfParams,
    from: DateTime<Utc>,
    to: DateTime<Utc>,
) -> Result<WhatIfResult, DashboardError> {
    let tz = tariff.tz();
    let rows = store.cost_rows(from, to)?;

    // cheapest period of the current tariff
    let (cheap_idx, cheap_price) = (0..tariff.periods.len())
        .map(|i| (i, tariff.price_for_index(i).gross))
        .min_by_key(|(_, p)| *p)
        .expect("a tariff has at least one period");
    let cheap_nano = cheap_price * Decimal::from(NANO);

    let nano_d = Decimal::from(NANO);
    let (mut sd_cur, mut sd_new, mut sd_kwh, mut sd_secs) = (Decimal::ZERO, Decimal::ZERO, Decimal::ZERO, 0i64);
    let (mut sh_cur, mut sh_new, mut sh_secs) = (Decimal::ZERO, Decimal::ZERO, 0i64);
    let share = Decimal::from(params.shift_percent.min(100)) / Decimal::from(100);

    for r in rows.iter().filter(|r| r.billable) {
        let hour = r.minute.with_timezone(&tz).hour();
        // 1) shut down instead of idle/sleep during the night window
        if matches!(r.state, PowerState::Idle | PowerState::Sleep) && in_window(hour, params.night_start_hour, params.night_end_hour) {
            let off_kwh_nano = Decimal::from(r.seconds) * params.off_watts / Decimal::from(3_600_000) * nano_d;
            let off_cost = off_kwh_nano * Decimal::from(r.price_nano) / nano_d;
            sd_cur += Decimal::from(r.cost_nano);
            sd_new += off_cost;
            sd_kwh += Decimal::from(r.kwh_nano) - off_kwh_nano;
            sd_secs += r.seconds;
        }
        // 2) move part of the heavy daytime use to the cheapest period
        if r.state == PowerState::Active && r.period_id != tariff.periods[cheap_idx].id && Decimal::from(r.price_nano) > cheap_nano {
            let moved = Decimal::from(r.kwh_nano) * share;
            let saving = moved * (Decimal::from(r.price_nano) - cheap_nano) / nano_d;
            sh_cur += Decimal::from(r.cost_nano);
            sh_new += Decimal::from(r.cost_nano) - saving;
            sh_secs += r.seconds;
        }
    }

    let window_days = Decimal::from(((to - from).num_seconds()).max(1)) / Decimal::from(86_400);
    let scenario = |cur: Decimal, new: Decimal, kwh: Decimal, secs: i64| Scenario {
        current_cost_nano: nano(cur),
        new_cost_nano: nano(new),
        saving_nano: nano(cur) - nano(new),
        saving_per_year_nano: nano((cur - new) * Decimal::from(365) / window_days),
        kwh_saved_nano: nano(kwh),
        hours_affected: secs / 3600,
    };
    let compacted = store.get_setting::<i64>("compacted_until_ts")?.unwrap_or(i64::MIN);
    let note = if compacted > from.timestamp() {
        "Part of this range is older than the detailed history (compacted), so it is not included.".to_string()
    } else {
        String::new()
    };

    Ok(WhatIfResult {
        from,
        to,
        shutdown_at_night: scenario(sd_cur, sd_new, sd_kwh, sd_secs),
        shift_to_cheap_period: scenario(sh_cur, sh_new, Decimal::ZERO, sh_secs),
        cheapest_period: tariff.periods[cheap_idx].label.clone(),
        params: params.clone(),
        note,
    })
}


