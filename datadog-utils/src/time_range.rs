use anyhow::{anyhow, Context};
use chrono::{DateTime, Duration, NaiveDate, NaiveDateTime, SubsecRound, TimeZone, Utc};
use once_cell::sync::Lazy;
use regex::Regex;
use std::str::FromStr;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TimeRange {
    pub from: DateTime<Utc>,
    pub to: DateTime<Utc>,
}

static DATE_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r##""date":"([^"]+)""##).unwrap());

impl TimeRange {
    /// Given a from / to, return a list of S3 keys that should be iterated over to get all logs.
    /// This is inclusive to the hour digit of [from, to].
    ///
    /// For example, if 'from' is 2:30am and 'to' is 5:01am, then the keys would look something like:
    /// ['hour=02', 'hour=03', 'hour=04', 'hour=05']
    pub fn folder_keys(&self, archive_path: &str) -> Vec<(String, NaiveDate)> {
        let format_to_datadog_bucket = |ts: DateTime<Utc>| {
            ts.format(&format!("{archive_path}/dt=%Y%m%d/hour=%H/"))
                .to_string()
        };
        let mut current = self.from;

        let to = self.to.round_subsecs(0) + Duration::hours(1);
        let mut folder_keys = vec![];
        while current.round_subsecs(0) < to {
            folder_keys.push((format_to_datadog_bucket(current), current.date_naive()));
            current += Duration::hours(1);
        }
        // We want to return the later folders first. This helps us start finding logs
        // for time ranges like "last 1 hour" quickly.
        //
        // Otherwise we'd start from searching buckets from 2 hours ago, then 1 hours ago, etc.
        folder_keys.reverse();
        folder_keys
    }

    pub fn new(from: DateTime<Utc>, to: DateTime<Utc>) -> Self {
        assert!(
            from < to,
            "Can't create TimeRange with from ({:?}) < to {:?}",
            from,
            to
        );
        Self { from, to }
    }

    pub fn overlaps(&self, other: &Self) -> bool {
        self.to >= other.from && other.to >= self.from
    }

    pub fn contains_log_line(&self, log_line: &str) -> bool {
        let date_time = Self::parse_log_line_date_time(log_line);
        self.from <= date_time && date_time <= self.to
    }

    fn parse_log_line_date_time(log_line: &str) -> DateTime<Utc> {
        if let Some(date_captures) = DATE_RE.captures(log_line) {
            let raw_date = date_captures.get(1).expect("captured").as_str();
            let naive_date = match NaiveDateTime::parse_from_str(raw_date, "%Y-%m-%dT%H:%M:%S.%3fZ")
            {
                Err(_) => panic!(
                    "Failed to parse date, slice: {}, full line: {}",
                    raw_date, log_line
                ),
                Ok(naive_date) => naive_date,
            };
            Utc.from_utc_datetime(&naive_date)
        } else {
            panic!("Failed to find date in log line: {}", log_line);
        }
    }
}

static FROM_TS: Lazy<Regex> = Lazy::new(|| Regex::new(r"from_ts=(\d+)").unwrap());
static TO_TS: Lazy<Regex> = Lazy::new(|| Regex::new(r"to_ts=(\d+)").unwrap());

static LAST_TIME_DURATION: Lazy<Regex> = Lazy::new(|| Regex::new(r"last\s*(\d*)\s*(\w+)").unwrap());

impl FromStr for TimeRange {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if let (Some(from_captures), Some(to_captures)) = (FROM_TS.captures(s), TO_TS.captures(s)) {
            let parse_ts_millis = |captures: regex::Captures| -> Result<i64, anyhow::Error> {
                let ts_millis_str = captures.get(1).expect("captured").as_str();
                let ts_millis = ts_millis_str.parse().with_context(|| {
                    format!(
                        "Failed to parse '{}' as an integer from Datadog link!",
                        ts_millis_str
                    )
                })?;
                Ok(ts_millis)
            };
            let from_ts_millis = parse_ts_millis(from_captures)?;
            let to_ts_millis = parse_ts_millis(to_captures)?;
            return Ok(Self::new(
                Utc.timestamp_millis_opt(from_ts_millis).unwrap(),
                Utc.timestamp_millis_opt(to_ts_millis).unwrap(),
            ));
        }

        if let Some(last_time_captures) = LAST_TIME_DURATION.captures(s) {
            let count_str = last_time_captures.get(1).expect("exists").as_str();
            let count = if count_str.is_empty() {
                1
            } else {
                count_str
                    .parse()
                    .with_context(|| format!("Failed to parse '{}' as an integer!", count_str))?
            };

            let unit_str = last_time_captures.get(2).expect("exists").as_str();
            let unit = TimeUnit::from_str(unit_str)?;
            return Ok(Self::new(
                Utc::now() - unit.into_duration(count as i64),
                Utc::now(),
            ));
        }

        // Try ISO 8601 absolute range: "2026-02-19T17:35:00Z to 2026-02-19T23:00:00Z"
        if let Some((from_str, to_str)) = s.split_once(" to ") {
            if let (Ok(from_dt), Ok(to_dt)) = (
                DateTime::parse_from_rfc3339(from_str.trim()),
                DateTime::parse_from_rfc3339(to_str.trim()),
            ) {
                return Ok(Self::new(from_dt.with_timezone(&Utc), to_dt.with_timezone(&Utc)));
            }
        }

        Err(anyhow!(
            "Failed to parse '{}' as a time range! Examples: 'last 4 hours', a Datadog URL, or 'ISO8601 to ISO8601'.",
            s
        ))
    }
}

enum TimeUnit {
    Minute,
    Hour,
    Day,
    Week,
}

static MINUTE_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"(m|(min|minute)s*)").unwrap());
static HOUR_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"(h|(hr|hour)s*)").unwrap());
static DAY_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"(d|days*)").unwrap());
static WEEK_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"(w|weeks*)").unwrap());

impl TimeUnit {
    fn into_duration(self, count: i64) -> Duration {
        match self {
            TimeUnit::Minute => Duration::minutes(count),
            TimeUnit::Hour => Duration::hours(count),
            TimeUnit::Day => Duration::days(count),
            TimeUnit::Week => Duration::weeks(count),
        }
    }
}

impl FromStr for TimeUnit {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if MINUTE_RE.is_match(s) {
            Ok(TimeUnit::Minute)
        } else if HOUR_RE.is_match(s) {
            Ok(TimeUnit::Hour)
        } else if DAY_RE.is_match(s) {
            Ok(TimeUnit::Day)
        } else if WEEK_RE.is_match(s) {
            Ok(TimeUnit::Week)
        } else {
            Err(anyhow!("Could not parse a valid time unit from '{}' (valid units are: mins, hours, days, weeks)", s))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    impl TimeRange {
        fn now_minus_duration(duration: Duration) -> Self {
            Self {
                from: Utc::now() - duration,
                to: Utc::now(),
            }
        }

        fn rounded_to_sec(self) -> Self {
            Self {
                from: self.from.round_subsecs(0),
                to: self.to.round_subsecs(0),
            }
        }
    }

    macro_rules! approx_eq {
        ($time_range_a:expr, $time_range_b:expr$(,)*) => {
            assert_eq!(
                $time_range_a.rounded_to_sec(),
                $time_range_b.rounded_to_sec(),
            )
        };
    }

    #[test]
    fn works_for_past_tense() {
        approx_eq!(
            TimeRange::from_str("last 30 minutes").unwrap(),
            TimeRange::now_minus_duration(Duration::minutes(30)),
        );
        approx_eq!(
            TimeRange::from_str("last 30 mins").unwrap(),
            TimeRange::now_minus_duration(Duration::minutes(30)),
        );
        approx_eq!(
            TimeRange::from_str("last 30 min").unwrap(),
            TimeRange::now_minus_duration(Duration::minutes(30)),
        );
        approx_eq!(
            TimeRange::from_str("last 30 minute").unwrap(),
            TimeRange::now_minus_duration(Duration::minutes(30)),
        );
        approx_eq!(
            TimeRange::from_str("last 30m").unwrap(),
            TimeRange::now_minus_duration(Duration::minutes(30)),
        );
        approx_eq!(
            TimeRange::from_str("last 30hr").unwrap(),
            TimeRange::now_minus_duration(Duration::hours(30)),
        );
        approx_eq!(
            TimeRange::from_str("last 30hrs").unwrap(),
            TimeRange::now_minus_duration(Duration::hours(30)),
        );
        approx_eq!(
            TimeRange::from_str("last 30 hours").unwrap(),
            TimeRange::now_minus_duration(Duration::hours(30)),
        );
        approx_eq!(
            TimeRange::from_str("last 1 hour").unwrap(),
            TimeRange::now_minus_duration(Duration::hours(1)),
        );
        approx_eq!(
            TimeRange::from_str("last 1h").unwrap(),
            TimeRange::now_minus_duration(Duration::hours(1)),
        );
        approx_eq!(
            TimeRange::from_str("last hour").unwrap(),
            TimeRange::now_minus_duration(Duration::hours(1)),
        );
    }

    #[test]
    fn works_for_datadog_urls() {
        assert_eq!(
            TimeRange::from_str("https://app.datadoghq.com/dashboard/ajr-z7z-6s2/multiplayer?from_ts=1605055459837&live=true&to_ts=1605228259837").unwrap(),
            TimeRange {
                from: Utc.timestamp_millis_opt(1605055459837).unwrap(),
                to: Utc.timestamp_millis_opt(1605228259837).unwrap(),
            }
        );
    }

    #[test]
    fn overlap_works() {
        let base_time = Utc::now();
        let range = TimeRange::new(base_time - Duration::hours(1), base_time);
        assert!(range.overlaps(&TimeRange::new(range.from - Duration::hours(1), range.from)));
        assert!(!range.overlaps(&TimeRange::new(
            range.from - Duration::hours(1),
            range.from - Duration::minutes(1),
        )));

        assert!(range.overlaps(&TimeRange::new(range.to, range.to + Duration::hours(1))));
        assert!(!range.overlaps(&TimeRange::new(
            range.to + Duration::minutes(1),
            range.to + Duration::hours(1),
        )));

        assert!(range.overlaps(&TimeRange::new(
            range.from + Duration::minutes(10),
            range.to - Duration::minutes(10),
        )));
    }

    #[test]
    fn works_for_iso8601_absolute_range() {
        assert_eq!(
            TimeRange::from_str("2026-02-19T17:35:00Z to 2026-02-19T23:00:00Z").unwrap(),
            TimeRange {
                from: Utc.with_ymd_and_hms(2026, 2, 19, 17, 35, 0).unwrap(),
                to: Utc.with_ymd_and_hms(2026, 2, 19, 23, 0, 0).unwrap(),
            }
        );
    }

    #[test]
    fn works_for_iso8601_with_offset() {
        assert_eq!(
            TimeRange::from_str("2026-02-19T17:35:00+00:00 to 2026-02-19T23:00:00+00:00").unwrap(),
            TimeRange {
                from: Utc.with_ymd_and_hms(2026, 2, 19, 17, 35, 0).unwrap(),
                to: Utc.with_ymd_and_hms(2026, 2, 19, 23, 0, 0).unwrap(),
            }
        );
    }

    #[test]
    #[should_panic(expected = "Can't create TimeRange")]
    fn iso8601_reversed_range_panics() {
        let _ =
            TimeRange::from_str("2026-02-19T23:00:00Z to 2026-02-19T17:35:00Z").unwrap();
    }

    #[test]
    fn parse_log_line_date_time_works() {
        let log_line = r###"{"_id":"AXsS3_cwAACgWnsy3dzCzQAA","date":"2021-08-04T20:34:32.841Z","source":"rsyslog","host":"multiplayer-134.prod.figma.com","message":"Disconnecting session due to duplicate reconnect key","service":"multiplayer","status":"warn","attributes":{"child_pid":681472,"reconnectKey":"1752a465678e8e44ac19e8edcf8022effcd6ac0e","file":{"key":"MAZ3340A2o4n43zKa9alKn"},"child_id":3,"level":"warn","trackingSessionId":"Oey1FghPOlbUsRA4","syslog":{"severity":6,"hostname":"multiplayer-134.prod.figma.com","appname":"multiplayer","prival":30,"facility":3,"version":0,"timestamp":"2021-08-04T20:34:32.841196+00:00"},"version":"a022b376e6438adcc1283e1a4bc2621c4999c3ee"},"tags":["env:production","source:rsyslog","availability-zone:us-west-2c","cluster:production","image:ami-0928f4202481dfdf6","instance-type:m4.10xlarge","kernel:none","name:production-multiplayer-134","region:us-west-2","role-multiplayer:true","security-group:sg-01c4cc9008beb970a","security-group:sg-0610441d0f7ba06a5"]}"###;
        assert_eq!(
            TimeRange::parse_log_line_date_time(log_line),
            NaiveDate::from_ymd_opt(2021, 8, 4)
                .unwrap()
                .and_hms_micro_opt(20, 34, 32, 841_000)
                .unwrap()
                .and_local_timezone(Utc)
                .unwrap(),
        );
    }
}
