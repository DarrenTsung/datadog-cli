mod api;

use chrono::{Local, TimeZone, Utc};
use datadog_api_client::datadogV1::model::MetricsQueryMetadata;
use datadog_utils::TimeRange;
use serde_derive::Serialize;
use std::fmt;
use std::str::FromStr;
use structopt::StructOpt;
use textplots::{Chart, LabelBuilder, LabelFormat, Plot, Shape};

// ---------------------------------------------------------------------------
// CLI types
// ---------------------------------------------------------------------------

#[derive(StructOpt, Debug)]
pub struct MetricsOpt {
    #[structopt(subcommand)]
    cmd: MetricsCommand,
}

#[derive(StructOpt, Debug)]
pub enum MetricsCommand {
    /// Query metrics and display a summary or raw data points.
    Query {
        /// Datadog metric query string (e.g. "avg:system.cpu.user{env:production}").
        #[structopt(long)]
        query: String,

        /// Time range (e.g. "last 1 hour", "last 4 hours").
        #[structopt(long)]
        time: TimeRange,

        /// Output raw (timestamp, value) JSON lines instead of a summary.
        #[structopt(long)]
        raw: bool,

        /// Roll up data points into fixed-size buckets.
        /// Accepts "hourly", "daily", or a duration like "5m", "4h", "2d".
        #[structopt(long)]
        rollup: Option<RollupInterval>,

        /// Compare before/after a pivot timestamp.
        /// Accepts an ISO 8601 timestamp (e.g. "2026-02-19T17:35:00Z") or
        /// epoch seconds.
        #[structopt(long)]
        compare: Option<String>,
    },
}

// ---------------------------------------------------------------------------
// RollupInterval
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RollupInterval {
    pub seconds: u64,
}

impl RollupInterval {
    pub fn new(seconds: u64) -> Self {
        Self { seconds }
    }
}

impl fmt::Display for RollupInterval {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}s", self.seconds)
    }
}

impl FromStr for RollupInterval {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "hourly" => return Ok(RollupInterval::new(3600)),
            "daily" => return Ok(RollupInterval::new(86400)),
            _ => {}
        }

        let unit_start = s
            .find(|c: char| c.is_alphabetic())
            .ok_or_else(|| format!("invalid rollup interval: {}", s))?;
        let (num_str, unit) = s.split_at(unit_start);
        let num: u64 = num_str
            .parse()
            .map_err(|_| format!("invalid number in rollup interval: {}", s))?;
        if num == 0 {
            return Err(format!("rollup interval must be > 0: {}", s));
        }
        let multiplier = match unit.to_lowercase().as_str() {
            "m" | "min" | "mins" | "minute" | "minutes" => 60,
            "h" | "hr" | "hrs" | "hour" | "hours" => 3600,
            "d" | "day" | "days" => 86400,
            _ => return Err(format!("unknown unit in rollup interval: {}", s)),
        };
        Ok(RollupInterval::new(num * multiplier))
    }
}

// ---------------------------------------------------------------------------
// Bucket / Stats types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct Bucket {
    start_ms: f64,
    end_ms: f64,
    values: Vec<f64>,
}

impl Bucket {
    fn stats(&self) -> Option<Stats> {
        if self.values.is_empty() {
            return None;
        }
        Some(compute_stats(&self.values))
    }
}

#[derive(Debug, Clone, Serialize)]
struct Stats {
    avg: f64,
    min: f64,
    max: f64,
    count: usize,
}

fn compute_stats(values: &[f64]) -> Stats {
    let min = values.iter().cloned().fold(f64::INFINITY, f64::min);
    let max = values.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
    let avg = values.iter().sum::<f64>() / values.len() as f64;
    Stats {
        avg,
        min,
        max,
        count: values.len(),
    }
}

// ---------------------------------------------------------------------------
// Bucketing logic
// ---------------------------------------------------------------------------

fn bucket_points(points: &[(f64, f64)], from_ms: f64, interval_secs: u64) -> Vec<Bucket> {
    if points.is_empty() || interval_secs == 0 {
        return Vec::new();
    }

    let interval_ms = interval_secs as f64 * 1000.0;

    // Find the range of bucket indices we need.
    let last_ts = points.last().map(|(ts, _)| *ts).unwrap_or(from_ms);
    let num_buckets = ((last_ts - from_ms) / interval_ms).ceil() as usize + 1;

    let mut buckets: Vec<Bucket> = (0..num_buckets)
        .map(|i| {
            let start = from_ms + i as f64 * interval_ms;
            Bucket {
                start_ms: start,
                end_ms: start + interval_ms,
                values: Vec::new(),
            }
        })
        .collect();

    for &(ts, val) in points {
        let idx = ((ts - from_ms) / interval_ms).floor() as usize;
        if idx < buckets.len() {
            buckets[idx].values.push(val);
        }
    }

    // Filter out empty buckets.
    buckets.retain(|b| !b.values.is_empty());
    buckets
}

// ---------------------------------------------------------------------------
// Compare timestamp parsing
// ---------------------------------------------------------------------------

fn parse_compare_timestamp(s: &str) -> Result<f64, String> {
    // Try RFC 3339 / ISO 8601 first.
    if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(s) {
        return Ok(dt.timestamp_millis() as f64);
    }
    // Try epoch seconds (integer or float).
    if let Ok(epoch_s) = s.parse::<f64>() {
        return Ok(epoch_s * 1000.0);
    }
    Err(format!(
        "invalid compare timestamp: {} (expected ISO 8601 or epoch seconds)",
        s
    ))
}

// ---------------------------------------------------------------------------
// Main entry point
// ---------------------------------------------------------------------------

pub async fn run_metrics(
    api_key: &str,
    app_key: &str,
    opt: MetricsOpt,
) -> anyhow::Result<()> {
    match opt.cmd {
        MetricsCommand::Query {
            query,
            time,
            raw,
            rollup,
            compare,
        } => {
            let pivot_ms = match &compare {
                Some(ts_str) => Some(
                    parse_compare_timestamp(ts_str)
                        .map_err(|e| anyhow::anyhow!("{}", e))?,
                ),
                None => None,
            };

            // V1 query API takes epoch seconds, not milliseconds.
            let from_s = time.from.timestamp();
            let to_s = time.to.timestamp();

            let response = api::query_metrics(api_key, app_key, from_s, to_s, &query).await?;

            let series = response.series.unwrap_or_default();
            if series.is_empty() {
                eprintln!("No series returned for query: {}", query);
                return Ok(());
            }

            // Pointlist timestamps are in milliseconds.
            let from_ms = from_s as f64 * 1000.0;
            let to_ms = to_s as f64 * 1000.0;

            let all_points: Vec<Vec<(f64, f64)>> =
                series.iter().map(|s| extract_points(s)).collect();

            match (raw, rollup, pivot_ms) {
                // Existing: summary + chart
                (false, None, None) => {
                    let (global_y_min, global_y_max) = global_y_range(&all_points);
                    for (i, (s, pts)) in series.iter().zip(&all_points).enumerate() {
                        if i > 0 {
                            println!();
                        }
                        print_series_summary(s, pts, from_ms, to_ms, global_y_min, global_y_max);
                    }
                }
                // Existing: raw JSON lines
                (true, None, None) => {
                    print_raw_points(&series);
                }
                // Rollup table
                (false, Some(interval), None) => {
                    for (i, (s, pts)) in series.iter().zip(&all_points).enumerate() {
                        if i > 0 {
                            println!();
                        }
                        print_series_header(s, pts);
                        let buckets = bucket_points(pts, from_ms, interval.seconds);
                        print_rollup_table(&buckets, None);
                    }
                }
                // Compare table
                (false, None, Some(pivot)) => {
                    for (i, (s, pts)) in series.iter().zip(&all_points).enumerate() {
                        if i > 0 {
                            println!();
                        }
                        print_series_header(s, pts);
                        print_compare_table(pts, pivot);
                    }
                }
                // Rollup + compare
                (false, Some(interval), Some(pivot)) => {
                    for (i, (s, pts)) in series.iter().zip(&all_points).enumerate() {
                        if i > 0 {
                            println!();
                        }
                        print_series_header(s, pts);
                        let buckets = bucket_points(pts, from_ms, interval.seconds);
                        print_rollup_table(&buckets, Some(pivot));
                    }
                }
                // Raw + rollup
                (true, Some(interval), None) => {
                    for (s, pts) in series.iter().zip(&all_points) {
                        let series_label = series_label(s);
                        let buckets = bucket_points(pts, from_ms, interval.seconds);
                        print_raw_rollup(&buckets, &series_label, None);
                    }
                }
                // Raw + compare
                (true, None, Some(pivot)) => {
                    for (s, pts) in series.iter().zip(&all_points) {
                        let series_label = series_label(s);
                        print_raw_compare(pts, pivot, &series_label);
                    }
                }
                // Raw + rollup + compare
                (true, Some(interval), Some(pivot)) => {
                    for (s, pts) in series.iter().zip(&all_points) {
                        let series_label = series_label(s);
                        let buckets = bucket_points(pts, from_ms, interval.seconds);
                        print_raw_rollup(&buckets, &series_label, Some(pivot));
                    }
                }
            }

            Ok(())
        }
    }
}

// ---------------------------------------------------------------------------
// Point extraction & helpers
// ---------------------------------------------------------------------------

fn extract_points(series: &MetricsQueryMetadata) -> Vec<(f64, f64)> {
    series
        .pointlist
        .as_ref()
        .map(|points| {
            points
                .iter()
                .filter_map(|pair| match pair.as_slice() {
                    [Some(ts), Some(val)] => Some((*ts, *val)),
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default()
}

fn series_label(series: &MetricsQueryMetadata) -> String {
    series
        .tag_set
        .as_ref()
        .map(|t| t.join(","))
        .or_else(|| series.scope.clone())
        .unwrap_or_default()
}

fn global_y_range(all_points: &[Vec<(f64, f64)>]) -> (f64, f64) {
    let y_min = all_points
        .iter()
        .flat_map(|pts| pts.iter().map(|(_, v)| *v))
        .fold(f64::INFINITY, f64::min);
    let y_max = all_points
        .iter()
        .flat_map(|pts| pts.iter().map(|(_, v)| *v))
        .fold(f64::NEG_INFINITY, f64::max);
    (y_min, y_max)
}

/// Format an epoch-ms timestamp as a local time string.
/// Uses "Mon 14:00" for multi-day ranges, "14:00" for same-day.
fn format_ts_local(epoch_ms: f64, include_date: bool) -> String {
    let dt = Utc
        .timestamp_millis_opt(epoch_ms as i64)
        .unwrap()
        .with_timezone(&Local);
    if include_date {
        dt.format("%a %H:%M").to_string()
    } else {
        dt.format("%H:%M").to_string()
    }
}

// ---------------------------------------------------------------------------
// Output: series header (extracted from print_series_summary)
// ---------------------------------------------------------------------------

fn print_series_header(series: &MetricsQueryMetadata, points: &[(f64, f64)]) {
    let display_name = series
        .display_name
        .as_deref()
        .or(series.expression.as_deref())
        .unwrap_or("unknown");
    println!("## {}", display_name);

    if let Some(tags) = &series.tag_set {
        if !tags.is_empty() {
            println!("Tags: {}", tags.join(", "));
        }
    }

    let interval_str = series
        .interval
        .map(|i| format!("{}s", i))
        .unwrap_or_else(|| "?".to_string());
    println!("Points: {}  |  Interval: {}", points.len(), interval_str);
}

// ---------------------------------------------------------------------------
// Output: summary + chart (unchanged behaviour)
// ---------------------------------------------------------------------------

const CHART_WIDTH: u32 = 120;
const CHART_HEIGHT: u32 = 40;

/// Print a line chart to stdout using textplots, with a human-readable
/// local-time X axis printed below.
fn print_chart(points: &[(f64, f64)], from_ms: f64, to_ms: f64, y_min: f64, y_max: f64) {
    if points.is_empty() {
        return;
    }

    // Normalize X to seconds-offset from `from_ms` to avoid f32 precision
    // loss with large epoch-ms values.
    let duration_s = (to_ms - from_ms) / 1000.0;
    let plot_data: Vec<(f32, f32)> = points
        .iter()
        .map(|(ts, v)| (((ts - from_ms) / 1000.0) as f32, *v as f32))
        .collect();

    Chart::new_with_y_range(
        CHART_WIDTH,
        CHART_HEIGHT,
        0.0,
        duration_s as f32,
        y_min as f32,
        y_max as f32,
    )
    .x_label_format(LabelFormat::None)
    .lineplot(&Shape::Lines(&plot_data))
    .nice();

    // Print a custom time axis with evenly-spaced local timestamps.
    // The chart's plot area width in characters (excluding Y-axis labels).
    // textplots uses ~10 chars for Y-axis labels on the right, so the plot
    // area is roughly CHART_WIDTH/2 - 10 characters wide (each braille char
    // is 2 dots wide).
    let plot_chars = (CHART_WIDTH / 2) as usize;
    let include_date = duration_s > 86400.0;

    // Pick number of ticks that fit nicely.
    let num_ticks = 5;
    let mut axis_line = vec![' '; plot_chars];
    let mut labels: Vec<(usize, String)> = Vec::new();

    for i in 0..=num_ticks {
        let frac = i as f64 / num_ticks as f64;
        let ts = from_ms + frac * (to_ms - from_ms);
        let label = format_ts_local(ts, include_date);
        let pos = (frac * (plot_chars - 1) as f64).round() as usize;
        labels.push((pos, label));
    }

    // Place tick marks.
    for &(pos, _) in &labels {
        if pos < axis_line.len() {
            axis_line[pos] = '┼';
        }
    }
    // Fill dashes between ticks.
    for i in 0..axis_line.len() {
        if axis_line[i] == ' ' {
            axis_line[i] = '─';
        }
    }
    println!("{}", axis_line.iter().collect::<String>());

    // Place labels below tick marks, avoiding overlap.
    // Use a wider buffer so the last label isn't truncated.
    let buf_len = plot_chars + 12;
    let mut label_line = vec![' '; buf_len];
    for (pos, label) in &labels {
        // Center the label on the tick position.
        let start = pos.saturating_sub(label.len() / 2);
        let end = (start + label.len()).min(buf_len);
        // Check for overlap with already-placed text.
        let slot_free = label_line[start..end].iter().all(|c| *c == ' ');
        if slot_free {
            for (j, ch) in label.chars().enumerate() {
                if start + j < buf_len {
                    label_line[start + j] = ch;
                }
            }
        }
    }
    let rendered: String = label_line.iter().collect::<String>();
    println!("{}", rendered.trim_end());
}

fn print_series_summary(
    series: &MetricsQueryMetadata,
    points: &[(f64, f64)],
    from_ms: f64,
    to_ms: f64,
    y_min: f64,
    y_max: f64,
) {
    print_series_header(series, points);

    if points.is_empty() {
        println!("(no data points)");
        return;
    }

    let values: Vec<f64> = points.iter().map(|(_, v)| *v).collect();
    let min = values.iter().cloned().fold(f64::INFINITY, f64::min);
    let max = values.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
    let avg = values.iter().sum::<f64>() / values.len() as f64;
    let last = *values.last().unwrap();

    println!(
        "Min: {:.1}  Max: {:.1}  Avg: {:.1}  Last: {:.1}",
        min, max, avg, last,
    );

    print_chart(points, from_ms, to_ms, y_min, y_max);
}

// ---------------------------------------------------------------------------
// Output: rollup table
// ---------------------------------------------------------------------------

fn print_rollup_table(buckets: &[Bucket], pivot_ms: Option<f64>) {
    if buckets.is_empty() {
        println!("(no data points)");
        return;
    }

    // Determine whether to include day-of-week in timestamps.
    let span_ms = buckets.last().unwrap().end_ms - buckets.first().unwrap().start_ms;
    let include_date = span_ms > 86_400_000.0;

    println!(
        "{:<22}| {:>9} | {:>9} | {:>9} | {:>5}",
        "Bucket", "Avg", "Min", "Max", "n"
    );
    println!(
        "{:-<22}+-{:-<9}-+-{:-<9}-+-{:-<9}-+-{:-<5}",
        "", "", "", "", ""
    );

    let mut passed_pivot = false;

    for bucket in buckets {
        // Insert pivot separator if needed.
        if let Some(pivot) = pivot_ms {
            if !passed_pivot && bucket.start_ms >= pivot {
                println!(
                    "{:=<22}+=={:=<9}=+=={:=<9}=+=={:=<9}=+=={:=<5}",
                    "", "", "", "", ""
                );
                passed_pivot = true;
            }
        }

        let start_label = format_ts_local(bucket.start_ms, include_date);
        let end_label = format_ts_local(bucket.end_ms, include_date);
        let label = format!("{} - {}", start_label, end_label);

        if let Some(stats) = bucket.stats() {
            println!(
                "{:<22}| {:>9.1} | {:>9.1} | {:>9.1} | {:>5}",
                label, stats.avg, stats.min, stats.max, stats.count,
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Output: compare table
// ---------------------------------------------------------------------------

fn print_compare_table(points: &[(f64, f64)], pivot_ms: f64) {
    if points.is_empty() {
        println!("(no data points)");
        return;
    }

    let before_vals: Vec<f64> = points
        .iter()
        .filter(|(ts, _)| *ts < pivot_ms)
        .map(|(_, v)| *v)
        .collect();
    let after_vals: Vec<f64> = points
        .iter()
        .filter(|(ts, _)| *ts >= pivot_ms)
        .map(|(_, v)| *v)
        .collect();

    println!(
        "{:<12}| {:>9} | {:>9} | {:>9} | {:>5}",
        "", "Avg", "Min", "Max", "n"
    );
    println!(
        "{:-<12}+-{:-<9}-+-{:-<9}-+-{:-<9}-+-{:-<5}",
        "", "", "", "", ""
    );

    if before_vals.is_empty() {
        println!(
            "{:<12}| {:>9} | {:>9} | {:>9} | {:>5}",
            "Before", "-", "-", "-", "0"
        );
    } else {
        let bs = compute_stats(&before_vals);
        println!(
            "{:<12}| {:>9.1} | {:>9.1} | {:>9.1} | {:>5}",
            "Before", bs.avg, bs.min, bs.max, bs.count,
        );
    }

    if after_vals.is_empty() {
        println!(
            "{:<12}| {:>9} | {:>9} | {:>9} | {:>5}",
            "After", "-", "-", "-", "0"
        );
    } else {
        let a_s = compute_stats(&after_vals);
        println!(
            "{:<12}| {:>9.1} | {:>9.1} | {:>9.1} | {:>5}",
            "After", a_s.avg, a_s.min, a_s.max, a_s.count,
        );
    }

    if !before_vals.is_empty() && !after_vals.is_empty() {
        let bs = compute_stats(&before_vals);
        let a_s = compute_stats(&after_vals);
        let delta_avg = a_s.avg - bs.avg;
        println!(
            "{:<12}| {:>+9.1} | {:>9} | {:>9} | {:>5}",
            "Delta", delta_avg, "", "",  "",
        );
    }
}

// ---------------------------------------------------------------------------
// Output: raw variants
// ---------------------------------------------------------------------------

#[derive(Serialize)]
struct RawPoint {
    series: String,
    timestamp: String,
    value: f64,
}

#[derive(Serialize)]
struct RawBucketPoint {
    series: String,
    bucket_start: String,
    bucket_end: String,
    avg: f64,
    min: f64,
    max: f64,
    count: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    period: Option<String>,
}

#[derive(Serialize)]
struct RawComparePoint {
    series: String,
    period: String,
    avg: f64,
    min: f64,
    max: f64,
    count: usize,
}

fn print_raw_points(series_list: &[MetricsQueryMetadata]) {
    for series in series_list {
        let label = series_label(series);
        let points = extract_points(series);
        for (ts_ms, value) in points {
            let dt = Utc.timestamp_millis_opt(ts_ms as i64).unwrap();
            let point = RawPoint {
                series: label.clone(),
                timestamp: dt.to_rfc3339(),
                value,
            };
            println!("{}", serde_json::to_string(&point).unwrap());
        }
    }
}

fn print_raw_rollup(buckets: &[Bucket], series_label: &str, pivot_ms: Option<f64>) {
    for bucket in buckets {
        if let Some(stats) = bucket.stats() {
            let start_dt = Utc.timestamp_millis_opt(bucket.start_ms as i64).unwrap();
            let end_dt = Utc.timestamp_millis_opt(bucket.end_ms as i64).unwrap();

            let period = pivot_ms.map(|pivot| {
                if bucket.end_ms <= pivot {
                    "before".to_string()
                } else if bucket.start_ms >= pivot {
                    "after".to_string()
                } else {
                    "pivot".to_string()
                }
            });

            let point = RawBucketPoint {
                series: series_label.to_string(),
                bucket_start: start_dt.to_rfc3339(),
                bucket_end: end_dt.to_rfc3339(),
                avg: stats.avg,
                min: stats.min,
                max: stats.max,
                count: stats.count,
                period,
            };
            println!("{}", serde_json::to_string(&point).unwrap());
        }
    }
}

fn print_raw_compare(points: &[(f64, f64)], pivot_ms: f64, series_label: &str) {
    let before_vals: Vec<f64> = points
        .iter()
        .filter(|(ts, _)| *ts < pivot_ms)
        .map(|(_, v)| *v)
        .collect();
    let after_vals: Vec<f64> = points
        .iter()
        .filter(|(ts, _)| *ts >= pivot_ms)
        .map(|(_, v)| *v)
        .collect();

    if !before_vals.is_empty() {
        let s = compute_stats(&before_vals);
        let point = RawComparePoint {
            series: series_label.to_string(),
            period: "before".to_string(),
            avg: s.avg,
            min: s.min,
            max: s.max,
            count: s.count,
        };
        println!("{}", serde_json::to_string(&point).unwrap());
    }
    if !after_vals.is_empty() {
        let s = compute_stats(&after_vals);
        let point = RawComparePoint {
            series: series_label.to_string(),
            period: "after".to_string(),
            avg: s.avg,
            min: s.min,
            max: s.max,
            count: s.count,
        };
        println!("{}", serde_json::to_string(&point).unwrap());
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // -- RollupInterval::from_str --

    #[test]
    fn rollup_interval_named() {
        assert_eq!(
            RollupInterval::from_str("hourly").unwrap(),
            RollupInterval::new(3600)
        );
        assert_eq!(
            RollupInterval::from_str("daily").unwrap(),
            RollupInterval::new(86400)
        );
        assert_eq!(
            RollupInterval::from_str("Hourly").unwrap(),
            RollupInterval::new(3600)
        );
    }

    #[test]
    fn rollup_interval_durations() {
        assert_eq!(
            RollupInterval::from_str("5m").unwrap(),
            RollupInterval::new(300)
        );
        assert_eq!(
            RollupInterval::from_str("4h").unwrap(),
            RollupInterval::new(14400)
        );
        assert_eq!(
            RollupInterval::from_str("2d").unwrap(),
            RollupInterval::new(172800)
        );
    }

    #[test]
    fn rollup_interval_invalid() {
        assert!(RollupInterval::from_str("").is_err());
        assert!(RollupInterval::from_str("abc").is_err());
        assert!(RollupInterval::from_str("0m").is_err());
        assert!(RollupInterval::from_str("5x").is_err());
    }

    // -- bucket_points --

    #[test]
    fn bucket_points_basic() {
        let from = 0.0;
        let interval = 3600; // 1 hour in seconds
        let points = vec![
            (100_000.0, 1.0),
            (200_000.0, 2.0),
            (3_700_000.0, 10.0), // second hour
        ];
        let buckets = bucket_points(&points, from, interval);
        assert_eq!(buckets.len(), 2);
        assert_eq!(buckets[0].values.len(), 2);
        assert_eq!(buckets[1].values.len(), 1);
        assert_eq!(buckets[1].values[0], 10.0);
    }

    #[test]
    fn bucket_points_empty() {
        let buckets = bucket_points(&[], 0.0, 3600);
        assert!(buckets.is_empty());
    }

    #[test]
    fn bucket_points_gaps() {
        let from = 0.0;
        let interval = 3600;
        // Points only in bucket 0 and bucket 2; bucket 1 is empty and filtered.
        let points = vec![
            (100_000.0, 1.0),
            (7_300_000.0, 5.0), // > 2 hours out
        ];
        let buckets = bucket_points(&points, from, interval);
        assert_eq!(buckets.len(), 2);
        // First bucket starts at 0, second bucket starts at 7200000 (2h).
        assert!((buckets[0].start_ms - 0.0).abs() < 0.1);
        assert!((buckets[1].start_ms - 7_200_000.0).abs() < 0.1);
    }

    // -- parse_compare_timestamp --

    #[test]
    fn parse_compare_iso8601() {
        let ms = parse_compare_timestamp("2026-02-19T17:35:00Z").unwrap();
        // Should be roughly 1771526100000 ms.
        assert!(ms > 1_700_000_000_000.0);
    }

    #[test]
    fn parse_compare_epoch_seconds() {
        let ms = parse_compare_timestamp("1700000000").unwrap();
        assert!((ms - 1_700_000_000_000.0).abs() < 0.1);
    }

    #[test]
    fn parse_compare_invalid() {
        assert!(parse_compare_timestamp("not-a-timestamp").is_err());
    }

    // -- compute_stats --

    #[test]
    fn compute_stats_basic() {
        let s = compute_stats(&[1.0, 2.0, 3.0, 4.0, 5.0]);
        assert!((s.avg - 3.0).abs() < f64::EPSILON);
        assert!((s.min - 1.0).abs() < f64::EPSILON);
        assert!((s.max - 5.0).abs() < f64::EPSILON);
        assert_eq!(s.count, 5);
    }

    #[test]
    fn compute_stats_single() {
        let s = compute_stats(&[42.0]);
        assert!((s.avg - 42.0).abs() < f64::EPSILON);
        assert!((s.min - 42.0).abs() < f64::EPSILON);
        assert!((s.max - 42.0).abs() < f64::EPSILON);
        assert_eq!(s.count, 1);
    }

    // -- Print smoke tests (no-panic) --

    #[test]
    fn print_chart_does_not_panic() {
        let from = 1_700_000_000_000.0;
        let to = from + 3_600_000.0;
        let points = vec![(from, 1.0), (from + 1_800_000.0, 3.0), (to, 2.0)];
        print_chart(&points, from, to, 0.0, 5.0);
        print_chart(&[], from, to, 0.0, 1.0);
    }

    #[test]
    fn format_ts_local_works() {
        let label = format_ts_local(1_700_000_000_000.0, false);
        assert!(label.contains(':'));
        let label_with_date = format_ts_local(1_700_000_000_000.0, true);
        assert!(label_with_date.contains(' '));
    }

    #[test]
    fn print_rollup_table_does_not_panic() {
        let buckets = vec![
            Bucket {
                start_ms: 1_700_000_000_000.0,
                end_ms: 1_700_003_600_000.0,
                values: vec![1.0, 2.0, 3.0],
            },
            Bucket {
                start_ms: 1_700_003_600_000.0,
                end_ms: 1_700_007_200_000.0,
                values: vec![4.0, 5.0],
            },
        ];
        print_rollup_table(&buckets, None);
        print_rollup_table(&buckets, Some(1_700_003_600_000.0));
        print_rollup_table(&[], None);
    }

    #[test]
    fn print_compare_table_does_not_panic() {
        let points = vec![
            (1_700_000_000_000.0, 1.0),
            (1_700_001_000_000.0, 2.0),
            (1_700_003_600_000.0, 10.0),
            (1_700_004_000_000.0, 12.0),
        ];
        print_compare_table(&points, 1_700_003_600_000.0);
        print_compare_table(&[], 1_700_003_600_000.0);
    }
}
