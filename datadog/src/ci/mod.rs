pub(crate) mod api;

use crate::{resolve_path, SortOrder};
use datadog_api_client::datadogV2::api::api_ci_visibility_tests::ListCIAppTestEventsOptionalParams;
use datadog_api_client::datadogV2::model::{
    CIAppAggregationFunction, CIAppCompute, CIAppComputeType, CIAppSort,
    CIAppTestsAggregateRequest, CIAppTestsGroupBy, CIAppTestsQueryFilter,
};
use datadog_utils::TimeRange;
use serde_json::Value;
use structopt::StructOpt;

#[derive(StructOpt, Debug)]
pub struct CiOpt {
    /// Time range (e.g., "last 7 days").
    #[structopt(long)]
    time_range: TimeRange,

    /// CI test search query (e.g., '@test.status:fail @git.branch:main').
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

    /// Comma-separated list of columns to include in output. Use @ as
    /// shorthand for attributes (e.g. @test.name -> attributes.test.name).
    #[structopt(long, default_value = "@test.status,@test.name,@test.service,@git.branch")]
    columns: String,

    /// Additional columns to append to --columns.
    #[structopt(long)]
    add_columns: Option<String>,

    /// Output all attributes for each CI test event instead of selected columns.
    #[structopt(long)]
    all_columns: bool,

    /// Group results by facet(s) and return counts instead of individual events.
    /// Comma-separated (e.g., '@test.status' or '@test.status,@test.service').
    #[structopt(long)]
    group_by: Option<String>,

    /// Bypass the --limit <= 100 guard.
    #[structopt(long)]
    force: bool,
}

const LIMIT_GUARD: &str = "\
Error: --limit is required and must be <= 100 (or use --force to bypass).

You are fetching too much data. Consider a more targeted approach:
  - Use a narrower --time-range (e.g. \"last 1 hour\" instead of \"last 7 days\")
  - Add filters to --query to reduce the result set
  - Use --limit with a small value (e.g. 10-20) and inspect before fetching more
  - Use --columns to reduce output size per row";

pub async fn run_ci(api_key: &str, app_key: &str, opt: CiOpt) -> anyhow::Result<()> {
    if let Some(ref group_by) = opt.group_by {
        return run_aggregate(api_key, app_key, &opt, group_by).await;
    }

    if !opt.force && !opt.limit.is_some_and(|l| l <= 100) {
        return Err(anyhow::anyhow!(LIMIT_GUARD));
    }

    let mut columns_str = opt.columns.clone();
    if let Some(ref add) = opt.add_columns {
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

    let sort = match opt.sort_by {
        SortOrder::Newest => CIAppSort::TIMESTAMP_DESCENDING,
        SortOrder::Oldest => CIAppSort::TIMESTAMP_ASCENDING,
    };

    let page_limit = opt.limit.map(|l| l.min(1000)).unwrap_or(1000) as i32;

    let mut params = ListCIAppTestEventsOptionalParams::default()
        .filter_from(opt.time_range.from)
        .filter_to(opt.time_range.to)
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
        let response = api::list_ci_test_events(api_key, app_key, params.clone()).await?;

        let events = response.data.unwrap_or_default();
        if events.is_empty() {
            if total_processed == 0 {
                eprintln!("No CI test events found.");
            }
            break;
        }

        for event in &events {
            if opt.limit.is_some_and(|l| total_processed >= l) {
                break;
            }

            let event_json = serde_json::to_value(event)?;
            let attrs = &event_json["attributes"];

            if opt.all_columns {
                println!("{}", serde_json::to_string(attrs)?);
            } else {
                let mut obj = serde_json::Map::new();
                for (col_name, col_path) in &columns {
                    let val = resolve_path(attrs, col_path);
                    obj.insert(col_name.clone(), val.clone());
                }
                println!("{}", serde_json::to_string(&Value::Object(obj))?);
            }
            total_processed += 1;
        }

        let next_cursor = response
            .meta
            .and_then(|m| m.page)
            .and_then(|p| p.after);

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

async fn run_aggregate(
    api_key: &str,
    app_key: &str,
    opt: &CiOpt,
    group_by_str: &str,
) -> anyhow::Result<()> {
    let group_by: Vec<CIAppTestsGroupBy> = group_by_str
        .split(',')
        .map(|facet| CIAppTestsGroupBy::new(facet.trim().to_string()))
        .collect();

    let compute = vec![CIAppCompute::new(CIAppAggregationFunction::COUNT)
        .type_(CIAppComputeType::TOTAL)];

    let mut filter = CIAppTestsQueryFilter::new()
        .from(opt.time_range.from.to_rfc3339())
        .to(opt.time_range.to.to_rfc3339());

    if let Some(ref query) = opt.query {
        filter = filter.query(query.clone());
    }

    let body = CIAppTestsAggregateRequest::new()
        .compute(compute)
        .filter(filter)
        .group_by(group_by);

    let response = api::aggregate_ci_test_events(api_key, app_key, body).await?;

    let buckets = response
        .data
        .and_then(|d| d.buckets)
        .unwrap_or_default();

    if buckets.is_empty() {
        eprintln!("No results.");
        return Ok(());
    }

    for bucket in &buckets {
        let mut obj = serde_json::Map::new();

        if let Some(ref by) = bucket.by {
            for (key, val) in by {
                obj.insert(key.clone(), val.clone());
            }
        }

        if let Some(ref computes) = bucket.computes {
            for (key, val) in computes {
                let v = serde_json::to_value(val)?;
                obj.insert(key.clone(), v);
            }
        }

        println!("{}", serde_json::to_string(&Value::Object(obj))?);
    }

    Ok(())
}
