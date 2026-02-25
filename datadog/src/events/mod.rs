pub(crate) mod api;

use crate::SortOrder;
use datadog_api_client::datadogV2::api::api_events::ListEventsOptionalParams;
use datadog_api_client::datadogV2::model::EventsSort;
use datadog_utils::TimeRange;
use serde_json::Value;
use structopt::StructOpt;

#[derive(StructOpt, Debug)]
pub struct EventsOpt {
    /// Time range (e.g., "last 1 hour").
    #[structopt(long)]
    time_range: TimeRange,

    /// Event search query (e.g., "source:deploy").
    #[structopt(long)]
    query: Option<String>,

    /// Sort order: "newest" (default) or "oldest".
    #[structopt(long = "sort-by", default_value = "newest")]
    sort_by: SortOrder,

    /// Max events to return.
    #[structopt(long)]
    limit: Option<usize>,

    /// Pagination cursor to resume a previous search.
    #[structopt(long)]
    cursor: Option<String>,

    /// Comma-separated list of tags to show (whitelist mode). When omitted,
    /// all tags except common infrastructure noise are shown automatically.
    #[structopt(long)]
    tags: Option<String>,

    /// Additional tags to include (useful for adding back excluded infra tags).
    #[structopt(long)]
    add_tags: Option<String>,

    /// Comma-separated tags to exclude from output.
    #[structopt(long)]
    remove_tags: Option<String>,

    /// Output all event attributes and the full tags array.
    #[structopt(long)]
    all_tags: bool,

    /// Bypass the --limit <= 100 guard.
    #[structopt(long)]
    force: bool,
}

/// Returns true if this tag key should be excluded by default.
fn is_infra_tag(key: &str) -> bool {
    // Exact key matches.
    const EXCLUDED_KEYS: &[&str] = &[
        "availability-zone",
        "canary_role",
        "cloud_provider",
        "cost_owner",
        "image",
        "instance-type",
        "kernel",
        "name",
        "orch_cluster_id",
        "pod_name",
        "pod_phase",
        "region",
        "zone",
    ];
    // Prefix matches (covers families like aws.0, aws_account, kube_namespace, etc.).
    const EXCLUDED_PREFIXES: &[&str] = &[
        "aws",
        "eks",
        "karpenter",
        "kube_",
        "kubernetes.io/",
        "security-group",
    ];
    EXCLUDED_KEYS.contains(&key)
        || EXCLUDED_PREFIXES.iter().any(|p| key.starts_with(p))
}

/// Extract the key portion of a "key:value" tag.
fn tag_key(tag: &str) -> &str {
    tag.split_once(':').map(|(k, _)| k).unwrap_or(tag)
}

/// Extract a value from a tags list (e.g. "env" from "env:production").
fn extract_tag(tags: &[String], key: &str) -> Option<String> {
    let prefix = format!("{key}:");
    tags.iter()
        .find_map(|tag| tag.strip_prefix(&prefix).map(|v| v.to_string()))
}

const LIMIT_GUARD: &str = "\
Error: --limit is required and must be <= 100 (or use --force to bypass).

You are fetching too much data. Consider a more targeted approach:
  - Use a narrower --time-range (e.g. \"last 15 minutes\" instead of \"last 1 day\")
  - Add filters to --query to reduce the result set
  - Use --limit with a small value (e.g. 10-20) and inspect before fetching more
  - Use --tags / --remove-tags to reduce output size per row";

pub async fn run_events(api_key: &str, app_key: &str, opt: EventsOpt) -> anyhow::Result<()> {
    if !opt.force && !opt.limit.is_some_and(|l| l <= 100) {
        return Err(anyhow::anyhow!(LIMIT_GUARD));
    }

    let sort = match opt.sort_by {
        SortOrder::Newest => EventsSort::TIMESTAMP_DESCENDING,
        SortOrder::Oldest => EventsSort::TIMESTAMP_ASCENDING,
    };

    // Parse explicit tag whitelist if provided.
    let whitelist: Option<Vec<String>> = opt.tags.as_ref().map(|t| {
        let mut keys: Vec<String> = t.split(',').map(|s| s.trim().to_string()).collect();
        if let Some(ref add) = opt.add_tags {
            keys.extend(add.split(',').map(|s| s.trim().to_string()));
        }
        keys
    });

    // Tags to force-include even if they'd normally be excluded (--add-tags
    // without --tags).
    let force_include: Vec<String> = opt
        .add_tags
        .as_ref()
        .map(|t| t.split(',').map(|s| s.trim().to_string()).collect())
        .unwrap_or_default();

    let remove: Vec<String> = opt
        .remove_tags
        .as_ref()
        .map(|t| t.split(',').map(|s| s.trim().to_string()).collect())
        .unwrap_or_default();

    let page_limit = opt.limit.map(|l| l.min(1000)).unwrap_or(1000) as i32;

    let mut params = ListEventsOptionalParams::default()
        .filter_from(opt.time_range.from.to_rfc3339())
        .filter_to(opt.time_range.to.to_rfc3339())
        .sort(sort)
        .page_limit(page_limit);

    if let Some(ref query) = opt.query {
        params = params.filter_query(query.clone());
    }

    if let Some(ref cursor) = opt.cursor {
        params = params.page_cursor(cursor.clone());
    }

    let mut total_processed = 0;
    loop {
        let response = api::list_events(api_key, app_key, params.clone()).await?;

        let events = response.data.unwrap_or_default();
        if events.is_empty() {
            if total_processed == 0 {
                eprintln!("No events found.");
            }
            break;
        }

        for event in &events {
            if opt.limit.is_some_and(|l| total_processed >= l) {
                break;
            }

            let outer_attrs = event.attributes.as_ref();
            let inner_attrs = outer_attrs.and_then(|a| a.attributes.as_ref());

            if opt.all_tags {
                let output = serde_json::json!({
                    "timestamp": outer_attrs.and_then(|a| a.timestamp).map(|t| t.to_rfc3339()),
                    "title": inner_attrs.and_then(|a| a.title.clone()),
                    "source": inner_attrs.and_then(|a| a.source_type_name.clone()),
                    "status": inner_attrs.and_then(|a| a.status.as_ref()).and_then(|s| serde_json::to_value(s).ok()),
                    "priority": inner_attrs.and_then(|a| a.priority.as_ref().and_then(|p| p.as_ref())).and_then(|p| serde_json::to_value(p).ok()),
                    "tags": outer_attrs.and_then(|a| a.tags.clone()),
                    "message": outer_attrs.and_then(|a| a.message.clone()),
                });
                println!("{}", serde_json::to_string(&output)?);
            } else {
                let tags_list = outer_attrs.and_then(|a| a.tags.as_ref());

                let mut obj = serde_json::Map::new();
                obj.insert("timestamp".to_string(), match outer_attrs.and_then(|a| a.timestamp) {
                    Some(t) => Value::String(t.to_rfc3339()),
                    None => Value::Null,
                });
                obj.insert("title".to_string(), match inner_attrs.and_then(|a| a.title.clone()) {
                    Some(t) => Value::String(t),
                    None => Value::Null,
                });
                obj.insert("message".to_string(), match outer_attrs.and_then(|a| a.message.clone()) {
                    Some(m) => Value::String(m),
                    None => Value::Null,
                });

                if let Some(tags) = tags_list {
                    if let Some(ref keys) = whitelist {
                        // Whitelist mode: only show specified tags.
                        for key in keys {
                            if remove.iter().any(|r| r == key) {
                                continue;
                            }
                            let val = extract_tag(tags, key)
                                .map(Value::String)
                                .unwrap_or(Value::Null);
                            obj.insert(key.clone(), val);
                        }
                    } else {
                        // Exclusion mode: show all tags except infra noise.
                        for tag in tags {
                            let key = tag_key(tag);
                            if remove.iter().any(|r| r == key) {
                                continue;
                            }
                            if is_infra_tag(key) && !force_include.iter().any(|f| f == key) {
                                continue;
                            }
                            if let Some(val) = extract_tag(tags, key) {
                                obj.insert(key.to_string(), Value::String(val));
                            }
                        }
                    }
                }

                println!("{}", serde_json::to_string(&Value::Object(obj))?);
            }
            total_processed += 1;
        }

        let next_cursor = response.meta.and_then(|m| m.page).and_then(|p| p.after);

        eprintln!(
            "Finished processing page. Total processed: {}. Next cursor: {:?}.",
            total_processed, next_cursor,
        );

        if opt.limit.is_some_and(|l| total_processed >= l) {
            break;
        }

        if let Some(next_cursor) = next_cursor {
            params.page_cursor = Some(next_cursor);
        } else {
            break;
        }
    }

    Ok(())
}
