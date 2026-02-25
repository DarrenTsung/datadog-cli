mod dashboard;
mod events;
mod metrics;
mod notebooks;

use anyhow::{anyhow, Context};
use datadog_utils::TimeRange;
use serde_derive::{Deserialize, Serialize};
use serde_json::Value;
use std::str::FromStr;
use std::time::Duration;
use structopt::StructOpt;
use tokio::time::sleep;

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
    /// Collect logs from the Datadog log API.
    Logs(LogsOpt),
    /// Manage Datadog notebooks from markdown files.
    Notebooks(notebooks::NotebooksOpt),
    /// Query Datadog metrics.
    Metrics(metrics::MetricsOpt),
    /// Search Datadog events.
    Events(events::EventsOpt),
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
fn resolve_path<'a>(value: &'a Value, path: &str) -> &'a Value {
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

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let opt = Opt::from_args();

    match opt.cmd {
        Command::Events(e_opt) => events::run_events(&opt.dd_api_key, &opt.dd_application_key, e_opt).await,
        Command::Logs(logs_opt) => run_logs(&opt.dd_api_key, &opt.dd_application_key, logs_opt).await,
        Command::Metrics(m_opt) => metrics::run_metrics(&opt.dd_api_key, &opt.dd_application_key, m_opt).await,
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
            if opt.all_columns {
                println!("{}", serde_json::to_string(attrs)?);
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

    Ok(())
}
