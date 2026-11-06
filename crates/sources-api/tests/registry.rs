use chrono::{DateTime, Duration, TimeZone, Utc};
use sources_api::*;

struct Fake {
    id: &'static str,
    result: Result<WallReading, SourceError>,
    max_age: Duration,
}

impl WallPowerSource for Fake {
    fn id(&self) -> &str {
        self.id
    }
    fn kind(&self) -> SourceKind {
        SourceKind::Other
    }
    fn describe(&self) -> String {
        format!("fake {}", self.id)
    }
    fn read(&mut self) -> Result<WallReading, SourceError> {
        self.result.clone()
    }
    fn max_age(&self) -> Duration {
        self.max_age
    }
}

fn now() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 5, 4, 12, 0, 0).unwrap()
}

fn fake(id: &'static str, result: Result<WallReading, SourceError>) -> Box<dyn WallPowerSource> {
    Box::new(Fake { id, result, max_age: Duration::seconds(30) })
}

fn reading(watts: f64, age_secs: i64) -> Result<WallReading, SourceError> {
    Ok(WallReading { watts, at: now() - Duration::seconds(age_secs) })
}

#[test]
fn the_first_healthy_source_wins_and_earlier_problems_are_reported() {
    let mut r = SourceRegistry::new();
    r.register(fake("a", Err(SourceError::Unavailable("plug offline".into())))).unwrap();
    r.register(fake("b", reading(54.5, 2))).unwrap();
    r.register(fake("c", reading(99.0, 1))).unwrap();
    let (best, issues) = r.latest(now());
    let (id, reading) = best.unwrap();
    assert_eq!((id.as_str(), reading.watts), ("b", 54.5));
    assert_eq!(issues.len(), 1);
    assert_eq!(issues[0].source_id, "a");
    assert!(issues[0].message.contains("plug offline"));
}

#[test]
fn stale_and_implausible_readings_are_rejected() {
    let mut r = SourceRegistry::new();
    r.register(fake("stale", reading(50.0, 120))).unwrap();
    r.register(fake("negative", reading(-5.0, 1))).unwrap();
    r.register(fake("nan", reading(f64::NAN, 1))).unwrap();
    r.register(fake("huge", reading(12_000.0, 1))).unwrap();
    let (best, issues) = r.latest(now());
    assert!(best.is_none());
    assert_eq!(issues.len(), 4);
    assert!(issues[0].message.contains("old"));
    assert!(issues[1].message.contains("plausible"));
}

#[test]
fn a_reading_exactly_at_max_age_is_still_fresh() {
    let mut r = SourceRegistry::new();
    r.register(fake("edge", reading(40.0, 30))).unwrap();
    assert!(r.latest(now()).0.is_some());
}

#[test]
fn duplicate_ids_are_refused() {
    let mut r = SourceRegistry::new();
    r.register(fake("x", reading(1.0, 0))).unwrap();
    let err = r.register(fake("x", reading(2.0, 0))).unwrap_err();
    assert!(err.contains("already registered"));
    assert_eq!(r.ids(), vec!["x".to_string()]);
}

#[test]
fn an_empty_registry_has_no_reading_and_no_issues() {
    let mut r = SourceRegistry::new();
    assert!(r.is_empty());
    let (best, issues) = r.latest(now());
    assert!(best.is_none() && issues.is_empty());
}

#[test]
fn plausibility_bounds() {
    let ok = |w| validate_reading(&WallReading { watts: w, at: now() }).is_ok();
    assert!(ok(0.0) && ok(5000.0) && ok(32.5));
    assert!(!ok(-0.1) && !ok(5000.1) && !ok(f64::INFINITY));
}