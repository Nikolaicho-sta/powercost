//! CSV / JSON export. Amounts are written as exact decimal strings (6 decimals) derived from the
//! integer nano-units; nothing is formatted through floats.

use crate::*;
use pc_core::NANO;
use rust_decimal::Decimal;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ExportFormat {
    Csv,
    Json,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Granularity {
    Hour,
    Day,
}

fn fmt_nano(nano: i64) -> String {
    let mut d = (Decimal::from(nano) / Decimal::from(NANO)).round_dp(6);
    d.rescale(6);
    d.to_string()
}

/// Export `[from, to)` per hour or local day, split by power state and tariff period.
pub fn export(
    store: &Storage,
    tariff: &Tariff,
    from: DateTime<Utc>,
    to: DateTime<Utc>,
    granularity: Granularity,
    format: ExportFormat,
    generated_at: DateTime<Utc>,
) -> Result<String, DashboardError> {
    let tz = tariff.tz();
    let bucket = match granularity {
        Granularity::Hour => Bucket::Hour,
        Granularity::Day => Bucket::Day,
    };
    let rows = store.breakdown_rows(from, to, tz, bucket)?;
    let state_name = |s: PowerState| format!("{s:?}").to_lowercase();

    match format {
        ExportFormat::Csv => {
            let mut out = String::from("local_start,utc_start,state,tariff_period,seconds,kwh,cost,currency\n");
            for r in &rows {
                out.push_str(&format!(
                    "{},{},{},{},{},{},{},{}\n",
                    r.local_start.format("%Y-%m-%d %H:%M"),
                    r.start.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
                    state_name(r.state),
                    r.period_id,
                    r.totals.seconds,
                    fmt_nano(r.totals.kwh_nano),
                    fmt_nano(r.totals.cost_nano),
                    tariff.currency
                ));
            }
            Ok(out)
        }
        ExportFormat::Json => {
            let (mut secs, mut kwh, mut cost) = (0i64, 0i64, 0i64);
            for r in &rows {
                secs += r.totals.seconds;
                kwh += r.totals.kwh_nano;
                cost += r.totals.cost_nano;
            }
            let layers = store.layer_seconds(from, to)?;
            let total_layer: i64 = layers.values().sum();
            let permille = |k: &str| if total_layer > 0 { layers.get(k).copied().unwrap_or(0) * 1000 / total_layer } else { 0 };
            let doc = serde_json::json!({
                "app": "PowerCost",
                "generated_at": generated_at.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
                "disclaimer": "Every figure is an ESTIMATE of electricity use and cost, not a measurement of your meter. Fixed monthly fees are not included.",
                "timezone": tariff.timezone,
                "currency": tariff.currency,
                "tariff": { "id": tariff.id, "version": tariff.version, "name": tariff.name, "verification": tariff.verification },
                "range": { "from": from.to_rfc3339_opts(chrono::SecondsFormat::Secs, true), "to": to.to_rfc3339_opts(chrono::SecondsFormat::Secs, true) },
                "granularity": match granularity { Granularity::Hour => "hour", Granularity::Day => "day" },
                "totals": { "seconds": secs, "kwh": fmt_nano(kwh), "cost": fmt_nano(cost) },
                "estimate_method_share_permille": {
                    "measured_sensors": permille("sensors"),
                    "calibrated_model": permille("calibrated"),
                    "default_model": permille("default"),
                    "note": "share of running time per estimation layer; only history that is not yet compacted"
                },
                "rows": rows.iter().map(|r| serde_json::json!({
                    "local_start": r.local_start.format("%Y-%m-%dT%H:%M:%S").to_string(),
                    "utc_start": r.start.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
                    "state": state_name(r.state),
                    "tariff_period": r.period_id,
                    "seconds": r.totals.seconds,
                    "kwh": fmt_nano(r.totals.kwh_nano),
                    "cost": fmt_nano(r.totals.cost_nano),
                })).collect::<Vec<_>>(),
            });
            Ok(serde_json::to_string_pretty(&doc).expect("json"))
        }
    }
}
