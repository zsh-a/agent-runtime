use super::*;

#[test]
fn scheduler_fires_cron_once_per_matching_minute() {
    let scheduler = AgentScheduler;
    let spec = scheduled_spec(ScheduleSpec::Cron {
        expression: "30 9 * * MON-FRI".to_owned(),
        timezone: "UTC".to_owned(),
    });
    let now = parse_rfc3339("2026-07-03T09:30:45Z");

    assert!(scheduler.should_fire(&spec, now, None));
    assert!(!scheduler.should_fire(&spec, parse_rfc3339("2026-07-03T09:31:00Z"), None));
    assert!(!scheduler.should_fire(
        &spec,
        now,
        Some(&run_record_started_at(parse_rfc3339(
            "2026-07-03T09:30:05Z"
        ))),
    ));
    assert!(scheduler.should_fire(
        &spec,
        now,
        Some(&run_record_started_at(parse_rfc3339(
            "2026-07-03T09:29:59Z"
        ))),
    ));
}

#[test]
fn scheduler_applies_fixed_offset_timezone_for_cron() {
    let scheduler = AgentScheduler;
    let spec = scheduled_spec(ScheduleSpec::Cron {
        expression: "0 9 * * *".to_owned(),
        timezone: "+08:00".to_owned(),
    });

    assert!(scheduler.should_fire(&spec, parse_rfc3339("2026-07-03T01:00:00Z"), None));
    assert!(!scheduler.should_fire(&spec, parse_rfc3339("2026-07-03T09:00:00Z"), None));
}

#[test]
fn scheduler_applies_named_timezone_database_for_cron() {
    let scheduler = AgentScheduler;
    let spec = scheduled_spec(ScheduleSpec::Cron {
        expression: "0 9 * * *".to_owned(),
        timezone: "America/New_York".to_owned(),
    });

    assert!(scheduler.should_fire(&spec, parse_rfc3339("2026-07-03T13:00:00Z"), None));
    assert!(scheduler.should_fire(&spec, parse_rfc3339("2026-01-05T14:00:00Z"), None));
    assert!(!scheduler.should_fire(&spec, parse_rfc3339("2026-07-03T14:00:00Z"), None));
}

#[test]
fn scheduler_uses_standard_cron_or_for_restricted_day_fields() {
    let scheduler = AgentScheduler;
    let spec = scheduled_spec(ScheduleSpec::Cron {
        expression: "30 9 1 * MON".to_owned(),
        timezone: "UTC".to_owned(),
    });

    assert!(scheduler.should_fire(&spec, parse_rfc3339("2026-07-01T09:30:00Z"), None));
    assert!(scheduler.should_fire(&spec, parse_rfc3339("2026-07-06T09:30:00Z"), None));
    assert!(!scheduler.should_fire(&spec, parse_rfc3339("2026-07-02T09:30:00Z"), None));
}
