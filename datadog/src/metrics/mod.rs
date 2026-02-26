pub(crate) mod api;

use chrono::{Local, TimeZone, Utc};
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
        /// Repeatable with name= prefix for formula queries (e.g. --query "a=count:metric{*}").
        #[structopt(long)]
        query: Vec<String>,

        /// Combine named queries with arithmetic (e.g. --formula "a * b").
        /// Requires all --query values to have a name= prefix.
        #[structopt(long)]
        formula: Option<Vec<String>>,

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

    /// List known tag values for a given metric and tag key.
    TagValues {
        /// Metric name (e.g. "sinatra.async_worker.jobs.execution_time_distrib").
        #[structopt(long)]
        metric: String,

        /// Tag key to list values for (e.g. "job_name").
        #[structopt(long)]
        tag: String,

        /// Optional glob filter on tag values (e.g. "*filechunk*").
        #[structopt(long)]
        filter: Option<String>,
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
// Named query parsing (for formula mode)
// ---------------------------------------------------------------------------

pub(crate) struct NamedQuery {
    pub name: String,
    pub query: String,
}

fn parse_named_query(s: &str) -> Result<NamedQuery, String> {
    let eq_pos = s
        .find('=')
        .ok_or_else(|| format!("query missing name= prefix: {}", s))?;
    let name = &s[..eq_pos];
    let query = &s[eq_pos + 1..];
    if name.is_empty() {
        return Err(format!("empty query name in: {}", s));
    }
    if query.is_empty() {
        return Err(format!("empty query after name= in: {}", s));
    }
    Ok(NamedQuery {
        name: name.to_string(),
        query: query.to_string(),
    })
}

/// Returns true if the query string looks like it has a `name=` prefix
/// (i.e. the part before the first `=` is a short alphanumeric identifier,
/// not a metric aggregation like "avg:").
fn has_name_prefix(s: &str) -> bool {
    match s.find('=') {
        Some(pos) => {
            let prefix = &s[..pos];
            !prefix.is_empty()
                && prefix.len() <= 10
                && prefix.chars().all(|c| c.is_alphanumeric() || c == '_')
        }
        None => false,
    }
}

// ---------------------------------------------------------------------------
// Formula series info (for V2 output)
// ---------------------------------------------------------------------------

pub(crate) struct FormulaSeriesInfo {
    pub label: String,
    pub group_tags: Vec<String>,
    pub num_points: usize,
}

pub(crate) fn print_formula_series_header(info: &FormulaSeriesInfo) {
    println!("## {}", info.label);
    if !info.group_tags.is_empty() {
        println!("Tags: {}", info.group_tags.join(", "));
    }
    println!("Points: {}", info.num_points);
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
// V2 single-query helper
// ---------------------------------------------------------------------------

/// Query a single metric via the V2 timeseries formula API by auto-wrapping
/// it as an identity formula (`name="a"`, formula `"a"`).
pub(crate) async fn query_single_v2(
    api_key: &str,
    app_key: &str,
    from_ms: i64,
    to_ms: i64,
    query: &str,
    interval_ms: Option<i64>,
) -> anyhow::Result<Vec<(Vec<(f64, f64)>, FormulaSeriesInfo)>> {
    let named = NamedQuery {
        name: "a".to_string(),
        query: query.to_string(),
    };
    let formulas = vec!["a".to_string()];

    let response = api::query_timeseries_formula(
        api_key,
        app_key,
        from_ms,
        to_ms,
        &[named],
        &formulas,
        interval_ms,
    )
    .await?;

    let data = response
        .data
        .ok_or_else(|| anyhow::anyhow!("No data in formula response"))?;
    let attrs = data
        .attributes
        .ok_or_else(|| anyhow::anyhow!("No attributes in formula response"))?;

    let times = attrs.times.unwrap_or_default();
    let all_values = attrs.values.unwrap_or_default();
    let series_meta = attrs.series.unwrap_or_default();

    let results: Vec<(Vec<(f64, f64)>, FormulaSeriesInfo)> = all_values
        .iter()
        .enumerate()
        .map(|(i, vals)| {
            let pts = extract_formula_points(&times, vals);
            let meta = series_meta.get(i);
            let group_tags = meta
                .and_then(|m| m.group_tags.clone())
                .unwrap_or_default();
            let info = FormulaSeriesInfo {
                label: query.to_string(),
                group_tags,
                num_points: pts.len(),
            };
            (pts, info)
        })
        .collect();

    Ok(results)
}

// ---------------------------------------------------------------------------
// Shared result rendering
// ---------------------------------------------------------------------------

fn render_results(
    results: &[(Vec<(f64, f64)>, FormulaSeriesInfo)],
    from_ms: f64,
    to_ms: f64,
    raw: bool,
    rollup: Option<RollupInterval>,
    pivot_ms: Option<f64>,
) {
    match (raw, rollup, pivot_ms) {
        (false, None, None) => {
            let all_pts: Vec<Vec<(f64, f64)>> =
                results.iter().map(|(pts, _)| pts.clone()).collect();
            let (global_y_min, global_y_max) = global_y_range(&all_pts);
            for (i, (pts, info)) in results.iter().enumerate() {
                if i > 0 {
                    println!();
                }
                print_formula_series_header(info);
                if pts.is_empty() {
                    println!("(no data points)");
                } else {
                    let values: Vec<f64> = pts.iter().map(|(_, v)| *v).collect();
                    let min = values.iter().cloned().fold(f64::INFINITY, f64::min);
                    let max = values.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
                    let avg = values.iter().sum::<f64>() / values.len() as f64;
                    let last = *values.last().unwrap();
                    println!(
                        "Min: {:.1}  Max: {:.1}  Avg: {:.1}  Last: {:.1}",
                        min, max, avg, last,
                    );
                    print_chart(pts, from_ms, to_ms, global_y_min, global_y_max);
                }
            }
        }
        (true, None, None) => {
            for (pts, info) in results {
                print_raw_formula_points(pts, &info.label, &info.group_tags);
            }
        }
        (false, Some(interval), None) => {
            for (i, (pts, info)) in results.iter().enumerate() {
                if i > 0 {
                    println!();
                }
                print_formula_series_header(info);
                let buckets = bucket_points(pts, from_ms, interval.seconds);
                print_rollup_table(&buckets, None);
            }
        }
        (false, None, Some(pivot)) => {
            for (i, (pts, info)) in results.iter().enumerate() {
                if i > 0 {
                    println!();
                }
                print_formula_series_header(info);
                print_compare_table(pts, pivot);
            }
        }
        (false, Some(interval), Some(pivot)) => {
            for (i, (pts, info)) in results.iter().enumerate() {
                if i > 0 {
                    println!();
                }
                print_formula_series_header(info);
                let buckets = bucket_points(pts, from_ms, interval.seconds);
                print_rollup_table(&buckets, Some(pivot));
            }
        }
        (true, Some(interval), None) => {
            for (pts, info) in results {
                let buckets = bucket_points(pts, from_ms, interval.seconds);
                print_raw_rollup(&buckets, &info.label, None);
            }
        }
        (true, None, Some(pivot)) => {
            for (pts, info) in results {
                print_raw_compare(pts, pivot, &info.label);
            }
        }
        (true, Some(interval), Some(pivot)) => {
            for (pts, info) in results {
                let buckets = bucket_points(pts, from_ms, interval.seconds);
                print_raw_rollup(&buckets, &info.label, Some(pivot));
            }
        }
    }
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
            formula,
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

            let use_formula = formula.is_some();

            if use_formula {
                // V2 formula path.
                let formulas = formula.unwrap();
                if formulas.is_empty() {
                    anyhow::bail!("--formula requires at least one formula expression");
                }
                if query.is_empty() {
                    anyhow::bail!("--formula requires at least one --query with a name= prefix");
                }
                let named_queries: Vec<NamedQuery> = query
                    .iter()
                    .map(|q| {
                        parse_named_query(q).map_err(|e| anyhow::anyhow!("{}", e))
                    })
                    .collect::<anyhow::Result<Vec<_>>>()?;

                // V2 API uses milliseconds.
                let from_ms = time.from.timestamp_millis();
                let to_ms = time.to.timestamp_millis();
                let interval_ms = rollup.map(|r| r.seconds as i64 * 1000);

                let response = api::query_timeseries_formula(
                    api_key,
                    app_key,
                    from_ms,
                    to_ms,
                    &named_queries,
                    &formulas,
                    interval_ms,
                )
                .await?;

                let data = response
                    .data
                    .ok_or_else(|| anyhow::anyhow!("No data in formula response"))?;
                let attrs = data
                    .attributes
                    .ok_or_else(|| anyhow::anyhow!("No attributes in formula response"))?;

                let times = attrs.times.unwrap_or_default();
                let all_values = attrs.values.unwrap_or_default();
                let series_meta = attrs.series.unwrap_or_default();

                if all_values.is_empty() {
                    eprintln!("No series returned for formula query");
                    return Ok(());
                }

                let from_ms_f = from_ms as f64;
                let to_ms_f = to_ms as f64;

                // Build (points, info) for each result series.
                let results: Vec<(Vec<(f64, f64)>, FormulaSeriesInfo)> = all_values
                    .iter()
                    .enumerate()
                    .map(|(i, vals)| {
                        let pts = extract_formula_points(&times, vals);
                        let meta = series_meta.get(i);
                        let group_tags = meta
                            .and_then(|m| m.group_tags.clone())
                            .unwrap_or_default();
                        let query_index = meta.and_then(|m| m.query_index).unwrap_or(0) as usize;
                        // Use formula expression if available, else the query name.
                        let label = if query_index < formulas.len() {
                            formulas[query_index].clone()
                        } else if query_index < named_queries.len() {
                            named_queries[query_index].name.clone()
                        } else {
                            format!("series_{}", i)
                        };
                        let info = FormulaSeriesInfo {
                            label,
                            group_tags,
                            num_points: pts.len(),
                        };
                        (pts, info)
                    })
                    .collect();

                render_results(&results, from_ms_f, to_ms_f, raw, rollup, pivot_ms);
            } else {
                // Single-query path (auto-wrapped as V2 identity formula).
                if query.is_empty() {
                    anyhow::bail!("--query is required");
                }
                if query.len() > 1 {
                    anyhow::bail!(
                        "multiple --query values require --formula; \
                         for a single query, omit the name= prefix"
                    );
                }
                let single_query = &query[0];
                if has_name_prefix(single_query) {
                    anyhow::bail!(
                        "query has a name= prefix but --formula was not provided; \
                         either add --formula or remove the prefix"
                    );
                }

                let from_ms = time.from.timestamp_millis();
                let to_ms = time.to.timestamp_millis();
                let interval_ms = rollup.map(|r| r.seconds as i64 * 1000);

                let results =
                    query_single_v2(api_key, app_key, from_ms, to_ms, single_query, interval_ms)
                        .await?;

                if results.is_empty() {
                    eprintln!("No series returned for query: {}", single_query);
                    return Ok(());
                }

                let from_ms_f = from_ms as f64;
                let to_ms_f = to_ms as f64;
                render_results(&results, from_ms_f, to_ms_f, raw, rollup, pivot_ms);
            }

            Ok(())
        }

        MetricsCommand::TagValues {
            metric,
            tag,
            filter,
        } => {
            let response = api::list_tags_by_metric_name(api_key, app_key, &metric).await?;

            let tags = response
                .data
                .and_then(|d| d.attributes)
                .and_then(|a| a.tags)
                .unwrap_or_default();

            // Extract values for the requested tag key.
            let prefix = format!("{}:", tag);
            let mut values: Vec<&str> = tags
                .iter()
                .filter_map(|t| t.strip_prefix(&prefix))
                .collect();

            // Apply glob filter if provided.
            if let Some(ref pattern) = filter {
                let glob = glob_pattern(pattern);
                values.retain(|v| glob_match(&glob, v));
            }

            values.sort_unstable();
            values.dedup();

            if values.is_empty() {
                eprintln!("No values found for tag \"{}\" on metric \"{}\"", tag, metric);
            } else {
                for v in &values {
                    println!("{}", v);
                }
            }

            Ok(())
        }
    }
}

// ---------------------------------------------------------------------------
// Point extraction & helpers
// ---------------------------------------------------------------------------

fn extract_formula_points(times: &[i64], values: &[Option<f64>]) -> Vec<(f64, f64)> {
    times
        .iter()
        .zip(values)
        .filter_map(|(t, v)| v.map(|val| (*t as f64, val)))
        .collect()
}

pub(crate) fn global_y_range(all_points: &[Vec<(f64, f64)>]) -> (f64, f64) {
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
// Output: summary + chart
// ---------------------------------------------------------------------------

const CHART_WIDTH: u32 = 120;
const CHART_HEIGHT: u32 = 40;

/// Print a line chart to stdout using textplots, with a human-readable
/// local-time X axis printed below.
pub(crate) fn print_chart(points: &[(f64, f64)], from_ms: f64, to_ms: f64, y_min: f64, y_max: f64) {
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

fn print_raw_formula_points(points: &[(f64, f64)], label: &str, group_tags: &[String]) {
    let series_name = if group_tags.is_empty() {
        label.to_string()
    } else {
        format!("{}{{{}}}", label, group_tags.join(","))
    };
    for &(ts_ms, value) in points {
        let dt = Utc.timestamp_millis_opt(ts_ms as i64).unwrap();
        let point = RawPoint {
            series: series_name.clone(),
            timestamp: dt.to_rfc3339(),
            value,
        };
        println!("{}", serde_json::to_string(&point).unwrap());
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
// Glob matching for --filter
// ---------------------------------------------------------------------------

/// Compile a simple glob pattern (supporting only `*`) into a list of literal
/// segments.  E.g. `"*filechunk*"` → `["", "filechunk", ""]`.
fn glob_pattern(pattern: &str) -> Vec<String> {
    pattern.split('*').map(String::from).collect()
}

/// Check whether `text` matches the glob segments produced by [`glob_pattern`].
fn glob_match(segments: &[String], text: &str) -> bool {
    if segments.is_empty() {
        return text.is_empty();
    }

    // The first segment must be a prefix of the text.
    if !text.starts_with(segments[0].as_str()) {
        return false;
    }
    let mut rest = &text[segments[0].len()..];

    for seg in &segments[1..] {
        if let Some(pos) = rest.find(seg.as_str()) {
            rest = &rest[pos + seg.len()..];
        } else {
            return false;
        }
    }

    // If the pattern ended with `*` the last segment is "" and we already
    // consumed it. If it didn't, the last segment must reach exactly to the
    // end of `text` — which means `rest` must be empty.
    if segments.last().map_or(false, |s| !s.is_empty()) {
        rest.is_empty()
    } else {
        true
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

    // -- parse_named_query --

    #[test]
    fn parse_named_query_valid() {
        let nq = parse_named_query("a=count:metric{env:prod}.as_count()").unwrap();
        assert_eq!(nq.name, "a");
        assert_eq!(nq.query, "count:metric{env:prod}.as_count()");
    }

    #[test]
    fn parse_named_query_multi_equals() {
        // Only splits on the first '='.
        let nq = parse_named_query("b=avg:metric{tag=value}").unwrap();
        assert_eq!(nq.name, "b");
        assert_eq!(nq.query, "avg:metric{tag=value}");
    }

    #[test]
    fn parse_named_query_missing_prefix() {
        assert!(parse_named_query("count:metric{*}").is_err());
    }

    #[test]
    fn parse_named_query_empty_name() {
        assert!(parse_named_query("=count:metric{*}").is_err());
    }

    #[test]
    fn parse_named_query_empty_query() {
        assert!(parse_named_query("a=").is_err());
    }

    // -- has_name_prefix --

    #[test]
    fn has_name_prefix_true() {
        assert!(has_name_prefix("a=count:metric{*}"));
        assert!(has_name_prefix("abc=avg:metric{*}"));
        assert!(has_name_prefix("q1=sum:metric{*}"));
    }

    #[test]
    fn has_name_prefix_false() {
        // No '=' at all.
        assert!(!has_name_prefix("avg:metric{*}"));
        // Prefix too long or has non-alphanumeric chars — treated as not a name.
        assert!(!has_name_prefix("avg:metric{tag=value}"));
    }

    // -- extract_formula_points --

    #[test]
    fn extract_formula_points_basic() {
        let times = vec![1000, 2000, 3000];
        let values = vec![Some(1.0), None, Some(3.0)];
        let pts = extract_formula_points(&times, &values);
        assert_eq!(pts.len(), 2);
        assert_eq!(pts[0], (1000.0, 1.0));
        assert_eq!(pts[1], (3000.0, 3.0));
    }

    #[test]
    fn extract_formula_points_empty() {
        let pts = extract_formula_points(&[], &[]);
        assert!(pts.is_empty());
    }

    // -- glob_match --

    #[test]
    fn glob_match_wildcard_both_ends() {
        let pat = glob_pattern("*file_chunk*");
        assert!(glob_match(&pat, "ml_file_chunks_index_job"));
        assert!(glob_match(&pat, "file_chunk_upload"));
        assert!(glob_match(&pat, "file_chunk"));
        assert!(!glob_match(&pat, "something_else"));
    }

    #[test]
    fn glob_match_prefix_wildcard() {
        let pat = glob_pattern("ml_*");
        assert!(glob_match(&pat, "ml_file_chunks"));
        assert!(glob_match(&pat, "ml_"));
        assert!(!glob_match(&pat, "other"));
    }

    #[test]
    fn glob_match_no_wildcard() {
        let pat = glob_pattern("exact");
        assert!(glob_match(&pat, "exact"));
        assert!(!glob_match(&pat, "exact_suffix"));
        assert!(!glob_match(&pat, "prefix_exact"));
    }

    #[test]
    fn glob_match_all() {
        let pat = glob_pattern("*");
        assert!(glob_match(&pat, "anything"));
        assert!(glob_match(&pat, ""));
    }
}
