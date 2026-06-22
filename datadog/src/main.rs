mod ci;
mod dashboard;
mod events;
mod metrics;
mod monitors;
mod notebooks;

use anyhow::{anyhow, Context};
use datadog_utils::TimeRange;
use serde_derive::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::hash_map::DefaultHasher;
use std::collections::HashSet;
use std::hash::{Hash, Hasher};
use std::str::FromStr;
use std::time::Duration;
use structopt::StructOpt;
use tokio::time::sleep;
use url::Url;

#[derive(StructOpt, Debug)]
#[structopt(name = "datadog", about = "A CLI for interacting with the Datadog API.")]
struct Opt {
    /// The Datadog API key, see: https://app.datadoghq.com/organization-settings/api-keys
    #[structopt(long, env = "DD_API_KEY", hide_env_values = true)]
    dd_api_key: String,

    /// The Datadog application key, see: https://app.datadoghq.com/organization-settings/application-keys
    #[structopt(long, env = "DD_APPLICATION_KEY", hide_env_values = true)]
    dd_application_key: String,

    #[structopt(subcommand)]
    cmd: Command,
}

#[derive(StructOpt, Debug)]
enum Command {
    /// Query CI Visibility test events.
    Ci(ci::CiOpt),
    /// Collect logs from the Datadog log API.
    Logs(LogsOpt),
    /// Manage Datadog notebooks from markdown files.
    Notebooks(notebooks::NotebooksOpt),
    /// Query Datadog metrics.
    Metrics(metrics::MetricsOpt),
    /// Search Datadog events.
    Events(events::EventsOpt),
    /// Inspect Datadog monitors: metadata, underlying metrics, and events.
    Monitors(monitors::MonitorsOpt),
    /// Unfurl a Datadog URL — show widget info and download the snapshot image.
    Unfurl(dashboard::UnfurlOpt),
}

#[derive(StructOpt, Debug)]
struct LogsOpt {
    /// The datadog url of the log search page. If this is present, it is used to derive
    /// the time range and query!
    #[structopt(long)]
    datadog_url: Option<String>,

    /// Time-range of logs to search through. Eg. "last 5 days". You can also
    /// provide a datadog url!
    ///
    /// You must provide this if datadog_url is not provided.
    #[structopt(long)]
    time_range: Option<TimeRange>,

    /// A datadog log query like: "env:production @file.key:YVAxndRJlWC4GoOoGmo8pu".
    ///
    /// You must provide this if datadog_url is not provided.
    #[structopt(long)]
    query: Option<String>,

    /// (Optional) The cursor to provide for the initial API call. Use this to resume pagination after a search was cut-off.
    #[structopt(long)]
    cursor: Option<String>,

    /// Maximum number of log rows to output.
    #[structopt(long)]
    limit: Option<usize>,

    /// Comma-separated list of columns to include in output. Use @ as
    /// shorthand for attributes. (e.g. @version -> attributes.version).
    #[structopt(long, default_value = "timestamp,service,message")]
    columns: String,

    /// Additional columns to append to --columns.
    #[structopt(long)]
    add_columns: Option<String>,

    /// Output all attributes for each log entry instead of selected columns.
    #[structopt(long)]
    all_columns: bool,

    /// Sort order for log entries: "newest" (default) or "oldest".
    #[structopt(long = "sort-by", default_value = "newest")]
    sort_by: SortOrder,

    /// Bypass the --limit <= 100 guard.
    #[structopt(long)]
    force: bool,

    /// For each log, also emit a short unique `id` handle and a `url` that
    /// deep-links to that exact log (highlighted) in the Datadog UI. Use the
    /// `id` to pick a specific log to share; its `url` opens it directly.
    #[structopt(long = "deep-links")]
    deep_links: bool,

    /// Minimum length of the short `id` handle emitted by --deep-links. The
    /// handle grows beyond this only when needed to keep every id unique.
    #[structopt(long = "id-min-len", default_value = "6")]
    id_min_len: usize,
}

#[derive(Debug, Clone)]
pub enum SortOrder {
    Newest,
    Oldest,
}

impl FromStr for SortOrder {
    type Err = anyhow::Error;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "newest" => Ok(SortOrder::Newest),
            "oldest" => Ok(SortOrder::Oldest),
            other => Err(anyhow!("Invalid sort order '{}': expected 'newest' or 'oldest'", other)),
        }
    }
}

impl SortOrder {
    fn to_api_value(&self) -> &'static str {
        match self {
            SortOrder::Newest => "-timestamp",
            SortOrder::Oldest => "timestamp",
        }
    }
}

#[derive(Serialize)]
struct SearchLogRequest {
    filter: Filter,
    sort: String,
    page: Page,
}

#[derive(Serialize)]
struct Filter {
    from: String,
    to: String,
    query: String,
    storage_tier: String,
}

#[derive(Serialize)]
struct Page {
    cursor: Option<String>,
    limit: usize,
}

#[derive(Deserialize)]
struct SearchLogResponse {
    data: Vec<serde_json::Value>,
    meta: Option<serde_json::Value>,
}

/// Resolve a dot-separated path like "attributes.version" into a nested JSON value.
pub fn resolve_path<'a>(value: &'a Value, path: &str) -> &'a Value {
    let mut current = value;
    for segment in path.split('.') {
        current = &current[segment];
    }
    current
}

/// Extract a value from the tags array (e.g. "pod_name" from "pod_name:some-value").
fn extract_tag(tags: &Value, key: &str) -> Option<String> {
    let prefix = format!("{key}:");
    tags.as_array()?.iter().find_map(|tag| {
        let s = tag.as_str()?;
        s.strip_prefix(&prefix).map(|v| v.to_string())
    })
}

/// Datadog log event ids are long opaque strings (the value the UI's `event=`
/// param wants). To give the model/human a short handle to pick a log by, we
/// hash the full id into a stable 16-char hex string and then truncate it to
/// the shortest length that keeps every handle in the result set unique.
fn hash_event_id(full_id: &str) -> String {
    let mut hasher = DefaultHasher::new();
    full_id.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

/// Find the shortest prefix length (>= `min_len`) at which every hash in
/// `hashes` is unique. Falls back to the longest available hash length if even
/// the full hashes collide (only possible with duplicate event ids).
fn unique_prefix_len(hashes: &[String], min_len: usize) -> usize {
    let max_len = hashes.iter().map(|h| h.len()).max().unwrap_or(min_len);
    let min_len = min_len.max(1).min(max_len);
    for len in min_len..=max_len {
        let mut seen = HashSet::new();
        if hashes.iter().all(|h| seen.insert(&h[..len.min(h.len())])) {
            return len;
        }
    }
    max_len
}

/// Build a Datadog logs deep-link that opens with one specific log highlighted.
/// `cols` is a comma-separated list of column names shown in the stream view.
fn build_log_deeplink(
    query: &str,
    from_ms: i64,
    to_ms: i64,
    cols: &str,
    event_id: &str,
) -> String {
    let mut url = Url::parse("https://app.datadoghq.com/logs").expect("valid base url");
    url.query_pairs_mut()
        .append_pair("query", query)
        .append_pair("agg_m", "count")
        .append_pair("agg_m_source", "base")
        .append_pair("agg_t", "count")
        .append_pair("cols", cols)
        .append_pair("fromUser", "true")
        .append_pair("messageDisplay", "inline")
        .append_pair("refresh_mode", "sliding")
        .append_pair("storage", "flex_tier")
        .append_pair("stream_sort", "desc")
        .append_pair("viz", "stream")
        .append_pair("from_ts", &from_ms.to_string())
        .append_pair("to_ts", &to_ms.to_string())
        .append_pair("live", "false")
        .append_pair("event", event_id);
    url.to_string()
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let opt = Opt::from_args();

    match opt.cmd {
        Command::Ci(ci_opt) => ci::run_ci(&opt.dd_api_key, &opt.dd_application_key, ci_opt).await,
        Command::Events(e_opt) => events::run_events(&opt.dd_api_key, &opt.dd_application_key, e_opt).await,
        Command::Logs(logs_opt) => run_logs(&opt.dd_api_key, &opt.dd_application_key, logs_opt).await,
        Command::Metrics(m_opt) => metrics::run_metrics(&opt.dd_api_key, &opt.dd_application_key, m_opt).await,
        Command::Monitors(mon_opt) => monitors::run_monitors(&opt.dd_api_key, &opt.dd_application_key, mon_opt).await,
        Command::Notebooks(nb_opt) => notebooks::run_notebooks(&opt.dd_api_key, &opt.dd_application_key, nb_opt).await,
        Command::Unfurl(unfurl_opt) => dashboard::run_unfurl(&opt.dd_api_key, &opt.dd_application_key, unfurl_opt).await,
    }
}

const LIMIT_GUARD: &str = "\
Error: --limit is required and must be <= 100 (or use --force to bypass).

You are fetching too much data. Consider a more targeted approach:
  - Use a narrower --time-range (e.g. \"last 15 minutes\" instead of \"last 1 day\")
  - Add filters to --query to reduce the result set
  - Use --limit with a small value (e.g. 10-20) and inspect before fetching more
  - Use --columns / --tags to reduce output size per row";

async fn run_logs(dd_api_key: &str, dd_application_key: &str, opt: LogsOpt) -> anyhow::Result<()> {
    if !opt.force && !opt.limit.is_some_and(|l| l <= 100) {
        return Err(anyhow!(LIMIT_GUARD));
    }

    let mut columns_str = opt.columns;
    if let Some(add) = opt.add_columns {
        columns_str = format!("{columns_str},{add}");
    }

    let columns: Vec<(String, String)> = columns_str
        .split(',')
        .map(|col| {
            let col = col.trim().to_string();
            let path = if let Some(rest) = col.strip_prefix('@') {
                format!("attributes.{rest}")
            } else {
                col.clone()
            };
            (col, path)
        })
        .collect();

    let client = reqwest::Client::new();
    let (time_range, query) = if let Some(datadog_url) = opt.datadog_url {
        let query = datadog_utils::query_from_url(&datadog_url)
            .context("Could not parse query from --datadog-url")?;
        // If there is a query and we can't find a time range, it's likely that the
        // time range is the last 15 minutes (the default for datadog).
        let time_range = match TimeRange::from_str(&datadog_url) {
            Ok(time_range) => time_range,
            Err(_err) => {
                eprintln!("No time-range found in datadog url, defaulting to querying the last 15 minutes!");
                TimeRange::from_str("last 15 minutes").expect("works")
            }
        };
        (time_range, query)
    } else {
        let time_range = if let Some(time_range) = opt.time_range {
            time_range
        } else {
            return Err(anyhow!(
                "If --datadog-url is not present, then --time-range must be provided!"
            ));
        };
        let query = if let Some(query) = opt.query {
            query
        } else {
            return Err(anyhow!(
                "If --datadog-url is not present, then --query must be provided!"
            ));
        };
        (time_range, query)
    };

    // Validate the query syntax before making the API call.
    let tips = datadog_utils::validate_query(&query);
    if !tips.is_empty() {
        let mut msg = String::from("Query validation error — tips for fixing:\n");
        for tip in &tips {
            msg.push_str(&format!("  - {}\n", tip));
        }
        return Err(anyhow!(msg));
    }

    // Captured before `query` is moved into the request — needed to build the
    // per-log deep-link URLs when --deep-links is set.
    let from_ms = time_range.from.timestamp_millis();
    let to_ms = time_range.to.timestamp_millis();
    let query_for_url = query.clone();
    let url_cols = {
        let cols: Vec<&str> = columns
            .iter()
            .map(|(name, _)| name.as_str())
            .filter(|c| *c != "timestamp" && *c != "message")
            .collect();
        if cols.is_empty() {
            "host,service".to_string()
        } else {
            cols.join(",")
        }
    };
    // In --deep-links mode rows are buffered so the short `id` handles can be
    // made unique across the whole result set before printing.
    let mut deep_rows: Vec<(String, serde_json::Map<String, Value>)> = Vec::new();

    let mut request = SearchLogRequest {
        filter: Filter {
            from: time_range.from.to_rfc3339(),
            to: time_range.to.to_rfc3339(),
            query,
            storage_tier: "flex".to_string(),
        },
        sort: opt.sort_by.to_api_value().to_string(),
        page: Page {
            cursor: opt.cursor,
            limit: 1_000,
        },
    };

    let mut total_processed = 0;
    let mut current_rate_limit_count = 0;
    loop {
        let response = client
            .post("https://api.datadoghq.com/api/v2/logs/events/search")
            .header("DD-API-KEY", dd_api_key)
            .header("DD-APPLICATION-KEY", dd_application_key)
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .json(&request)
            .send()
            .await?;

        let response_text = response.text().await?;
        if response_text.contains("Rate limit") {
            // Because Datadog has multiple rate limits (3 requests in 10 seconds, 1080 requests in an hour)
            // we'll try again after a few seconds.
            current_rate_limit_count += 1;
            if current_rate_limit_count < 10 {
                eprintln!("Retrying in a couple seconds after encountering a rate limit!");
                sleep(Duration::from_secs(2)).await;
                continue;
            } else {
                eprintln!("Exiting because we encountered rate limit too many times in a row!\n\nLast cursor was: '{:?}'. Provide this as an argument (--cursor) when retrying to resume log collection.\n\nRaw Text:\n{}", request.page.cursor, response_text);
                break;
            }
        } else {
            current_rate_limit_count = 0;
        }

        let response = match serde_json::from_str::<SearchLogResponse>(&response_text)
            .with_context(|| format!("Failed to parse response text: {:?}", response_text))
        {
            Ok(response) => response,
            Err(err) => {
                // Contains some sort of 5xx, let's retry.
                if response_text.contains("code\":5") {
                    eprintln!("Retrying in a bit after encountering a 5xx!");
                    sleep(Duration::from_secs(1)).await;
                    continue;
                } else if response_text.contains("504 Gateway") {
                    eprintln!("Retrying in a bit after encountering a 504!");
                    sleep(Duration::from_secs(1)).await;
                    continue;
                } else if response_text.contains("429 TOO MANY REQUESTS") {
                    eprintln!("Retrying in a bit after encountering a 429!");
                    sleep(Duration::from_secs(1)).await;
                    continue;
                } else {
                    return Err(err);
                }
            }
        };

        for log in &response.data {
            if opt.limit.is_some_and(|l| total_processed >= l) {
                break;
            }
            let attrs = &log["attributes"];
            let tags = &attrs["tags"];

            // Build the row object (all attributes, or the selected columns).
            let mut obj = if opt.all_columns {
                attrs.as_object().cloned().unwrap_or_default()
            } else {
                let mut obj = serde_json::Map::new();
                for (col_name, col_path) in &columns {
                    let val = resolve_path(attrs, col_path);
                    if val.is_null() {
                        if let Some(tag_val) = extract_tag(tags, col_name) {
                            obj.insert(col_name.clone(), Value::String(tag_val));
                        } else {
                            obj.insert(col_name.clone(), val.clone());
                        }
                    } else {
                        obj.insert(col_name.clone(), val.clone());
                    }
                }
                obj
            };

            if opt.deep_links {
                let event_id = log["id"].as_str().unwrap_or_default().to_string();
                let url =
                    build_log_deeplink(&query_for_url, from_ms, to_ms, &url_cols, &event_id);
                obj.insert("url".to_string(), Value::String(url));
                // The short `id` is assigned after all rows are collected so it
                // can be made unique across the full result set.
                deep_rows.push((event_id, obj));
            } else {
                println!("{}", serde_json::to_string(&Value::Object(obj))?);
            }
            total_processed += 1;
        }

        let next_cursor = response.meta.and_then(|meta| match &meta["page"]["after"] {
            Value::String(next_cursor) => Some(next_cursor.to_string()),
            Value::Null => None,
            unknown => panic!("Received unknown value for meta.page.after: {:?}", unknown),
        });

        eprintln!(
            "Finished processing page. Total processed: {}. Next cursor: {:?}.",
            total_processed,
            next_cursor,
        );

        if opt.limit.is_some_and(|l| total_processed >= l) {
            break;
        }

        if let Some(next_cursor) = next_cursor {
            request.page.cursor = Some(next_cursor);
        } else {
            break;
        }
    }

    // In --deep-links mode, rows were buffered so we can assign each a short
    // `id` handle that is unique across the whole result set, then print.
    if opt.deep_links {
        let hashes: Vec<String> = deep_rows
            .iter()
            .map(|(event_id, _)| hash_event_id(event_id))
            .collect();
        let len = unique_prefix_len(&hashes, opt.id_min_len);
        for ((_, mut obj), hash) in deep_rows.into_iter().zip(hashes.iter()) {
            let short = hash[..len.min(hash.len())].to_string();
            obj.insert("id".to_string(), Value::String(short));
            println!("{}", serde_json::to_string(&Value::Object(obj))?);
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_event_id_is_stable_and_hex() {
        let a = hash_event_id("AwAAAZ7bmY1713pPRgAAABhBWjdibVk3V0FBQVVYLWhuRzJ3cjFBQTE");
        let b = hash_event_id("AwAAAZ7bmY1713pPRgAAABhBWjdibVk3V0FBQVVYLWhuRzJ3cjFBQTE");
        assert_eq!(a, b, "hashing is deterministic across calls");
        assert_eq!(a.len(), 16);
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn unique_prefix_len_respects_min() {
        // Distinct hashes that differ in the first char — min length wins.
        let hashes = vec!["abcd000000000000".to_string(), "ef01000000000000".to_string()];
        assert_eq!(unique_prefix_len(&hashes, 6), 6);
    }

    #[test]
    fn unique_prefix_len_grows_on_collision() {
        // Share the first 6 chars, diverge at index 7 -> need length 7.
        let hashes = vec!["aaaaaa1000000000".to_string(), "aaaaaa2000000000".to_string()];
        assert_eq!(unique_prefix_len(&hashes, 6), 7);
    }

    #[test]
    fn unique_prefix_len_single_row_uses_min() {
        let hashes = vec!["abcdef0123456789".to_string()];
        assert_eq!(unique_prefix_len(&hashes, 6), 6);
    }

    #[test]
    fn deeplink_encodes_query_and_event() {
        let url = build_log_deeplink(
            "service:ugit @repo:*abc*",
            1781738400000,
            1781739600000,
            "host,service",
            "AwAAAevent",
        );
        assert!(url.starts_with("https://app.datadoghq.com/logs?"));
        // Query is form-encoded (space -> +, : -> %3A, @ -> %40); `*` stays raw,
        // which Datadog accepts.
        assert!(url.contains("query=service%3Augit+%40repo%3A*abc*"), "got: {url}");
        assert!(url.contains("event=AwAAAevent"));
        assert!(url.contains("from_ts=1781738400000"));
        assert!(url.contains("to_ts=1781739600000"));
        assert!(url.contains("storage=flex_tier"));
        assert!(url.contains("cols=host%2Cservice"));
    }
}
