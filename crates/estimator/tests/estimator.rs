use estimator::*;
use pc_core::{Confidence, PowerState};
use rust_decimal::Decimal;
use std::str::FromStr;

fn d(s: &str) -> Decimal {
    Decimal::from_str(s).unwrap()
}

fn laptop_cfg() -> EstimatorConfig {
    EstimatorConfig {
        psu_efficiency: d("0.9"),
        baseline_w: d("8"),
        per_drive_w: d("2"),
        hardware: Hardware {
            cpu_tdp_w: d("45"),
            cpu_idle_w: d("5"),
            gpu_tdp_w: Some(d("60")),
            gpu_idle_w: d("5"),
            drives: 1,
        },
        ..EstimatorConfig::default()
    }
}

fn running(state: PowerState, cpu_load: f64, gpu_util: f64) -> Inputs {
    Inputs { cpu_load_pct: cpu_load, gpu_util_pct: gpu_util, ..Inputs::new(state) }
}

// ---------- calibration ----------

#[test]
fn calibration_interpolates_linearly_and_clamps() {
    let c = Calibration::new(d("20"), d("100")).unwrap();
    assert_eq!(c.watts_at(0), d("20"));
    assert_eq!(c.watts_at(500), d("60"));
    assert_eq!(c.watts_at(1000), d("100"));
    assert_eq!(c.watts_at(5000), d("100"));
}

#[test]
fn calibration_supports_a_mid_point() {
    let c = Calibration::from_points(vec![
        CalibrationPoint { util_permille: 1000, watts: d("120") },
        CalibrationPoint { util_permille: 0, watts: d("20") },
        CalibrationPoint { util_permille: 500, watts: d("40") }, // sorted on construction
    ])
    .unwrap();
    assert_eq!(c.watts_at(250), d("30"));
    assert_eq!(c.watts_at(750), d("80"));
}

#[test]
fn calibration_rejects_bad_input() {
    assert!(Calibration::new(d("100"), d("100")).is_err());
    assert!(Calibration::new(d("120"), d("100")).is_err());
    assert!(Calibration::new(d("0"), d("100")).is_err());
    assert!(Calibration::from_points(vec![CalibrationPoint { util_permille: 0, watts: d("20") }]).is_err());
    assert!(Calibration::from_points(vec![
        CalibrationPoint { util_permille: 0, watts: d("20") },
        CalibrationPoint { util_permille: 0, watts: d("30") },
        CalibrationPoint { util_permille: 1000, watts: d("90") },
    ])
    .is_err());
}

// ---------- layers ----------

#[test]
fn default_model_uses_tdp_baseline_and_psu_efficiency() {
    let e = Estimator::new(laptop_cfg()).unwrap();
    // idle: cpu 5 + gpu 5 + rest (8 + 2) = 20 W DC / 0.9
    let idle = e.estimate(&running(PowerState::Idle, 0.0, 0.0));
    assert_eq!(idle.layer, Layer::Default);
    assert_eq!(idle.confidence(), Confidence::DefaultModel);
    assert_eq!(idle.watts, d("22.22"));
    // full: 45 + 60 + 10 = 115 / 0.9
    let full = e.estimate(&running(PowerState::Idle, 100.0, 100.0));
    assert_eq!(full.watts, d("127.78"));
    assert!(full.low < full.watts && full.watts < full.high);
}

#[test]
fn sensors_layer_adds_modelled_rest_and_divides_by_efficiency() {
    let e = Estimator::new(laptop_cfg()).unwrap();
    let i = Inputs { cpu_power_w: Some(18.0), gpu_power_w: Some(35.0), ..running(PowerState::Idle, 60.0, 30.0) };
    let est = e.estimate(&i);
    assert_eq!(est.layer, Layer::Sensors);
    assert_eq!(est.confidence(), Confidence::MeasuredSensors);
    // (18 + 35 + 8 + 2) / 0.9 = 70.00
    assert_eq!(est.watts, d("70.00"));
    assert!(est.method.contains("CPU 18"));
}

#[test]
fn sensors_layer_models_gpu_when_only_cpu_is_measured() {
    let e = Estimator::new(laptop_cfg()).unwrap();
    let i = Inputs { cpu_power_w: Some(10.0), ..running(PowerState::Idle, 10.0, 0.0) };
    let est = e.estimate(&i);
    assert_eq!(est.layer, Layer::Sensors);
    assert!(est.method.contains("GPU modelled"));
    assert_eq!(est.watts, d("27.78")); // (10 + 5 + 10) / 0.9
}

#[test]
fn battery_charging_adds_to_wall_draw() {
    let e = Estimator::new(laptop_cfg()).unwrap();
    let base = Inputs { cpu_power_w: Some(18.0), gpu_power_w: Some(35.0), ..running(PowerState::Idle, 0.0, 0.0) };
    let charging = Inputs { charge_w: Some(9.0), ..base.clone() };
    let diff = e.estimate(&charging).watts - e.estimate(&base).watts;
    assert_eq!(diff, d("10.00")); // 9 W / 0.9
}

#[test]
fn calibration_wins_over_sensors_by_default_and_can_be_switched() {
    let mut cfg = laptop_cfg();
    cfg.calibration = Some(Calibration::new(d("30"), d("130")).unwrap());
    let i = Inputs { cpu_power_w: Some(18.0), gpu_power_w: Some(35.0), ..running(PowerState::Idle, 50.0, 10.0) };

    let est = Estimator::new(cfg.clone()).unwrap().estimate(&i);
    assert_eq!(est.layer, Layer::Calibrated);
    assert_eq!(est.watts, d("80.00")); // util 50% -> 30 + 100*0.5

    cfg.prefer_calibrated = false;
    assert_eq!(Estimator::new(cfg).unwrap().estimate(&i).layer, Layer::Sensors);
}

#[test]
fn fallback_chain_skips_unavailable_layers() {
    let mut cfg = laptop_cfg();
    cfg.calibration = Some(Calibration::new(d("30"), d("130")).unwrap());
    let est = Estimator::new(cfg).unwrap();
    // no sensors -> calibrated
    assert_eq!(est.estimate(&running(PowerState::Idle, 0.0, 0.0)).layer, Layer::Calibrated);
    // no sensors, no calibration -> default
    let est = Estimator::new(laptop_cfg()).unwrap();
    assert_eq!(est.estimate(&running(PowerState::Idle, 0.0, 0.0)).layer, Layer::Default);
}

#[test]
fn calibrated_utilisation_uses_the_busier_of_cpu_and_gpu() {
    let mut cfg = laptop_cfg();
    cfg.calibration = Some(Calibration::new(d("20"), d("120")).unwrap());
    let e = Estimator::new(cfg).unwrap();
    assert_eq!(e.estimate(&running(PowerState::Idle, 10.0, 80.0)).watts, d("100.00"));
}

// ---------- monitor ----------

#[test]
fn monitor_is_off_by_default_and_counts_only_when_active() {
    let mut cfg = laptop_cfg();
    let off = Estimator::new(cfg.clone()).unwrap().estimate(&running(PowerState::Active, 0.0, 0.0)).watts;
    cfg.include_monitor = true;
    let e = Estimator::new(cfg).unwrap();
    assert_eq!(e.estimate(&running(PowerState::Active, 0.0, 0.0)).watts - off, d("25.00"));
    assert_eq!(e.estimate(&running(PowerState::Idle, 0.0, 0.0)).watts, d("22.22")); // idle: monitor asleep
}

// ---------- non-running states ----------

#[test]
fn off_hibernate_and_plain_sleep_use_configured_watts() {
    let mut cfg = laptop_cfg();
    cfg.off_standby_w = d("1.5");
    cfg.sleep_w = d("2");
    cfg.hibernate_w = d("0.3");
    let e = Estimator::new(cfg.clone()).unwrap();
    assert_eq!(e.estimate(&Inputs::new(PowerState::Off)).watts, d("1.50"));
    assert_eq!(e.estimate(&Inputs::new(PowerState::Sleep)).watts, d("2.00"));
    assert_eq!(e.estimate(&Inputs::new(PowerState::Hibernate)).watts, d("0.30"));

    cfg.unplugged_when_off = true;
    assert_eq!(Estimator::new(cfg).unwrap().estimate(&Inputs::new(PowerState::Off)).watts, d("0.00"));
}

#[test]
fn modern_standby_is_priced_by_its_active_share() {
    // The reference laptop: 96.5% of "sleep" was actively running.
    let e = Estimator::new(laptop_cfg()).unwrap(); // idle model 22.22 W, sleep 1 W
    let est = e.estimate(&Inputs { sleep_active_permille: Some(965), ..Inputs::new(PowerState::Sleep) });
    // 22.22*0.965 + 1*0.035 = 21.48
    assert_eq!(est.watts, d("21.48"));
    assert!(est.method.contains("96.5%"));
    // far above what a flat 1 W sleep would claim
    assert!(est.watts > d("20"));
    // and a true deep sleep (0% active) is just sleep watts
    let deep = e.estimate(&Inputs { sleep_active_permille: Some(0), ..Inputs::new(PowerState::Sleep) });
    assert_eq!(deep.watts, d("1.00"));
}

#[test]
fn modern_standby_with_calibration_uses_calibrated_idle() {
    let mut cfg = laptop_cfg();
    cfg.calibration = Some(Calibration::new(d("15"), d("100")).unwrap());
    let e = Estimator::new(cfg).unwrap();
    let est = e.estimate(&Inputs { sleep_active_permille: Some(1000), ..Inputs::new(PowerState::Sleep) });
    assert_eq!(est.watts, d("15.00"));
    assert_eq!(est.layer, Layer::Calibrated);
}

// ---------- config validation ----------

#[test]
fn config_validation() {
    let mut cfg = laptop_cfg();
    cfg.psu_efficiency = d("1.2");
    assert!(Estimator::new(cfg).is_err());
    let mut cfg = laptop_cfg();
    cfg.psu_efficiency = d("0.3");
    assert!(Estimator::new(cfg).is_err());
    let mut cfg = laptop_cfg();
    cfg.sleep_w = d("-1");
    assert!(Estimator::new(cfg).is_err());
    assert!(Estimator::new(EstimatorConfig::default()).is_ok());
}

#[test]
fn inputs_and_config_roundtrip_through_json() {
    let i = Inputs { cpu_power_w: Some(12.5), sleep_active_permille: Some(964), ..Inputs::new(PowerState::Sleep) };
    let back: Inputs = serde_json::from_str(&serde_json::to_string(&i).unwrap()).unwrap();
    assert_eq!(i, back);
    let mut cfg = laptop_cfg();
    cfg.calibration = Some(Calibration::new(d("20"), d("100")).unwrap());
    let back: EstimatorConfig = serde_json::from_str(&serde_json::to_string(&cfg).unwrap()).unwrap();
    assert_eq!(cfg, back);
}
