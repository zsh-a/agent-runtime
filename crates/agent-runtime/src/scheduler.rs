use std::collections::BTreeSet;

use agent_core::{AgentRunRecord, AgentSpec, ScheduleSpec};
use chrono::{Datelike, Timelike};
use time::{OffsetDateTime, UtcOffset};

#[derive(Clone, Copy)]
pub struct AgentScheduler;

impl AgentScheduler {
    pub fn should_fire(
        &self,
        spec: &AgentSpec,
        now: OffsetDateTime,
        last_run: Option<&AgentRunRecord>,
    ) -> bool {
        match spec.schedule {
            ScheduleSpec::Manual => false,
            ScheduleSpec::Interval {
                every_seconds,
                preferred_hour_local,
                jitter_seconds,
            } => {
                if let Some(last) = last_run {
                    let elapsed = now - last.started_at;
                    if elapsed.whole_seconds() < every_seconds as i64 {
                        return false;
                    }
                }
                let Some(hour) = preferred_hour_local else {
                    return true;
                };
                let jitter = jitter_seconds.unwrap_or(300) as i64;
                let now_hour = now.hour() as i64;
                let now_minute = now.minute() as i64;
                let seconds_from_target = ((now_hour - hour as i64) * 3600 + now_minute * 60).abs();
                seconds_from_target <= jitter
            }
            ScheduleSpec::Cron {
                ref expression,
                ref timezone,
            } => should_fire_cron(expression, timezone, now, last_run),
        }
    }
}

fn should_fire_cron(
    expression: &str,
    timezone: &str,
    now: OffsetDateTime,
    last_run: Option<&AgentRunRecord>,
) -> bool {
    let Some(schedule) = CronSchedule::parse(expression) else {
        return false;
    };
    let Some(local_now) = cron_local_time(timezone, now) else {
        return false;
    };
    if !schedule.matches(local_now) {
        return false;
    }
    let current_fire_minute = now.unix_timestamp().div_euclid(60);
    if let Some(last) = last_run
        && last.started_at.unix_timestamp().div_euclid(60) >= current_fire_minute
    {
        return false;
    }
    true
}

fn cron_local_time(timezone: &str, now: OffsetDateTime) -> Option<CronLocalTime> {
    if let Some(offset) = parse_timezone_offset(timezone) {
        return Some(CronLocalTime::from_offset_datetime(now.to_offset(offset)));
    }

    let tz = timezone.trim().parse::<chrono_tz::Tz>().ok()?;
    let utc =
        chrono::DateTime::<chrono::Utc>::from_timestamp(now.unix_timestamp(), now.nanosecond())?;
    let local = utc.with_timezone(&tz);
    Some(CronLocalTime {
        minute: local.minute(),
        hour: local.hour(),
        day_of_month: local.day(),
        month: local.month(),
        weekday_from_sunday: local.weekday().num_days_from_sunday(),
    })
}

fn parse_timezone_offset(timezone: &str) -> Option<UtcOffset> {
    let timezone = timezone.trim();
    if timezone.eq_ignore_ascii_case("UTC")
        || timezone.eq_ignore_ascii_case("Etc/UTC")
        || timezone.eq_ignore_ascii_case("Z")
        || matches!(timezone, "+00:00" | "-00:00")
    {
        return Some(UtcOffset::UTC);
    }
    let sign = match timezone.as_bytes().first().copied()? {
        b'+' => 1,
        b'-' => -1,
        _ => return None,
    };
    let mut parts = timezone[1..].split(':');
    let hours = parts.next()?.parse::<i32>().ok()?;
    let minutes = parts.next().unwrap_or("0").parse::<i32>().ok()?;
    let seconds = parts.next().unwrap_or("0").parse::<i32>().ok()?;
    if parts.next().is_some()
        || !(0..=23).contains(&hours)
        || !(0..=59).contains(&minutes)
        || !(0..=59).contains(&seconds)
    {
        return None;
    }
    UtcOffset::from_whole_seconds(sign * (hours * 3600 + minutes * 60 + seconds)).ok()
}

#[derive(Debug, Clone, Copy)]
struct CronLocalTime {
    minute: u32,
    hour: u32,
    day_of_month: u32,
    month: u32,
    weekday_from_sunday: u32,
}

impl CronLocalTime {
    fn from_offset_datetime(at: OffsetDateTime) -> Self {
        Self {
            minute: u32::from(at.minute()),
            hour: u32::from(at.hour()),
            day_of_month: u32::from(at.day()),
            month: at.month() as u32,
            weekday_from_sunday: u32::from(at.weekday().number_days_from_sunday()),
        }
    }
}

#[derive(Debug)]
struct CronSchedule {
    minutes: CronField,
    hours: CronField,
    days_of_month: CronField,
    months: CronField,
    days_of_week: CronField,
}

impl CronSchedule {
    fn parse(expression: &str) -> Option<Self> {
        let fields = expression.split_whitespace().collect::<Vec<_>>();
        if fields.len() != 5 {
            return None;
        }
        Some(Self {
            minutes: CronField::parse(fields[0], 0, 59, NameSet::None)?,
            hours: CronField::parse(fields[1], 0, 23, NameSet::None)?,
            days_of_month: CronField::parse(fields[2], 1, 31, NameSet::None)?,
            months: CronField::parse(fields[3], 1, 12, NameSet::Month)?,
            days_of_week: CronField::parse(fields[4], 0, 7, NameSet::Weekday)?,
        })
    }

    fn matches(&self, at: CronLocalTime) -> bool {
        let day_of_month_matches = self.days_of_month.matches(at.day_of_month);
        let day_of_week_matches = self.days_of_week.matches(at.weekday_from_sunday);
        let day_matches = if !self.days_of_month.is_any() && !self.days_of_week.is_any() {
            day_of_month_matches || day_of_week_matches
        } else {
            day_of_month_matches && day_of_week_matches
        };
        self.minutes.matches(at.minute)
            && self.hours.matches(at.hour)
            && day_matches
            && self.months.matches(at.month)
    }
}

#[derive(Debug)]
enum CronField {
    Any,
    Values(BTreeSet<u32>),
}

impl CronField {
    fn parse(raw: &str, min: u32, max: u32, names: NameSet) -> Option<Self> {
        if raw == "*" {
            return Some(Self::Any);
        }
        let mut values = BTreeSet::new();
        for part in raw.split(',') {
            parse_cron_part(part.trim(), min, max, names, &mut values)?;
        }
        if values.is_empty() {
            None
        } else {
            Some(Self::Values(values))
        }
    }

    fn matches(&self, value: u32) -> bool {
        match self {
            Self::Any => true,
            Self::Values(values) => values.contains(&value),
        }
    }

    fn is_any(&self) -> bool {
        matches!(self, Self::Any)
    }
}

#[derive(Debug, Clone, Copy)]
enum NameSet {
    None,
    Month,
    Weekday,
}

fn parse_cron_part(
    part: &str,
    min: u32,
    max: u32,
    names: NameSet,
    values: &mut BTreeSet<u32>,
) -> Option<()> {
    if part.is_empty() {
        return None;
    }
    let (range_part, step) = match part.split_once('/') {
        Some((range_part, step)) => {
            let step = step.parse::<u32>().ok()?;
            if step == 0 {
                return None;
            }
            (range_part, step)
        }
        None => (part, 1),
    };
    let (start, end) = if range_part == "*" {
        (min, max)
    } else if let Some((start, end)) = range_part.split_once('-') {
        (
            parse_cron_value(start, min, max, names)?,
            parse_cron_value(end, min, max, names)?,
        )
    } else {
        let value = parse_cron_value(range_part, min, max, names)?;
        (value, value)
    };
    if start > end {
        return None;
    }
    for value in (start..=end).step_by(step as usize) {
        values.insert(normalize_cron_value(value, names));
    }
    Some(())
}

fn parse_cron_value(raw: &str, min: u32, max: u32, names: NameSet) -> Option<u32> {
    let value = match names {
        NameSet::None => None,
        NameSet::Month => month_name_value(raw),
        NameSet::Weekday => weekday_name_value(raw),
    }
    .or_else(|| raw.parse::<u32>().ok())?;
    if (min..=max).contains(&value) {
        Some(value)
    } else {
        None
    }
}

fn normalize_cron_value(value: u32, names: NameSet) -> u32 {
    if matches!(names, NameSet::Weekday) && value == 7 {
        0
    } else {
        value
    }
}

fn month_name_value(raw: &str) -> Option<u32> {
    match raw.to_ascii_uppercase().as_str() {
        "JAN" => Some(1),
        "FEB" => Some(2),
        "MAR" => Some(3),
        "APR" => Some(4),
        "MAY" => Some(5),
        "JUN" => Some(6),
        "JUL" => Some(7),
        "AUG" => Some(8),
        "SEP" => Some(9),
        "OCT" => Some(10),
        "NOV" => Some(11),
        "DEC" => Some(12),
        _ => None,
    }
}

fn weekday_name_value(raw: &str) -> Option<u32> {
    match raw.to_ascii_uppercase().as_str() {
        "SUN" => Some(0),
        "MON" => Some(1),
        "TUE" => Some(2),
        "WED" => Some(3),
        "THU" => Some(4),
        "FRI" => Some(5),
        "SAT" => Some(6),
        _ => None,
    }
}
