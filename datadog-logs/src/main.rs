use anyhow::{anyhow, Context};
use datadog_utils::TimeRange;
use serde_derive::{Deserialize, Serialize};
use serde_json::Value;
use std::str::FromStr;
use std::time::Duration;
use structopt::StructOpt;
use tokio::time::sleep;

#[derive(StructOpt, Debug)]
#[structopt(
    name = "datadog-logs",
    about = "A tool for collecting logs from the Datadog log API."
)]
struct Opt {
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

    /// The Datadog API key, see: https://app.datadoghq.com/organization-settings/api-keys
    #[structopt(long, env = "DD_API_KEY", hide_env_values = true)]
    dd_api_key: String,

    /// The Datadog application key, see: https://app.datadoghq.com/organization-settings/application-keys
    #[structopt(long, env = "DD_APPLICATION_KEY", hide_env_values = true)]
    dd_application_key: String,

    /// (Optional) The cursor to provide for the initial API call. Use this to resume pagination after a search was cut-off.
    #[structopt(long)]
    cursor: Option<String>,

    /// Exit once this number of logs are found. Implementation is
    /// best-effort, so output may result in slightly more logs than the this value.
    #[structopt(long)]
    exit_after: Option<usize>,
}

#[derive(Serialize)]
struct SearchLogRequest {
    filter: Filter,
    page: Page,
}

#[derive(Serialize)]
struct Filter {
    from: String,
    to: String,
    query: String,
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

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let opt = Opt::from_args();

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

    let mut request = SearchLogRequest {
        filter: Filter {
            from: time_range.from.to_rfc3339(),
            to: time_range.to.to_rfc3339(),
            query,
        },
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
            .header("DD-API-KEY", opt.dd_api_key.clone())
            .header("DD-APPLICATION-KEY", opt.dd_application_key.clone())
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
            println!("{}", serde_json::to_string(&log["attributes"])?);
        }

        let next_cursor = response.meta.and_then(|meta| match &meta["page"]["after"] {
            Value::String(next_cursor) => Some(next_cursor.to_string()),
            Value::Null => None,
            unknown => panic!("Received unknown value for meta.page.after: {:?}", unknown),
        });

        total_processed += response.data.len();
        eprintln!(
            "Finished processing page with {} items. Total processed: {}. Next cursor: {:?}.",
            response.data.len(),
            total_processed,
            next_cursor,
        );

        if let Some(exit_after) = opt.exit_after {
            if total_processed > exit_after {
                eprintln!("Exiting because exit_after condition is met!");
                break;
            }
        }

        if let Some(next_cursor) = next_cursor {
            request.page.cursor = Some(next_cursor);
        } else {
            break;
        }
    }

    Ok(())
}
