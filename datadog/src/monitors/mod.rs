mod api;

use crate::events;
use crate::metrics;

use chrono::{Local, TimeZone, Utc};
use datadog_api_client::datadogV1::model::{Monitor, MonitorType};
use datadog_api_client::datadogV2::api::api_events::ListEventsOptionalParams;
use datadog_api_client::datadogV2::model::EventsSort;
use datadog_utils::TimeRange;
use std::str::FromStr;
use structopt::StructOpt;
use url::Url;

// ---------------------------------------------------------------------------
// CLI types
// ---------------------------------------------------------------------------

#[derive(StructOpt, Debug)]
pub struct MonitorsOpt {
    #[structopt(subcommand)]
    cmd: MonitorsCommand,
}

#[derive(StructOpt, Debug)]
pub enum MonitorsCommand {
    /// Inspect a monitor: metadata, underlying metric chart, and monitor events.
    Inspect {
        /// Monitor URL or numeric ID.
        monitor: String,

        /// Time range override (e.g. "last 4 hours").
        #[structopt(long)]
        time: Option<TimeRange>,

        /// Specific event ID (auto-parsed from URL, or provided manually).
        #[structopt(long)]
        event: Option<String>,

        /// Output raw JSON lines.
        #[structopt(long)]
        raw: bool,
    },
}

// ---------------------------------------------------------------------------
// URL / input parsing
// ---------------------------------------------------------------------------

struct MonitorInput {
    monitor_id: i64,
    time_range: Option<(i64, i64)>,
    event_id: Option<String>,
}

fn parse_monitor_input(s: &str) -> anyhow::Result<MonitorInput> {
    // Pure numeric → bare monitor ID.
    if let Ok(id) = s.parse::<i64>() {
        return Ok(MonitorInput {
            monitor_id: id,
            time_range: None,
            event_id: None,
        });
    }

    // Otherwise try to parse as a URL.
    let url = Url::parse(s).map_err(|_| {
        anyhow::anyhow!(
            "Invalid monitor input: expected a numeric ID or Datadog monitor URL, got: {}",
            s
        )
    })?;

    // Extract monitor ID from path: /monitors/{id}
    let monitor_id = url
        .path_segments()
        .and_then(|mut segs| {
            // Find "monitors" segment then take the next one.
            while let Some(seg) = segs.next() {
                if seg == "monitors" {
                    return segs.next();
                }
            }
            None
        })
        .and_then(|id_str| id_str.parse::<i64>().ok())
        .ok_or_else(|| {
            anyhow::anyhow!("Could not extract monitor ID from URL path: {}", url.path())
        })?;

    // Extract query params.
    let pairs: Vec<(String, String)> = url.query_pairs().map(|(k, v)| (k.to_string(), v.to_string())).collect();

    let from_ts = pairs.iter().find(|(k, _)| k == "from_ts").and_then(|(_, v)| v.parse::<i64>().ok());
    let to_ts = pairs.iter().find(|(k, _)| k == "to_ts").and_then(|(_, v)| v.parse::<i64>().ok());
    let time_range = from_ts.zip(to_ts);

    let event_id = pairs
        .iter()
        .find(|(k, _)| k == "event_id")
        .map(|(_, v)| v.clone());

    Ok(MonitorInput {
        monitor_id,
        time_range,
        event_id,
    })
}

// ---------------------------------------------------------------------------
// Metric query extraction
// ---------------------------------------------------------------------------

/// Extract the inner metric query from a monitor query string.
///
/// Monitor queries look like: `avg(last_5m):avg:system.cpu.user{env:production} > 90`
/// We want to extract: `avg:system.cpu.user{env:production}`
fn extract_metric_query(monitor_query: &str) -> Option<String> {
    let mut depth: usize = 0;
    let mut separator_pos = None;

    for (i, ch) in monitor_query.char_indices() {
        match ch {
            '(' => depth += 1,
            ')' => {
                if depth > 0 {
                    depth -= 1;
                }
                if depth == 0 {
                    // Check if next char is ':'
                    let rest = &monitor_query[i + 1..];
                    if rest.starts_with(':') {
                        separator_pos = Some(i + 2); // skip past "):"
                        break;
                    }
                }
            }
            _ => {}
        }
    }

    let start = separator_pos?;
    let rest = &monitor_query[start..];

    // Find the comparison operator boundary.
    let end = [" > ", " >= ", " < ", " <= ", " == "]
        .iter()
        .filter_map(|op| rest.find(op))
        .min()
        .unwrap_or(rest.len());

    let query = rest[..end].trim();
    if query.is_empty() {
        None
    } else {
        Some(query.to_string())
    }
}

// ---------------------------------------------------------------------------
// Display helpers
// ---------------------------------------------------------------------------

fn format_pt(dt: &chrono::DateTime<Utc>) -> String {
    dt.with_timezone(&Local).format("%Y-%m-%d %H:%M PT").to_string()
}

fn format_monitor_type(t: &MonitorType) -> String {
    t.to_string()
}

fn print_monitor_summary(monitor: &Monitor, raw: bool) {
    if raw {
        // Serialize the whole monitor as JSON.
        if let Ok(json) = serde_json::to_string(monitor) {
            println!("{}", json);
        }
        return;
    }

    let name = monitor.name.as_deref().unwrap_or("(unnamed)");
    println!("## Monitor: {}", name);

    let id_str = monitor
        .id
        .map(|id| id.to_string())
        .unwrap_or_else(|| "?".to_string());
    let type_str = format_monitor_type(&monitor.type_);
    let state_str = monitor
        .overall_state
        .as_ref()
        .map(|s| s.to_string())
        .unwrap_or_else(|| "Unknown".to_string());
    println!("ID: {}  |  Type: {}  |  Status: {}", id_str, type_str, state_str);

    let created = monitor
        .created
        .as_ref()
        .map(|dt| format_pt(dt))
        .unwrap_or_else(|| "?".to_string());
    let modified = monitor
        .modified
        .as_ref()
        .map(|dt| format_pt(dt))
        .unwrap_or_else(|| "?".to_string());
    println!("Created: {}  |  Modified: {}", created, modified);

    if let Some(creator) = &monitor.creator {
        if let Some(email) = &creator.email {
            println!("Creator: {}", email);
        }
    }

    println!();
    println!("Query: {}", monitor.query);

    // Thresholds.
    if let Some(opts) = &monitor.options {
        if let Some(thresh) = &opts.thresholds {
            println!();
            println!("Thresholds:");
            if let Some(c) = thresh.critical {
                println!("  Critical: {}", c);
            }
            if let Some(Some(w)) = thresh.warning {
                println!("  Warning: {}", w);
            }
            if let Some(Some(cr)) = thresh.critical_recovery {
                println!("  Critical Recovery: {}", cr);
            }
            if let Some(Some(wr)) = thresh.warning_recovery {
                println!("  Warning Recovery: {}", wr);
            }
        }
    }

    // Tags.
    if let Some(tags) = &monitor.tags {
        if !tags.is_empty() {
            println!();
            println!("Tags: {}", tags.join(", "));
        }
    }

    // Message.
    if let Some(msg) = &monitor.message {
        if !msg.is_empty() {
            println!();
            println!("Message:");
            // Indent the message for readability.
            for line in msg.lines() {
                println!("  {}", line);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Main entry point
// ---------------------------------------------------------------------------

pub async fn run_monitors(
    api_key: &str,
    app_key: &str,
    opt: MonitorsOpt,
) -> anyhow::Result<()> {
    match opt.cmd {
        MonitorsCommand::Inspect {
            monitor,
            time,
            event,
            raw,
        } => {
            run_monitors_inspect(api_key, app_key, &monitor, time, event, raw).await
        }
    }
}

async fn run_monitors_inspect(
    api_key: &str,
    app_key: &str,
    monitor_str: &str,
    time_flag: Option<TimeRange>,
    event_flag: Option<String>,
    raw: bool,
) -> anyhow::Result<()> {
    // 1. Parse input.
    let input = parse_monitor_input(monitor_str)?;

    // 2. Resolve time range: --time flag > URL params > default "last 1 hour".
    let time_range = if let Some(t) = time_flag {
        t
    } else if let Some((from_ms, to_ms)) = input.time_range {
        let from = Utc.timestamp_millis_opt(from_ms).single().ok_or_else(|| {
            anyhow::anyhow!("Invalid from_ts in URL: {}", from_ms)
        })?;
        let to = Utc.timestamp_millis_opt(to_ms).single().ok_or_else(|| {
            anyhow::anyhow!("Invalid to_ts in URL: {}", to_ms)
        })?;
        TimeRange { from, to }
    } else {
        TimeRange::from_str("last 1 hour").expect("valid default time range")
    };

    // 3. Resolve event ID: --event flag > URL event_id param > None.
    let event_id = event_flag.or(input.event_id);

    // 4. Fetch monitor.
    let monitor = api::get_monitor(api_key, app_key, input.monitor_id).await?;

    // 5. Print monitor summary.
    print_monitor_summary(&monitor, raw);

    // 6. If event_id is numeric, fetch it via the V1 get_event API.
    //    The notification link (Slack/email) has a numeric event_id, but
    //    Datadog's UI remaps it to an opaque base64 blob that isn't
    //    reversible — so we can only fetch numeric IDs.
    if let Some(ref eid) = event_id {
        if let Ok(numeric_id) = eid.parse::<i64>() {
            if !raw {
                println!();
                println!("---");
                println!();
            }
            print_trigger_event(api_key, app_key, numeric_id, raw).await;
        }
    }

    // 7. If metric/query alert type, extract and query the underlying metric.
    let is_metric_type = matches!(
        monitor.type_,
        MonitorType::METRIC_ALERT | MonitorType::QUERY_ALERT
    );

    if is_metric_type {
        if let Some(metric_query) = extract_metric_query(&monitor.query) {
            if !raw {
                println!();
                println!("---");
                println!();
                println!("## Underlying Metric: {}", metric_query);
            }

            let from_s = time_range.from.timestamp();
            let to_s = time_range.to.timestamp();

            match metrics::api::query_metrics(api_key, app_key, from_s, to_s, &metric_query).await
            {
                Ok(response) => {
                    let series = response.series.unwrap_or_default();
                    if series.is_empty() {
                        if raw {
                            println!(
                                "{}",
                                serde_json::json!({"section": "metric", "error": "no series returned"})
                            );
                        } else {
                            eprintln!("No series returned for metric query: {}", metric_query);
                        }
                    } else {
                        let from_ms = from_s as f64 * 1000.0;
                        let to_ms = to_s as f64 * 1000.0;

                        let all_points: Vec<Vec<(f64, f64)>> =
                            series.iter().map(|s| metrics::extract_points(s)).collect();

                        if raw {
                            // Output raw JSON points.
                            for (s, pts) in series.iter().zip(&all_points) {
                                let label = s
                                    .tag_set
                                    .as_ref()
                                    .map(|t| t.join(","))
                                    .or_else(|| s.scope.clone())
                                    .unwrap_or_default();
                                for &(ts_ms, value) in pts {
                                    let dt = Utc.timestamp_millis_opt(ts_ms as i64).unwrap();
                                    println!(
                                        "{}",
                                        serde_json::json!({
                                            "section": "metric",
                                            "series": label,
                                            "timestamp": dt.to_rfc3339(),
                                            "value": value,
                                        })
                                    );
                                }
                            }
                        } else {
                            let (global_y_min, global_y_max) =
                                metrics::global_y_range(&all_points);
                            for (i, (s, pts)) in
                                series.iter().zip(&all_points).enumerate()
                            {
                                if i > 0 {
                                    println!();
                                }
                                metrics::print_series_summary(
                                    s,
                                    pts,
                                    from_ms,
                                    to_ms,
                                    global_y_min,
                                    global_y_max,
                                );
                            }
                        }
                    }
                }
                Err(err) => {
                    if raw {
                        println!(
                            "{}",
                            serde_json::json!({"section": "metric", "error": err.to_string()})
                        );
                    } else {
                        eprintln!("Failed to query underlying metric: {}", err);
                    }
                }
            }
        }
    }

    // 7. Query monitor events.
    if !raw {
        println!();
        println!("---");
        println!();
        println!("## Monitor Events");
    }

    let monitor_event_query = format!("source:alert {}", input.monitor_id);
    let params = ListEventsOptionalParams::default()
        .filter_from(time_range.from.to_rfc3339())
        .filter_to(time_range.to.to_rfc3339())
        .filter_query(monitor_event_query)
        .sort(EventsSort::TIMESTAMP_DESCENDING)
        .page_limit(50);

    match events::api::list_events(api_key, app_key, params).await {
        Ok(response) => {
            let events = response.data.unwrap_or_default();
            if events.is_empty() {
                if raw {
                    println!(
                        "{}",
                        serde_json::json!({"section": "monitor_events", "events": []})
                    );
                } else {
                    println!("(no monitor events found in this time range)");
                }
            } else {
                for event in &events {
                    let outer_attrs = event.attributes.as_ref();
                    let inner_attrs = outer_attrs.and_then(|a| a.attributes.as_ref());

                    let timestamp = outer_attrs
                        .and_then(|a| a.timestamp)
                        .map(|t| t.with_timezone(&Local).format("%Y-%m-%d %H:%M PT").to_string())
                        .unwrap_or_else(|| "?".to_string());
                    let title = inner_attrs
                        .and_then(|a| a.title.clone())
                        .unwrap_or_default();
                    let status = inner_attrs
                        .and_then(|a| a.status.as_ref())
                        .map(|s| format!("{:?}", s))
                        .unwrap_or_else(|| "?".to_string());

                    if raw {
                        println!(
                            "{}",
                            serde_json::json!({
                                "section": "monitor_events",
                                "timestamp": outer_attrs.and_then(|a| a.timestamp).map(|t| t.to_rfc3339()),
                                "status": status,
                                "title": title,
                            })
                        );
                    } else {
                        println!("{}  {}  {}", timestamp, status, title);
                    }
                }
            }
        }
        Err(err) => {
            if raw {
                println!(
                    "{}",
                    serde_json::json!({"section": "monitor_events", "error": err.to_string()})
                );
            } else {
                eprintln!("Failed to fetch monitor events: {}", err);
            }
        }
    }

    Ok(())
}

async fn print_trigger_event(api_key: &str, app_key: &str, event_id: i64, raw: bool) {
    match events::api::get_event(api_key, app_key, event_id).await {
        Ok(response) => {
            if let Some(event) = response.event {
                if raw {
                    if let Ok(json) = serde_json::to_string(&event) {
                        println!("{}", json);
                    }
                    return;
                }

                println!("## Trigger Event");

                let timestamp = event
                    .date_happened
                    .map(|epoch_s| {
                        Utc.timestamp_opt(epoch_s, 0)
                            .unwrap()
                            .with_timezone(&Local)
                            .format("%Y-%m-%d %H:%M PT")
                            .to_string()
                    })
                    .unwrap_or_else(|| "?".to_string());
                let title = event.title.as_deref().unwrap_or("(no title)");
                let alert_type = event
                    .alert_type
                    .as_ref()
                    .map(|t| t.to_string())
                    .unwrap_or_else(|| "?".to_string());

                println!("Time: {}", timestamp);
                println!("Alert Type: {}", alert_type);
                println!("Title: {}", title);

                if let Some(tags) = &event.tags {
                    if !tags.is_empty() {
                        println!("Tags: {}", tags.join(", "));
                    }
                }

                if let Some(text) = &event.text {
                    if !text.is_empty() {
                        println!();
                        // Strip Datadog markdown delimiters.
                        let text = text
                            .trim_start_matches("%%% \n")
                            .trim_end_matches("\n %%%")
                            .trim();
                        for line in text.lines().take(20) {
                            println!("  {}", line);
                        }
                    }
                }
            } else {
                if raw {
                    println!(
                        "{}",
                        serde_json::json!({"section": "trigger_event", "error": "event not found"})
                    );
                } else {
                    println!("## Trigger Event");
                    println!("(event {} not found)", event_id);
                }
            }
        }
        Err(err) => {
            if raw {
                println!(
                    "{}",
                    serde_json::json!({"section": "trigger_event", "error": err.to_string()})
                );
            } else {
                eprintln!("Failed to fetch trigger event: {}", err);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // -- parse_monitor_input --

    #[test]
    fn parse_bare_numeric_id() {
        let input = parse_monitor_input("51915671").unwrap();
        assert_eq!(input.monitor_id, 51915671);
        assert!(input.time_range.is_none());
        assert!(input.event_id.is_none());
    }

    #[test]
    fn parse_url_with_all_params() {
        let url = "https://app.datadoghq.com/monitors/51915671?event_id=abc123&from_ts=1772058251000&to_ts=1772059403749&live=true";
        let input = parse_monitor_input(url).unwrap();
        assert_eq!(input.monitor_id, 51915671);
        assert_eq!(input.time_range, Some((1772058251000, 1772059403749)));
        assert_eq!(input.event_id.as_deref(), Some("abc123"));
    }

    #[test]
    fn parse_url_without_time_params() {
        let url = "https://app.datadoghq.com/monitors/12345";
        let input = parse_monitor_input(url).unwrap();
        assert_eq!(input.monitor_id, 12345);
        assert!(input.time_range.is_none());
        assert!(input.event_id.is_none());
    }

    #[test]
    fn parse_invalid_input() {
        assert!(parse_monitor_input("not-a-url-or-number").is_err());
    }

    // -- extract_metric_query --

    #[test]
    fn extract_standard_metric_alert() {
        let q = "avg(last_5m):avg:system.cpu.user{env:production} > 90";
        assert_eq!(
            extract_metric_query(q),
            Some("avg:system.cpu.user{env:production}".to_string())
        );
    }

    #[test]
    fn extract_change_monitor() {
        let q = "change(avg(last_5m),last_1h):avg:system.mem.used{*} > 100";
        assert_eq!(
            extract_metric_query(q),
            Some("avg:system.mem.used{*}".to_string())
        );
    }

    #[test]
    fn extract_with_by_clause() {
        let q = "avg(last_5m):avg:system.cpu.user{env:production} by {host} > 90";
        assert_eq!(
            extract_metric_query(q),
            Some("avg:system.cpu.user{env:production} by {host}".to_string())
        );
    }

    #[test]
    fn extract_composite_returns_none() {
        // Composite monitors use logical operators without the aggregation wrapper.
        let q = "1234 && 5678";
        assert_eq!(extract_metric_query(q), None);
    }

    #[test]
    fn extract_log_monitor_returns_none() {
        // Log monitors use "logs(...)" syntax but the inner query isn't a metric.
        let q = r#"logs("service:web status:error").index("*").rollup("count").last("5m") > 100"#;
        // This has parens but the first paren-balanced `)` is followed by `.`, not `:`.
        assert_eq!(extract_metric_query(q), None);
    }

    // -- print_monitor_summary smoke test --

    #[test]
    fn print_monitor_summary_does_not_panic() {
        let monitor = Monitor::new("avg(last_5m):avg:system.cpu.user{*} > 90".to_string(), MonitorType::METRIC_ALERT);
        print_monitor_summary(&monitor, false);
        print_monitor_summary(&monitor, true);
    }
}
