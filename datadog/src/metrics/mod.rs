mod api;

use chrono::{Local, TimeZone, Utc};
use datadog_api_client::datadogV1::model::MetricsQueryMetadata;
use datadog_utils::TimeRange;
use serde_derive::Serialize;
use structopt::StructOpt;
use textplots::{Chart, LabelBuilder, LabelFormat, Plot, Shape};

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
    },
}

pub async fn run_metrics(
    api_key: &str,
    app_key: &str,
    opt: MetricsOpt,
) -> anyhow::Result<()> {
    match opt.cmd {
        MetricsCommand::Query { query, time, raw } => {
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

            if raw {
                print_raw_points(&series);
            } else {
                // Compute global Y range across all series.
                let all_points: Vec<Vec<(f64, f64)>> =
                    series.iter().map(|s| extract_points(s)).collect();
                let global_y_min = all_points
                    .iter()
                    .flat_map(|pts| pts.iter().map(|(_, v)| *v))
                    .fold(f64::INFINITY, f64::min);
                let global_y_max = all_points
                    .iter()
                    .flat_map(|pts| pts.iter().map(|(_, v)| *v))
                    .fold(f64::NEG_INFINITY, f64::max);

                for (i, (s, pts)) in series.iter().zip(&all_points).enumerate() {
                    if i > 0 {
                        println!();
                    }
                    print_series_summary(s, pts, from_ms, to_ms, global_y_min, global_y_max);
                }
            }

            Ok(())
        }
    }
}

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

#[derive(Serialize)]
struct RawPoint {
    series: String,
    timestamp: String,
    value: f64,
}

fn print_raw_points(series_list: &[MetricsQueryMetadata]) {
    for series in series_list {
        let series_label = series
            .tag_set
            .as_ref()
            .map(|t| t.join(","))
            .or_else(|| series.scope.clone())
            .unwrap_or_default();

        let points = extract_points(series);
        for (ts_ms, value) in points {
            let dt = Utc.timestamp_millis_opt(ts_ms as i64).unwrap();
            let point = RawPoint {
                series: series_label.clone(),
                timestamp: dt.to_rfc3339(),
                value,
            };
            println!("{}", serde_json::to_string(&point).unwrap());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn print_chart_does_not_panic() {
        // 1 hour range
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
}
