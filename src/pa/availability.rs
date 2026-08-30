//! Deterministic owner-calendar availability calculation.
//!
//! Availability is evaluated in the owner's IANA timezone while busy
//! intervals and returned starts remain UTC. Local candidate starts are
//! generated on a fifteen-minute grid. Nonexistent local times are skipped;
//! ambiguous local times use their earliest UTC interpretation.

use std::fmt;

use chrono::{
    DateTime, Datelike, Duration as ChronoDuration, LocalResult, NaiveDate, NaiveDateTime,
    NaiveTime as ChronoTime, TimeZone, Timelike, Utc, Weekday,
};
use chrono_tz::Tz;
use time::{Duration, OffsetDateTime, Time, UtcOffset};

/// The result type returned by availability constructors and calculations.
pub type AvailabilityResult<T> = Result<T, AvailabilityError>;

/// Validation failures for availability inputs and policy values.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AvailabilityError {
    /// The interval's end is not strictly after its start.
    InvalidBusyInterval {
        starts_at: OffsetDateTime,
        ends_at: OffsetDateTime,
    },
    /// A local working window is empty or reversed.
    InvalidWorkingWindow { starts_at: Time, ends_at: Time },
    /// The timezone is not a recognized IANA timezone identifier.
    InvalidTimezone { timezone: String },
    /// A policy duration was negative or, where required, zero.
    InvalidDuration { field: &'static str },
    /// A date/time conversion exceeded the supported range.
    DateTimeOverflow,
}

impl fmt::Display for AvailabilityError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidBusyInterval { .. } => {
                formatter.write_str("busy interval end must be after its start")
            }
            Self::InvalidWorkingWindow { .. } => {
                formatter.write_str("working-window end must be after its start")
            }
            Self::InvalidTimezone { timezone } => {
                write!(formatter, "invalid IANA timezone {timezone}")
            }
            Self::InvalidDuration { field } => write!(formatter, "{field} must be non-negative"),
            Self::DateTimeOverflow => {
                formatter.write_str("date/time is outside the supported range")
            }
        }
    }
}

impl std::error::Error for AvailabilityError {}

/// A UTC half-open busy interval `[starts_at, ends_at)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct BusyInterval {
    starts_at: OffsetDateTime,
    ends_at: OffsetDateTime,
}

impl BusyInterval {
    /// Constructs a strictly positive interval and normalizes both endpoints
    /// to UTC.
    pub fn new(starts_at: OffsetDateTime, ends_at: OffsetDateTime) -> AvailabilityResult<Self> {
        let starts_at = starts_at.to_offset(UtcOffset::UTC);
        let ends_at = ends_at.to_offset(UtcOffset::UTC);
        if ends_at <= starts_at {
            return Err(AvailabilityError::InvalidBusyInterval { starts_at, ends_at });
        }
        Ok(Self { starts_at, ends_at })
    }

    /// Alias emphasizing the validating constructor.
    pub fn try_new(starts_at: OffsetDateTime, ends_at: OffsetDateTime) -> AvailabilityResult<Self> {
        Self::new(starts_at, ends_at)
    }

    /// Returns the UTC start (inclusive).
    pub const fn starts_at(&self) -> OffsetDateTime {
        self.starts_at
    }

    /// Returns the UTC end (exclusive).
    pub const fn ends_at(&self) -> OffsetDateTime {
        self.ends_at
    }

    /// Alias for [`Self::starts_at`].
    pub const fn start(&self) -> OffsetDateTime {
        self.starts_at()
    }

    /// Alias for [`Self::ends_at`].
    pub const fn end(&self) -> OffsetDateTime {
        self.ends_at()
    }
}

/// A validated local working window that does not cross midnight.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct WorkingWindow {
    starts_at: Time,
    ends_at: Time,
}

impl WorkingWindow {
    /// Constructs a non-empty local working window.
    pub fn new(starts_at: Time, ends_at: Time) -> AvailabilityResult<Self> {
        if ends_at <= starts_at {
            return Err(AvailabilityError::InvalidWorkingWindow { starts_at, ends_at });
        }
        Ok(Self { starts_at, ends_at })
    }

    /// Constructs a window from hour/minute components.
    pub fn from_hm(
        start_hour: u8,
        start_minute: u8,
        end_hour: u8,
        end_minute: u8,
    ) -> AvailabilityResult<Self> {
        let starts_at = Time::from_hms(start_hour, start_minute, 0).map_err(|_| {
            AvailabilityError::InvalidWorkingWindow {
                starts_at: Time::MIDNIGHT,
                ends_at: Time::MIDNIGHT,
            }
        })?;
        let ends_at = Time::from_hms(end_hour, end_minute, 0).map_err(|_| {
            AvailabilityError::InvalidWorkingWindow {
                starts_at,
                ends_at: Time::MIDNIGHT,
            }
        })?;
        Self::new(starts_at, ends_at)
    }

    /// Returns the local window start (inclusive).
    pub const fn starts_at(&self) -> Time {
        self.starts_at
    }

    /// Returns the local window end (exclusive).
    pub const fn ends_at(&self) -> Time {
        self.ends_at
    }

    /// Alias for [`Self::starts_at`].
    pub const fn start(&self) -> Time {
        self.starts_at()
    }

    /// Alias for [`Self::ends_at`].
    pub const fn end(&self) -> Time {
        self.ends_at()
    }
}

/// A local working schedule indexed by [`Weekday`].
pub type WorkingWindows = Vec<(Weekday, Vec<WorkingWindow>)>;

/// Owner timezone, working hours, and booking constraints.
#[derive(Debug, Clone)]
pub struct AvailabilityPolicy {
    timezone_name: String,
    timezone: Tz,
    windows: [Vec<WorkingWindow>; 7],
    minimum_notice: Duration,
    booking_horizon: Duration,
    meeting_buffer: Duration,
}

impl AvailabilityPolicy {
    /// Constructs a validated policy from per-weekday windows.
    pub fn new<I>(
        timezone: impl Into<String>,
        windows: I,
        minimum_notice: Duration,
        booking_horizon: Duration,
        meeting_buffer: Duration,
    ) -> AvailabilityResult<Self>
    where
        I: IntoIterator<Item = (Weekday, Vec<WorkingWindow>)>,
    {
        let timezone_name = timezone.into();
        let timezone_name = timezone_name.trim().to_owned();
        let timezone =
            timezone_name
                .parse::<Tz>()
                .map_err(|_| AvailabilityError::InvalidTimezone {
                    timezone: timezone_name.clone(),
                })?;
        if minimum_notice.is_negative()
            || booking_horizon <= Duration::ZERO
            || meeting_buffer.is_negative()
        {
            let field = if minimum_notice.is_negative() {
                "minimum_notice"
            } else if booking_horizon <= Duration::ZERO {
                "booking_horizon"
            } else {
                "meeting_buffer"
            };
            return Err(AvailabilityError::InvalidDuration { field });
        }

        let mut indexed: [Vec<WorkingWindow>; 7] = std::array::from_fn(|_| Vec::new());
        for (weekday, mut day_windows) in windows {
            day_windows.sort_by_key(|window| window.starts_at());
            let target = &mut indexed[weekday_index(weekday)];
            target.extend(day_windows);
        }
        for day_windows in &mut indexed {
            day_windows.sort_by_key(|window| window.starts_at());
            if day_windows
                .windows(2)
                .any(|pair| pair[1].starts_at() < pair[0].ends_at())
            {
                return Err(AvailabilityError::InvalidWorkingWindow {
                    starts_at: Time::MIDNIGHT,
                    ends_at: Time::MIDNIGHT,
                });
            }
        }

        Ok(Self {
            timezone_name,
            timezone,
            windows: indexed,
            minimum_notice,
            booking_horizon,
            meeting_buffer,
        })
    }

    /// Constructs a policy with the default Monday-Friday schedule.
    pub fn for_timezone(timezone: impl Into<String>) -> AvailabilityResult<Self> {
        Self::new(
            timezone,
            default_working_windows(),
            Duration::hours(1),
            Duration::days(60),
            Duration::ZERO,
        )
    }

    /// Returns the owner IANA timezone identifier.
    pub fn timezone(&self) -> &str {
        &self.timezone_name
    }

    /// Alias for [`Self::timezone`].
    pub fn owner_timezone(&self) -> &str {
        self.timezone()
    }

    /// Returns the minimum notice interval.
    pub const fn minimum_notice(&self) -> Duration {
        self.minimum_notice
    }

    /// Returns the booking horizon interval.
    pub const fn booking_horizon(&self) -> Duration {
        self.booking_horizon
    }

    /// Returns the buffer required before and after a meeting.
    pub const fn meeting_buffer(&self) -> Duration {
        self.meeting_buffer
    }

    /// Returns working windows for one local weekday.
    pub fn working_windows(&self, weekday: Weekday) -> &[WorkingWindow] {
        &self.windows[weekday_index(weekday)]
    }

    /// Alias for [`Self::working_windows`].
    pub fn windows_for(&self, weekday: Weekday) -> &[WorkingWindow] {
        self.working_windows(weekday)
    }

    /// Returns the earliest UTC starts satisfying this policy.
    pub fn available_slots(
        &self,
        now: OffsetDateTime,
        requested_duration: Duration,
        outlook_busy: &[BusyInterval],
        google_busy: &[BusyInterval],
        limit: usize,
    ) -> AvailabilityResult<Vec<OffsetDateTime>> {
        if requested_duration <= Duration::ZERO {
            return Err(AvailabilityError::InvalidDuration {
                field: "requested_duration",
            });
        }
        if limit == 0 {
            return Ok(Vec::new());
        }

        let now = now.to_offset(UtcOffset::UTC);
        let horizon_end = now
            .checked_add(self.booking_horizon)
            .ok_or(AvailabilityError::DateTimeOverflow)?;
        let lead_time = now
            .checked_add(self.minimum_notice)
            .ok_or(AvailabilityError::DateTimeOverflow)?;
        let now_utc = to_chrono_utc(now)?;
        let horizon_utc = to_chrono_utc(horizon_end)?;
        let lead_utc = to_chrono_utc(lead_time)?;
        let local_now = now_utc.with_timezone(&self.timezone);
        let local_horizon = horizon_utc.with_timezone(&self.timezone);
        let rounded_lead_local =
            ceil_local_grid(lead_utc.with_timezone(&self.timezone).naive_local());
        let busy = merge_busy_intervals(outlook_busy, google_busy);

        let mut date = local_now.date_naive();
        let final_date = local_horizon.date_naive();
        let mut slots = Vec::with_capacity(limit.min(16));
        while date <= final_date {
            let weekday = date.weekday();
            for window in self.working_windows(weekday) {
                let window_start = chrono_time(window.starts_at());
                let window_end = chrono_time(window.ends_at());
                let first_start = ceil_window_grid(date, window_start);
                let Some(mut local_start) = first_start else {
                    continue;
                };
                while local_start < NaiveDateTime::new(date, window_end) {
                    if local_start >= rounded_lead_local
                        && let Some(candidate_start) = local_to_utc(&self.timezone, local_start)
                    {
                        let candidate_start = from_chrono_utc(candidate_start)?;
                        let candidate_end = match candidate_start.checked_add(requested_duration) {
                            Some(end) => end,
                            None => break,
                        };
                        if candidate_start >= lead_time
                            && candidate_end <= horizon_end
                            && candidate_ends_within_working_window(
                                &self.timezone,
                                date,
                                window_end,
                                candidate_end,
                            )?
                            && buffered_is_free(
                                candidate_start,
                                candidate_end,
                                self.meeting_buffer,
                                &busy,
                            )?
                        {
                            slots.push(candidate_start);
                            if slots.len() == limit {
                                return Ok(slots);
                            }
                        }
                    }
                    local_start = match local_start.checked_add_signed(ChronoDuration::minutes(15))
                    {
                        Some(next) => next,
                        None => break,
                    };
                }
            }
            date = match date.succ_opt() {
                Some(next) => next,
                None => break,
            };
        }
        Ok(slots)
    }

    /// Alias for [`Self::available_slots`].
    pub fn find_slots(
        &self,
        now: OffsetDateTime,
        requested_duration: Duration,
        outlook_busy: &[BusyInterval],
        google_busy: &[BusyInterval],
        limit: usize,
    ) -> AvailabilityResult<Vec<OffsetDateTime>> {
        self.available_slots(now, requested_duration, outlook_busy, google_busy, limit)
    }
}

impl Default for AvailabilityPolicy {
    fn default() -> Self {
        Self::for_timezone("UTC").expect("UTC and default availability policy are valid")
    }
}

/// Returns the default Monday-Friday 08:00-18:00 local schedule.
pub fn default_working_windows() -> WorkingWindows {
    let window = WorkingWindow::from_hm(8, 0, 18, 0).expect("default working window is valid");
    vec![
        (Weekday::Mon, vec![window]),
        (Weekday::Tue, vec![window]),
        (Weekday::Wed, vec![window]),
        (Weekday::Thu, vec![window]),
        (Weekday::Fri, vec![window]),
    ]
}

/// Merges overlapping and touching intervals from both calendars into one UTC
/// union. Inputs are already validated by [`BusyInterval::new`].
pub fn merge_busy_intervals(
    outlook_busy: &[BusyInterval],
    google_busy: &[BusyInterval],
) -> Vec<BusyInterval> {
    let mut intervals: Vec<BusyInterval> =
        outlook_busy.iter().chain(google_busy).copied().collect();
    intervals.sort_by_key(BusyInterval::starts_at);
    let mut merged = Vec::with_capacity(intervals.len());
    for interval in intervals {
        let Some(previous) = merged.last_mut() else {
            merged.push(interval);
            continue;
        };
        if interval.starts_at() <= previous.ends_at() {
            if interval.ends_at() > previous.ends_at() {
                previous.ends_at = interval.ends_at();
            }
        } else {
            merged.push(interval);
        }
    }
    merged
}

fn weekday_index(weekday: Weekday) -> usize {
    weekday.num_days_from_monday() as usize
}

fn chrono_time(time: Time) -> ChronoTime {
    ChronoTime::from_hms_nano_opt(
        time.hour() as u32,
        time.minute() as u32,
        time.second() as u32,
        time.nanosecond(),
    )
    .expect("time::Time is representable as chrono::NaiveTime")
}

fn ceil_window_grid(date: NaiveDate, time: ChronoTime) -> Option<NaiveDateTime> {
    let mut minute = time.hour() as i64 * 60 + time.minute() as i64;
    if time.second() != 0 || time.nanosecond() != 0 {
        minute += 1;
    }
    minute = ((minute + 14) / 15) * 15;
    if minute >= 24 * 60 {
        return None;
    }
    let hour = (minute / 60) as u32;
    let minute = (minute % 60) as u32;
    Some(NaiveDateTime::new(
        date,
        ChronoTime::from_hms_opt(hour, minute, 0).expect("grid time is valid"),
    ))
}

fn ceil_local_grid(local: NaiveDateTime) -> NaiveDateTime {
    let date = local.date();
    ceil_window_grid(date, local.time()).unwrap_or_else(|| {
        NaiveDateTime::new(
            date.succ_opt().expect("date is representable"),
            ChronoTime::from_hms_opt(0, 0, 0).expect("midnight is valid"),
        )
    })
}

fn local_to_utc(timezone: &Tz, local: NaiveDateTime) -> Option<DateTime<Utc>> {
    match timezone.from_local_datetime(&local) {
        LocalResult::Single(value) => Some(value.with_timezone(&Utc)),
        LocalResult::Ambiguous(first, second) => {
            let first = first.with_timezone(&Utc);
            let second = second.with_timezone(&Utc);
            Some(first.min(second))
        }
        LocalResult::None => None,
    }
}

fn to_chrono_utc(value: OffsetDateTime) -> AvailabilityResult<DateTime<Utc>> {
    DateTime::<Utc>::from_timestamp(value.unix_timestamp(), value.nanosecond())
        .ok_or(AvailabilityError::DateTimeOverflow)
}

fn from_chrono_utc(value: DateTime<Utc>) -> AvailabilityResult<OffsetDateTime> {
    OffsetDateTime::from_unix_timestamp(value.timestamp())
        .ok()
        .and_then(|base| base.replace_nanosecond(value.timestamp_subsec_nanos()).ok())
        .map(|value| value.to_offset(UtcOffset::UTC))
        .ok_or(AvailabilityError::DateTimeOverflow)
}

fn candidate_ends_within_working_window(
    timezone: &Tz,
    date: NaiveDate,
    window_end: ChronoTime,
    candidate_end: OffsetDateTime,
) -> AvailabilityResult<bool> {
    let candidate_end_utc = to_chrono_utc(candidate_end)?;
    let candidate_end_local = candidate_end_utc.with_timezone(timezone).naive_local();
    if candidate_end_local.date() != date || candidate_end_local.time() > window_end {
        return Ok(false);
    }

    // A repeated local window end has two UTC interpretations. The policy's
    // deterministic rule chooses the earlier one, so an elapsed meeting may
    // not pass that first boundary and re-enter the local time range after a
    // DST fall-back. A nonexistent end has no such UTC boundary; the actual
    // local end check above still excludes candidates that jump beyond it.
    match local_to_utc(timezone, NaiveDateTime::new(date, window_end)) {
        Some(window_end_utc) => Ok(candidate_end_utc <= window_end_utc),
        None => Ok(true),
    }
}

fn buffered_is_free(
    candidate_start: OffsetDateTime,
    candidate_end: OffsetDateTime,
    buffer: Duration,
    busy: &[BusyInterval],
) -> AvailabilityResult<bool> {
    let buffered_start = candidate_start
        .checked_sub(buffer)
        .ok_or(AvailabilityError::DateTimeOverflow)?;
    let buffered_end = candidate_end
        .checked_add(buffer)
        .ok_or(AvailabilityError::DateTimeOverflow)?;
    Ok(busy.iter().all(|interval| {
        buffered_end <= interval.starts_at() || buffered_start >= interval.ends_at()
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Weekday;
    use std::collections::HashSet;
    use time::{Date, Duration, Month, OffsetDateTime, Time, UtcOffset};

    fn utc(day: u8, hour: u8, minute: u8) -> OffsetDateTime {
        OffsetDateTime::new_in_offset(
            Date::from_calendar_date(2024, Month::January, day).expect("date"),
            Time::from_hms(hour, minute, 0).expect("time"),
            UtcOffset::UTC,
        )
    }

    fn utc_timestamp(timestamp: i64) -> OffsetDateTime {
        OffsetDateTime::from_unix_timestamp(timestamp).expect("timestamp")
    }

    fn window(start_hour: u8, start_minute: u8, end_hour: u8, end_minute: u8) -> WorkingWindow {
        WorkingWindow::new(
            Time::from_hms(start_hour, start_minute, 0).expect("start"),
            Time::from_hms(end_hour, end_minute, 0).expect("end"),
        )
        .expect("working window")
    }

    fn policy(
        weekday: Weekday,
        start_hour: u8,
        end_hour: u8,
        notice: Duration,
        horizon: Duration,
        buffer: Duration,
    ) -> AvailabilityPolicy {
        AvailabilityPolicy::new(
            "UTC",
            [(weekday, vec![window(start_hour, 0, end_hour, 0)])],
            notice,
            horizon,
            buffer,
        )
        .expect("availability policy")
    }

    #[test]
    fn defaults_are_weekdays_with_one_hour_notice_and_sixty_day_horizon() {
        let policy = AvailabilityPolicy::default();

        assert_eq!(policy.timezone(), "UTC");
        assert_eq!(policy.minimum_notice(), Duration::hours(1));
        assert_eq!(policy.booking_horizon(), Duration::days(60));
        assert_eq!(policy.meeting_buffer(), Duration::ZERO);
        assert_eq!(policy.working_windows(Weekday::Mon), &[window(8, 0, 18, 0)]);
        assert_eq!(policy.working_windows(Weekday::Fri), &[window(8, 0, 18, 0)]);
        assert!(policy.working_windows(Weekday::Sat).is_empty());
        assert!(policy.working_windows(Weekday::Sun).is_empty());
    }

    #[test]
    fn invalid_iana_timezone_is_rejected() {
        assert!(matches!(
            AvailabilityPolicy::for_timezone("Australia/Not-A-Timezone"),
            Err(AvailabilityError::InvalidTimezone { .. })
        ));
    }

    #[test]
    fn overlapping_working_windows_are_rejected() {
        assert!(matches!(
            AvailabilityPolicy::new(
                "UTC",
                [(
                    Weekday::Mon,
                    vec![window(9, 0, 11, 0), window(10, 0, 12, 0)],
                )],
                Duration::ZERO,
                Duration::hours(1),
                Duration::ZERO,
            ),
            Err(AvailabilityError::InvalidWorkingWindow { .. })
        ));
    }

    #[test]
    fn invalid_policy_durations_are_rejected() {
        for (minimum_notice, booking_horizon, meeting_buffer, field) in [
            (
                Duration::hours(-1),
                Duration::hours(1),
                Duration::ZERO,
                "minimum_notice",
            ),
            (
                Duration::ZERO,
                Duration::ZERO,
                Duration::ZERO,
                "booking_horizon",
            ),
            (
                Duration::ZERO,
                Duration::hours(1),
                Duration::hours(-1),
                "meeting_buffer",
            ),
        ] {
            assert_eq!(
                AvailabilityPolicy::new(
                    "UTC",
                    [],
                    minimum_notice,
                    booking_horizon,
                    meeting_buffer,
                )
                .unwrap_err(),
                AvailabilityError::InvalidDuration { field }
            );
        }
    }

    #[test]
    fn outlook_and_google_busy_intervals_are_one_union() {
        let policy = policy(
            Weekday::Mon,
            9,
            12,
            Duration::ZERO,
            Duration::hours(4),
            Duration::ZERO,
        );
        let outlook = [BusyInterval::new(utc(1, 9, 0), utc(1, 9, 30)).expect("busy")];
        let google = [BusyInterval::new(utc(1, 9, 15), utc(1, 9, 45)).expect("busy")];

        let slots = policy
            .available_slots(utc(1, 8, 0), Duration::minutes(30), &outlook, &google, 2)
            .expect("slots");

        assert_eq!(slots, vec![utc(1, 9, 45), utc(1, 10, 0)]);
    }

    #[test]
    fn overlapping_and_touching_busy_intervals_are_merged() {
        let policy = policy(
            Weekday::Mon,
            9,
            12,
            Duration::ZERO,
            Duration::hours(4),
            Duration::ZERO,
        );
        let outlook = [BusyInterval::new(utc(1, 9, 0), utc(1, 9, 30)).expect("busy")];
        let google = [
            BusyInterval::new(utc(1, 9, 15), utc(1, 9, 45)).expect("busy"),
            BusyInterval::new(utc(1, 9, 45), utc(1, 10, 0)).expect("busy"),
        ];

        assert_eq!(
            merge_busy_intervals(&outlook, &google),
            vec![BusyInterval::new(utc(1, 9, 0), utc(1, 10, 0)).expect("busy")]
        );

        let slots = policy
            .available_slots(utc(1, 8, 0), Duration::minutes(30), &outlook, &google, 1)
            .expect("slots");

        assert_eq!(slots, vec![utc(1, 10, 0)]);
    }

    #[test]
    fn zero_requested_duration_is_rejected() {
        let error = AvailabilityPolicy::default()
            .available_slots(utc(1, 8, 0), Duration::ZERO, &[], &[], 1)
            .unwrap_err();

        assert_eq!(
            error,
            AvailabilityError::InvalidDuration {
                field: "requested_duration"
            }
        );
    }

    #[test]
    fn negative_requested_duration_is_rejected() {
        let error = AvailabilityPolicy::default()
            .available_slots(utc(1, 8, 0), Duration::minutes(-15), &[], &[], 1)
            .unwrap_err();

        assert_eq!(
            error,
            AvailabilityError::InvalidDuration {
                field: "requested_duration"
            }
        );
    }

    #[test]
    fn lead_time_is_rounded_up_to_the_next_quarter_hour() {
        let policy = policy(
            Weekday::Mon,
            9,
            12,
            Duration::hours(1),
            Duration::hours(4),
            Duration::ZERO,
        );

        let slots = policy
            .available_slots(utc(1, 8, 7), Duration::minutes(15), &[], &[], 1)
            .expect("slots");

        assert_eq!(slots, vec![utc(1, 9, 15)]);
    }

    #[test]
    fn candidates_must_finish_before_the_booking_horizon() {
        let policy = policy(
            Weekday::Mon,
            8,
            12,
            Duration::ZERO,
            Duration::hours(1),
            Duration::ZERO,
        );

        let slots = policy
            .available_slots(utc(1, 8, 0), Duration::minutes(30), &[], &[], 10)
            .expect("slots");

        assert_eq!(slots, vec![utc(1, 8, 0), utc(1, 8, 15), utc(1, 8, 30)]);
    }

    #[test]
    fn candidates_never_cross_a_working_window_boundary() {
        let policy = policy(
            Weekday::Mon,
            9,
            10,
            Duration::ZERO,
            Duration::hours(4),
            Duration::ZERO,
        );

        let slots = policy
            .available_slots(utc(1, 8, 0), Duration::minutes(30), &[], &[], 10)
            .expect("slots");

        assert_eq!(slots, vec![utc(1, 9, 0), utc(1, 9, 15), utc(1, 9, 30)]);
    }

    #[test]
    fn meeting_buffer_must_also_be_free() {
        let policy = policy(
            Weekday::Mon,
            9,
            11,
            Duration::ZERO,
            Duration::hours(4),
            Duration::minutes(15),
        );
        let busy = [BusyInterval::new(utc(1, 10, 0), utc(1, 10, 15)).expect("busy")];

        let slots = policy
            .available_slots(utc(1, 8, 0), Duration::minutes(30), &busy, &[], 10)
            .expect("slots");

        assert!(!slots.contains(&utc(1, 9, 30)));
        assert!(slots.contains(&utc(1, 9, 0)));
    }

    #[test]
    fn result_limit_returns_only_the_earliest_slots() {
        let policy = policy(
            Weekday::Mon,
            9,
            12,
            Duration::ZERO,
            Duration::hours(4),
            Duration::ZERO,
        );

        let slots = policy
            .available_slots(utc(1, 8, 0), Duration::minutes(15), &[], &[], 2)
            .expect("slots");

        assert_eq!(slots, vec![utc(1, 9, 0), utc(1, 9, 15)]);
    }

    #[test]
    fn sydney_spring_dst_gap_is_skipped() {
        let policy = AvailabilityPolicy::new(
            "Australia/Sydney",
            [(Weekday::Sun, vec![window(1, 0, 4, 0)])],
            Duration::ZERO,
            Duration::days(2),
            Duration::ZERO,
        )
        .expect("policy");
        let now = OffsetDateTime::from_unix_timestamp(1_728_140_400).expect("timestamp");

        let slots = policy
            .available_slots(now, Duration::minutes(15), &[], &[], 20)
            .expect("slots");
        let unique_utc_starts: HashSet<_> =
            slots.iter().map(|slot| slot.unix_timestamp()).collect();
        assert_eq!(unique_utc_starts.len(), slots.len());
        assert_eq!(
            slots,
            vec![
                utc_timestamp(1_728_140_400),
                utc_timestamp(1_728_141_300),
                utc_timestamp(1_728_142_200),
                utc_timestamp(1_728_143_100),
                utc_timestamp(1_728_144_000),
                utc_timestamp(1_728_144_900),
                utc_timestamp(1_728_145_800),
                utc_timestamp(1_728_146_700),
            ]
        );
        let timezone: chrono_tz::Tz = "Australia/Sydney".parse().expect("timezone");
        let local_hours: Vec<_> = slots
            .iter()
            .map(|slot| {
                chrono::DateTime::<chrono::Utc>::from_timestamp(
                    slot.unix_timestamp(),
                    slot.nanosecond(),
                )
                .expect("timestamp")
                .with_timezone(&timezone)
                .hour()
            })
            .collect();
        assert!(local_hours.iter().all(|hour| *hour != 2));
        assert!(local_hours.contains(&3));
    }

    #[test]
    fn sydney_spring_dst_duration_must_not_cross_the_local_work_boundary() {
        let policy = AvailabilityPolicy::new(
            "Australia/Sydney",
            [(Weekday::Sun, vec![window(1, 0, 3, 0)])],
            Duration::ZERO,
            Duration::hours(4),
            Duration::ZERO,
        )
        .expect("policy");
        let now = OffsetDateTime::new_in_offset(
            Date::from_calendar_date(2024, Month::October, 5).expect("date"),
            Time::from_hms(14, 30, 0).expect("time"),
            UtcOffset::UTC,
        );

        let slots = policy
            .available_slots(now, Duration::minutes(30), &[], &[], 10)
            .expect("slots");

        let unique_utc_starts: HashSet<_> =
            slots.iter().map(|slot| slot.unix_timestamp()).collect();
        assert_eq!(unique_utc_starts.len(), slots.len());

        assert_eq!(
            slots,
            vec![
                OffsetDateTime::new_in_offset(
                    Date::from_calendar_date(2024, Month::October, 5).expect("date"),
                    Time::from_hms(15, 0, 0).expect("time"),
                    UtcOffset::UTC,
                ),
                OffsetDateTime::new_in_offset(
                    Date::from_calendar_date(2024, Month::October, 5).expect("date"),
                    Time::from_hms(15, 15, 0).expect("time"),
                    UtcOffset::UTC,
                ),
                OffsetDateTime::new_in_offset(
                    Date::from_calendar_date(2024, Month::October, 5).expect("date"),
                    Time::from_hms(15, 30, 0).expect("time"),
                    UtcOffset::UTC,
                ),
            ]
        );
    }

    #[test]
    fn sydney_fall_dst_ambiguity_chooses_earliest_utc_instant() {
        let policy = AvailabilityPolicy::new(
            "Australia/Sydney",
            [(Weekday::Sun, vec![window(1, 0, 4, 0)])],
            Duration::ZERO,
            Duration::days(2),
            Duration::ZERO,
        )
        .expect("policy");
        let now = OffsetDateTime::from_unix_timestamp(1_712_412_000).expect("timestamp");

        let slots = policy
            .available_slots(now, Duration::minutes(15), &[], &[], 5)
            .expect("slots");

        assert_eq!(slots[4].unix_timestamp(), 1_712_415_600);
    }

    #[test]
    fn reversed_busy_intervals_are_rejected() {
        assert!(BusyInterval::new(utc(1, 10, 0), utc(1, 9, 0)).is_err());
        assert!(BusyInterval::new(utc(1, 9, 0), utc(1, 9, 0)).is_err());
    }
}
