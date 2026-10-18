use crate::settings::{SourceBaseSettings, SourceSettings};
use crate::source::{Source, SourcePowerStateResult};
use anyhow::anyhow;
use chrono::{DateTime, Datelike, Local, NaiveDate, NaiveTime, TimeDelta, TimeZone, Weekday};
use serde::Deserialize;
use std::error::Error;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::watch;
use tokio::sync::watch::Receiver;
use tokio::time::sleep;

/// Prefix of a [`TimeCondition`] that refers to the day after the configured weekday.
const NEXT_DAY_PREFIX: &str = "nextday@";
/// How many days to look back and ahead when searching for events. Must be larger than a week
/// plus the largest day offset of a [`TimeCondition`].
const SEARCH_WINDOW_DAYS: i64 = 9;
/// Maximum time to sleep before re-evaluating the timer, so that wall clock jumps (NTP sync,
/// suspend, ...) are picked up.
const MAX_SLEEP: Duration = Duration::from_secs(300);

/// A point in time relative to a weekday, in the format `HH:MM`, `HH:MM:SS`, `nextday@HH:MM` or
/// `nextday@HH:MM:SS`.
#[derive(Clone, Copy, Debug, PartialEq, Deserialize)]
#[serde(try_from = "String")]
pub struct TimeCondition {
    day_offset: u8,
    time: NaiveTime,
}

impl TryFrom<String> for TimeCondition {
    type Error = anyhow::Error;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        let (day_offset, time_str) = match value.strip_prefix(NEXT_DAY_PREFIX) {
            Some(rest) => (1, rest),
            None => (0, value.as_str()),
        };
        let time = NaiveTime::parse_from_str(time_str, "%H:%M:%S")
            .or_else(|_| NaiveTime::parse_from_str(time_str, "%H:%M"))
            .map_err(|e| anyhow!("invalid timer time '{value}': {e}"))?;
        Ok(Self { day_offset, time })
    }
}

impl TimeCondition {
    /// The point in time this condition refers to, for the given date of the weekday it is
    /// configured for.
    ///
    /// Local times that don't exist (because the clock is moved forward for DST) are moved to the
    /// first valid time after it. Ambiguous local times (clock moved backwards) resolve to the
    /// earlier one.
    fn datetime<Tz: TimeZone>(&self, base_date: NaiveDate, tz: &Tz) -> DateTime<Tz> {
        let naive = (base_date + TimeDelta::days(self.day_offset as i64)).and_time(self.time);
        // DST gaps are at most a few hours.
        (0..=24 * 60)
            .find_map(|minutes| {
                tz.from_local_datetime(&(naive + TimeDelta::minutes(minutes)))
                    .earliest()
            })
            .unwrap_or_else(|| tz.from_utc_datetime(&naive))
    }
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct Days {
    #[serde(default)]
    mon: Option<TimeCondition>,
    #[serde(default)]
    tue: Option<TimeCondition>,
    #[serde(default)]
    wed: Option<TimeCondition>,
    #[serde(default)]
    thu: Option<TimeCondition>,
    #[serde(default)]
    fri: Option<TimeCondition>,
    #[serde(default)]
    sat: Option<TimeCondition>,
    #[serde(default)]
    sun: Option<TimeCondition>,
}

impl Days {
    fn get(&self, weekday: Weekday) -> Option<&TimeCondition> {
        match weekday {
            Weekday::Mon => self.mon.as_ref(),
            Weekday::Tue => self.tue.as_ref(),
            Weekday::Wed => self.wed.as_ref(),
            Weekday::Thu => self.thu.as_ref(),
            Weekday::Fri => self.fri.as_ref(),
            Weekday::Sat => self.sat.as_ref(),
            Weekday::Sun => self.sun.as_ref(),
        }
    }

    fn is_empty(&self) -> bool {
        [
            Weekday::Mon,
            Weekday::Tue,
            Weekday::Wed,
            Weekday::Thu,
            Weekday::Fri,
            Weekday::Sat,
            Weekday::Sun,
        ]
        .into_iter()
        .all(|wd| self.get(wd).is_none())
    }

    /// All events within [`SEARCH_WINDOW_DAYS`] around `now`.
    fn events_around<Tz: TimeZone>(
        &self,
        now: &DateTime<Tz>,
    ) -> impl Iterator<Item = DateTime<Tz>> {
        let today = now.date_naive();
        let tz = now.timezone();
        (-SEARCH_WINDOW_DAYS..=SEARCH_WINDOW_DAYS).filter_map(move |offset| {
            let date = today + TimeDelta::days(offset);
            self.get(date.weekday()).map(|tc| tc.datetime(date, &tz))
        })
    }

    /// The latest event that happened at or before `now`.
    fn last_at_or_before<Tz: TimeZone>(&self, now: &DateTime<Tz>) -> Option<DateTime<Tz>> {
        self.events_around(now).filter(|dt| dt <= now).max()
    }

    /// The earliest event that happens after `now`.
    fn next_after<Tz: TimeZone>(&self, now: &DateTime<Tz>) -> Option<DateTime<Tz>> {
        self.events_around(now).filter(|dt| dt > now).min()
    }
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct Settings {
    start: Days,
    end: Days,
    #[serde(flatten)]
    base: SourceBaseSettings,
}

impl Settings {
    /// The timer is active if the most recent start is later than the most recent end.
    fn is_active<Tz: TimeZone>(&self, now: &DateTime<Tz>) -> bool {
        match (
            self.start.last_at_or_before(now),
            self.end.last_at_or_before(now),
        ) {
            (Some(start), Some(end)) => start > end,
            (Some(_), None) => true,
            (None, _) => false,
        }
    }

    /// The next point in time after `now` at which the timer may change its state.
    fn next_change<Tz: TimeZone>(&self, now: &DateTime<Tz>) -> Option<DateTime<Tz>> {
        [self.start.next_after(now), self.end.next_after(now)]
            .into_iter()
            .flatten()
            .min()
    }
}

impl SourceSettings for Settings {
    type Impl = TimerSource;

    fn base(&self) -> &SourceBaseSettings {
        &self.base
    }

    fn create_source(&self) -> Result<Self::Impl, Box<dyn Error>> {
        TimerSource::new(self.clone()).map_err(Into::into)
    }
}

pub struct TimerSource {
    settings: Settings,
}

impl TimerSource {
    fn new(settings: Settings) -> Result<Self, anyhow::Error> {
        if settings.start.is_empty() || settings.end.is_empty() {
            return Err(anyhow!(
                "timer needs at least one day configured for both start and end"
            ));
        }
        Ok(Self { settings })
    }
}

#[async_trait]
impl Source for TimerSource {
    fn base_settings(&self) -> &SourceBaseSettings {
        self.settings.base()
    }

    fn create_channel(self: Arc<Self>) -> Receiver<SourcePowerStateResult> {
        let (tx, rx) = watch::channel::<SourcePowerStateResult>(Ok(false));
        tokio::spawn(async move {
            let mut last_active = None;
            loop {
                let now = Local::now();
                let active = self.settings.is_active(&now);
                if last_active != Some(active) {
                    tx.send(Ok(active)).unwrap();
                    last_active = Some(active);
                }

                let sleep_time = self
                    .settings
                    .next_change(&now)
                    .and_then(|next| (next - now).to_std().ok())
                    .map_or(MAX_SLEEP, |d| d.min(MAX_SLEEP));
                sleep(sleep_time).await;
            }
        });
        rx
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono_tz::Europe::Berlin;

    fn tc(s: &str) -> TimeCondition {
        TimeCondition::try_from(s.to_string()).unwrap()
    }

    fn at(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> DateTime<chrono_tz::Tz> {
        Berlin.with_ymd_and_hms(y, mo, d, h, mi, 0).unwrap()
    }

    fn all_days(s: &str) -> Days {
        Days {
            mon: Some(tc(s)),
            tue: Some(tc(s)),
            wed: Some(tc(s)),
            thu: Some(tc(s)),
            fri: Some(tc(s)),
            sat: Some(tc(s)),
            sun: Some(tc(s)),
        }
    }

    fn no_days() -> Days {
        Days {
            mon: None,
            tue: None,
            wed: None,
            thu: None,
            fri: None,
            sat: None,
            sun: None,
        }
    }

    fn settings(start: Days, end: Days) -> Settings {
        Settings {
            start,
            end,
            base: SourceBaseSettings {
                name: "test".to_string(),
                enable: true,
            },
        }
    }

    /// The "Awake" timer from the example config: weekdays 05:00-23:23, weekends 08:00 to
    /// 01:30 the next day.
    fn awake() -> Settings {
        let mut start = all_days("05:00");
        start.sat = Some(tc("08:00"));
        start.sun = Some(tc("08:00"));
        let mut end = all_days("23:23");
        end.sat = Some(tc("nextday@01:30"));
        end.sun = Some(tc("nextday@01:30"));
        settings(start, end)
    }

    #[test]
    fn parse_time_condition() {
        assert_eq!(
            tc("05:30"),
            TimeCondition {
                day_offset: 0,
                time: NaiveTime::from_hms_opt(5, 30, 0).unwrap()
            }
        );
        assert_eq!(
            tc("23:59:58"),
            TimeCondition {
                day_offset: 0,
                time: NaiveTime::from_hms_opt(23, 59, 58).unwrap()
            }
        );
        assert_eq!(
            tc("nextday@01:30"),
            TimeCondition {
                day_offset: 1,
                time: NaiveTime::from_hms_opt(1, 30, 0).unwrap()
            }
        );
    }

    #[test]
    fn parse_time_condition_invalid() {
        for s in [
            "",
            "5",
            "25:00",
            "12:60",
            "tomorrow@01:00",
            "nextday@",
            "12:00 ",
        ] {
            assert!(
                TimeCondition::try_from(s.to_string()).is_err(),
                "'{s}' should not parse"
            );
        }
    }

    fn from_toml(s: &str) -> Result<Settings, config::ConfigError> {
        config::Config::builder()
            .add_source(config::File::from_str(s, config::FileFormat::Toml))
            .build()?
            .try_deserialize()
    }

    #[test]
    fn deserialize_settings() {
        let s = from_toml(
            r#"
            name = "Awake"
            enable = true
            start = {
                mon = "05:00", tue = "05:00", wed = "05:00", thu = "05:00", fri = "05:00",
                sat = "08:00", sun = "08:00"
            }
            end = {
                mon = "23:23", tue = "23:23", wed = "23:23", thu = "23:23", fri = "23:23",
                sat = "nextday@01:30", sun = "nextday@01:30"
            }
            "#,
        )
        .unwrap();
        assert_eq!((&s.start, &s.end), (&awake().start, &awake().end));

        let s = from_toml(
            r#"
            name = "Sparse"
            enable = true
            start = { mon = "20:00" }
            end = { tue = "02:00" }
            "#,
        )
        .unwrap();
        assert_eq!(s.start.mon, Some(tc("20:00")));
        assert_eq!(s.start.tue, None);

        assert!(
            from_toml(
                r#"
                name = "Invalid"
                enable = true
                start = { mon = "nope" }
                end = { mon = "02:00" }
                "#
            )
            .is_err(),
            "invalid time must fail deserialization"
        );
    }

    #[test]
    fn datetime_same_and_next_day() {
        let date = NaiveDate::from_ymd_opt(2026, 10, 17).unwrap();
        assert_eq!(tc("08:00").datetime(date, &Berlin), at(2026, 10, 17, 8, 0));
        assert_eq!(
            tc("nextday@01:30").datetime(date, &Berlin),
            at(2026, 10, 18, 1, 30)
        );
        // Month and year boundaries.
        let date = NaiveDate::from_ymd_opt(2026, 12, 31).unwrap();
        assert_eq!(
            tc("nextday@00:15").datetime(date, &Berlin),
            at(2027, 1, 1, 0, 15)
        );
    }

    #[test]
    fn datetime_dst_gap() {
        // 2026-03-29: clocks jump from 02:00 to 03:00 in Berlin.
        let date = NaiveDate::from_ymd_opt(2026, 3, 29).unwrap();
        assert_eq!(tc("02:30").datetime(date, &Berlin), at(2026, 3, 29, 3, 0));
    }

    #[test]
    fn datetime_dst_ambiguous() {
        // 2026-10-25: clocks go back from 03:00 to 02:00 in Berlin; 02:30 happens twice.
        let date = NaiveDate::from_ymd_opt(2026, 10, 25).unwrap();
        let dt = tc("02:30").datetime(date, &Berlin);
        assert_eq!(dt.to_utc().to_rfc3339(), "2026-10-25T00:30:00+00:00");
    }

    #[test]
    fn awake_weekday() {
        let s = awake();
        // 2026-10-14 is a Wednesday.
        assert!(!s.is_active(&at(2026, 10, 14, 4, 59)));
        assert!(s.is_active(&at(2026, 10, 14, 5, 0)));
        assert!(s.is_active(&at(2026, 10, 14, 12, 0)));
        assert!(s.is_active(&at(2026, 10, 14, 23, 22)));
        assert!(!s.is_active(&at(2026, 10, 14, 23, 23)));
        assert!(!s.is_active(&at(2026, 10, 14, 23, 59)));
    }

    #[test]
    fn awake_overnight_weekend() {
        let s = awake();
        // Saturday 2026-10-17 08:00 until Sunday 01:30.
        assert!(!s.is_active(&at(2026, 10, 17, 7, 59)));
        assert!(s.is_active(&at(2026, 10, 17, 8, 0)));
        assert!(s.is_active(&at(2026, 10, 17, 23, 59)));
        assert!(s.is_active(&at(2026, 10, 18, 0, 30)));
        assert!(s.is_active(&at(2026, 10, 18, 1, 29)));
        assert!(!s.is_active(&at(2026, 10, 18, 1, 30)));
        assert!(!s.is_active(&at(2026, 10, 18, 7, 0)));
        // Sunday's window runs into Monday morning.
        assert!(s.is_active(&at(2026, 10, 19, 1, 0)));
        assert!(!s.is_active(&at(2026, 10, 19, 2, 0)));
        assert!(s.is_active(&at(2026, 10, 19, 5, 0)));
    }

    #[test]
    fn next_change() {
        let s = awake();
        // Wednesday noon -> end at 23:23.
        assert_eq!(
            s.next_change(&at(2026, 10, 14, 12, 0)),
            Some(at(2026, 10, 14, 23, 23))
        );
        // Wednesday night -> start on Thursday 05:00.
        assert_eq!(
            s.next_change(&at(2026, 10, 14, 23, 30)),
            Some(at(2026, 10, 15, 5, 0))
        );
        // Exactly at an event, the next one is returned.
        assert_eq!(
            s.next_change(&at(2026, 10, 14, 5, 0)),
            Some(at(2026, 10, 14, 23, 23))
        );
        // Saturday night -> end on Sunday 01:30.
        assert_eq!(
            s.next_change(&at(2026, 10, 17, 23, 0)),
            Some(at(2026, 10, 18, 1, 30))
        );
    }

    #[test]
    fn sparse_days() {
        // Only active from Monday 20:00 to Tuesday 02:00 (configured on Tuesday).
        let mut start = no_days();
        start.mon = Some(tc("20:00"));
        let mut end = no_days();
        end.tue = Some(tc("02:00"));
        let s = settings(start, end);

        // 2026-10-12 is a Monday.
        assert!(!s.is_active(&at(2026, 10, 12, 19, 59)));
        assert!(s.is_active(&at(2026, 10, 12, 20, 0)));
        assert!(s.is_active(&at(2026, 10, 13, 1, 59)));
        assert!(!s.is_active(&at(2026, 10, 13, 2, 0)));
        assert!(!s.is_active(&at(2026, 10, 16, 12, 0)));
        assert_eq!(
            s.next_change(&at(2026, 10, 13, 2, 0)),
            Some(at(2026, 10, 19, 20, 0))
        );
    }

    #[test]
    fn across_dst_change() {
        let s = settings(all_days("22:00"), all_days("nextday@06:00"));
        // Night of 2026-10-24 to 2026-10-25 is one hour longer.
        assert!(s.is_active(&at(2026, 10, 24, 23, 0)));
        assert!(s.is_active(&at(2026, 10, 25, 5, 59)));
        assert!(!s.is_active(&at(2026, 10, 25, 6, 0)));
        let start = at(2026, 10, 24, 22, 0);
        let end = s.next_change(&start).unwrap();
        assert_eq!(end, at(2026, 10, 25, 6, 0));
        assert_eq!(end - start, TimeDelta::hours(9));
    }

    #[test]
    fn same_start_and_end_is_inactive() {
        let s = settings(all_days("12:00"), all_days("12:00"));
        assert!(!s.is_active(&at(2026, 10, 14, 11, 0)));
        assert!(!s.is_active(&at(2026, 10, 14, 12, 0)));
        assert!(!s.is_active(&at(2026, 10, 14, 13, 0)));
    }

    #[test]
    fn empty_days_rejected() {
        assert!(TimerSource::new(settings(no_days(), all_days("12:00"))).is_err());
        assert!(TimerSource::new(settings(all_days("12:00"), no_days())).is_err());
        assert!(TimerSource::new(awake()).is_ok());
    }
}
