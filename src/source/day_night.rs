use crate::settings::{SourceBaseSettings, SourceSettings};
use crate::source::{Source, SourcePowerStateResult};
use chrono::{DateTime, Datelike, Local, NaiveDate, NaiveTime, TimeDelta, TimeZone};
use serde::Deserialize;
use std::error::Error;
use std::fmt::{Debug, Display, Formatter};
use std::sync::Arc;
use std::time::Duration;
use sunrise::{Coordinates, DawnType, SolarDay, SolarEvent};
use tokio::sync::watch;
use tokio::sync::watch::Receiver;
use tokio::time::sleep;

/// Maximum time to sleep before re-evaluating, so that wall clock jumps (NTP sync, suspend, ...)
/// are picked up.
const MAX_SLEEP: Duration = Duration::from_secs(300);

#[derive(Debug)]
pub struct InvalidCoords;

impl Display for InvalidCoords {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str("invalid coordinates")
    }
}

impl Error for InvalidCoords {}

#[derive(Clone, Copy, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct DayNightCoords {
    lat: f64,
    lon: f64,
}

impl TryFrom<DayNightCoords> for Coordinates {
    type Error = InvalidCoords;

    fn try_from(value: DayNightCoords) -> Result<Self, InvalidCoords> {
        Self::new(value.lat, value.lon).ok_or(InvalidCoords)
    }
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct Settings {
    pos: DayNightCoords,
    #[serde(flatten)]
    base: SourceBaseSettings,
}

impl SourceSettings for Settings {
    type Impl = DayNightSource;

    fn base(&self) -> &SourceBaseSettings {
        &self.base
    }

    fn create_source(&self) -> Result<Self::Impl, Box<dyn Error>> {
        DayNightSource::new(self.clone()).map_err(Into::into)
    }
}

pub struct DayNightSource {
    coords: Coordinates,
    base_settings: SourceBaseSettings,
}

impl DayNightSource {
    fn new(settings: Settings) -> Result<Self, InvalidCoords> {
        Ok(Self {
            coords: settings.pos.try_into()?,
            base_settings: settings.base,
        })
    }
}

#[async_trait]
impl Source for DayNightSource {
    fn base_settings(&self) -> &SourceBaseSettings {
        &self.base_settings
    }

    fn create_channel(self: Arc<Self>) -> Receiver<SourcePowerStateResult> {
        let (tx, rx) = watch::channel::<SourcePowerStateResult>(Ok(false));

        tokio::spawn(async move {
            let mut last_day = None;
            loop {
                let now = Local::now();
                let (currently_day, next_change) = state_at(self.coords, &now);
                if last_day != Some(currently_day) {
                    tx.send(Ok(currently_day)).ok();
                    last_day = Some(currently_day);
                }

                let sleep_time = (next_change - now)
                    .to_std()
                    .map_or(MAX_SLEEP, |d| d.min(MAX_SLEEP));
                sleep(sleep_time).await;
            }
        });

        rx
    }
}

/// Whether it is day (between civil dawn and civil dusk) at `now`, and the next point in time
/// at which that changes.
///
/// Events of the surrounding days are considered, since the local date of `now` doesn't need to
/// match the solar day at the coordinates.
fn state_at<Tz: TimeZone>(coords: Coordinates, now: &DateTime<Tz>) -> (bool, DateTime<Tz>) {
    let today = now.date_naive();
    let tz = now.timezone();
    let mut events = (-1..=2)
        .flat_map(|offset| day_events(coords, today + TimeDelta::days(offset), &tz))
        .collect::<Vec<_>>();
    events.sort_by(|(a, _), (b, _)| a.cmp(b));

    let currently_day = events
        .iter()
        .rev()
        .find(|(dt, _)| dt <= now)
        .is_some_and(|(_, is_day)| *is_day);
    let mut upcoming = events.into_iter().filter(|(dt, _)| dt > now);
    let first_upcoming = upcoming.clone().next();
    let next_change = upcoming
        .find(|(_, is_day)| *is_day != currently_day)
        .or(first_upcoming)
        .map(|(dt, _)| dt)
        .unwrap_or_else(|| start_of(today + TimeDelta::days(1), &tz));
    (currently_day, next_change)
}

/// Events of the given date, as pairs of the point in time and whether it is day from then on.
///
/// If civil dawn and dusk don't happen (polar day or night), the whole date is treated as day or
/// night respectively.
fn day_events<Tz: TimeZone>(
    coords: Coordinates,
    date: NaiveDate,
    tz: &Tz,
) -> Vec<(DateTime<Tz>, bool)> {
    let solar_day = SolarDay::new(coords, date);
    let dawn = solar_day.event_time(SolarEvent::Dawn(DawnType::Civil));
    let dusk = solar_day.event_time(SolarEvent::Dusk(DawnType::Civil));
    match (dawn, dusk) {
        (Some(dawn), Some(dusk)) => vec![
            (dawn.with_timezone(tz), true),
            (dusk.with_timezone(tz), false),
        ],
        _ => vec![(start_of(date, tz), is_polar_day(coords, date))],
    }
}

/// Polar day happens around the summer solstice of the respective hemisphere.
fn is_polar_day(coords: Coordinates, date: NaiveDate) -> bool {
    let northern_summer = (4..=9).contains(&date.month());
    (coords.lat() >= 0.0) == northern_summer
}

fn start_of<Tz: TimeZone>(date: NaiveDate, tz: &Tz) -> DateTime<Tz> {
    let midnight = date.and_time(NaiveTime::MIN);
    tz.from_local_datetime(&midnight)
        .earliest()
        .unwrap_or_else(|| tz.from_utc_datetime(&midnight))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono_tz::Europe::Berlin;
    use chrono_tz::Tz;

    const HAMBURG: (f64, f64) = (53.55, 9.99);
    const LONGYEARBYEN: (f64, f64) = (78.22, 15.65);
    const SYDNEY: (f64, f64) = (-33.87, 151.21);

    fn coords((lat, lon): (f64, f64)) -> Coordinates {
        DayNightCoords { lat, lon }.try_into().unwrap()
    }

    fn at(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> DateTime<Tz> {
        Berlin.with_ymd_and_hms(y, mo, d, h, mi, 0).unwrap()
    }

    fn assert_between(dt: &DateTime<Tz>, from: DateTime<Tz>, to: DateTime<Tz>) {
        assert!(*dt > from && *dt < to, "{dt} not between {from} and {to}");
    }

    #[test]
    fn invalid_coords() {
        assert!(
            Coordinates::try_from(DayNightCoords {
                lat: 91.0,
                lon: 0.0
            })
            .is_err()
        );
        assert!(
            Coordinates::try_from(DayNightCoords {
                lat: 0.0,
                lon: 181.0
            })
            .is_err()
        );
    }

    #[test]
    fn winter_day_in_hamburg() {
        let c = coords(HAMBURG);
        // Civil dawn is around 07:50 and civil dusk around 16:45.
        let (day, dawn) = state_at(c, &at(2026, 12, 21, 5, 0));
        assert!(!day, "early morning must be night");
        assert_between(&dawn, at(2026, 12, 21, 7, 30), at(2026, 12, 21, 8, 15));

        let (day, dusk) = state_at(c, &at(2026, 12, 21, 12, 0));
        assert!(day, "noon must be day");
        assert_between(&dusk, at(2026, 12, 21, 16, 15), at(2026, 12, 21, 17, 0));

        let (day, next_dawn) = state_at(c, &at(2026, 12, 21, 20, 0));
        assert!(!day, "evening must be night");
        assert_between(&next_dawn, at(2026, 12, 22, 7, 30), at(2026, 12, 22, 8, 15));
    }

    #[test]
    fn summer_day_in_hamburg() {
        let c = coords(HAMBURG);
        // Civil dawn is around 04:10 and civil dusk around 22:45 (CEST).
        let (day, dawn) = state_at(c, &at(2026, 6, 21, 1, 0));
        assert!(!day);
        assert_between(&dawn, at(2026, 6, 21, 3, 45), at(2026, 6, 21, 4, 30));

        let (day, dusk) = state_at(c, &at(2026, 6, 21, 21, 0));
        assert!(day);
        assert_between(&dusk, at(2026, 6, 21, 22, 30), at(2026, 6, 21, 23, 15));
    }

    #[test]
    fn transitions_are_consistent() {
        let c = coords(HAMBURG);
        let mut now = at(2026, 10, 24, 0, 0);
        let mut last = state_at(c, &now).0;
        assert!(!last);
        // Walk through a week including the DST change, following the reported changes.
        for _ in 0..14 {
            let (_, next) = state_at(c, &now);
            assert!(next > now);
            assert!(next - now < TimeDelta::hours(24));
            let (before, _) = state_at(c, &(next - TimeDelta::seconds(1)));
            let (after, _) = state_at(c, &next);
            assert_eq!(before, last);
            assert_ne!(before, after, "state must change at {next}");
            last = after;
            now = next;
        }
    }

    #[test]
    fn polar_day_and_night() {
        let c = coords(LONGYEARBYEN);
        let (day, next) = state_at(c, &at(2026, 6, 21, 0, 30));
        assert!(day, "midnight sun must be day");
        assert_eq!(next, at(2026, 6, 22, 0, 0));

        let (day, next) = state_at(c, &at(2026, 12, 21, 12, 0));
        assert!(!day, "polar night must be night");
        assert_eq!(next, at(2026, 12, 22, 0, 0));
    }

    #[test]
    fn southern_hemisphere() {
        let c = coords(SYDNEY);
        assert!(is_polar_day(
            coords((-80.0, 0.0)),
            NaiveDate::from_ymd_opt(2026, 12, 21).unwrap()
        ));
        assert!(!is_polar_day(
            coords((-80.0, 0.0)),
            NaiveDate::from_ymd_opt(2026, 6, 21).unwrap()
        ));
        // 12:00 in Sydney is 02:00 in Berlin during the European winter.
        assert!(state_at(c, &at(2026, 12, 21, 2, 0)).0);
        assert!(!state_at(c, &at(2026, 12, 21, 14, 0)).0);
        // 06:00 on the next day in Sydney, after dawn, while it's still the 21st in Berlin.
        let now = at(2026, 12, 21, 20, 0);
        let (day, next) = state_at(c, &now);
        assert!(day);
        assert!(next > now);
        // Civil dusk is around 20:30 in Sydney.
        assert_between(&next, at(2026, 12, 22, 10, 0), at(2026, 12, 22, 11, 0));

        let sydney = chrono_tz::Australia::Sydney;
        let (day, dawn) = state_at(c, &sydney.with_ymd_and_hms(2026, 12, 21, 3, 0, 0).unwrap());
        assert!(!day);
        assert_between(
            &dawn.with_timezone(&Berlin),
            at(2026, 12, 20, 19, 0),
            at(2026, 12, 20, 20, 0),
        );
    }
}
