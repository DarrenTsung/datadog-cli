use anyhow::Context;
use datadog_api_client::datadog::{APIKey, Configuration};
use datadog_api_client::datadogV2::api::api_metrics::{
    ListTagsByMetricNameOptionalParams, MetricsAPI as MetricsAPIV2,
};
use datadog_api_client::datadogV2::model::{
    MetricAllTagsResponse, MetricsDataSource, MetricsTimeseriesQuery, QueryFormula,
    TimeseriesFormulaQueryRequest, TimeseriesFormulaQueryResponse, TimeseriesFormulaRequest,
    TimeseriesFormulaRequestAttributes, TimeseriesFormulaRequestType, TimeseriesQuery,
};

use super::NamedQuery;

fn make_configuration(api_key: &str, app_key: &str) -> Configuration {
    let mut config = Configuration::new();
    config.set_auth_key(
        "apiKeyAuth",
        APIKey {
            key: api_key.to_string(),
            prefix: String::new(),
        },
    );
    config.set_auth_key(
        "appKeyAuth",
        APIKey {
            key: app_key.to_string(),
            prefix: String::new(),
        },
    );
    config
}

pub async fn query_timeseries_formula(
    api_key: &str,
    app_key: &str,
    from_ms: i64,
    to_ms: i64,
    named_queries: &[NamedQuery],
    formulas: &[String],
    interval_ms: Option<i64>,
) -> anyhow::Result<TimeseriesFormulaQueryResponse> {
    let config = make_configuration(api_key, app_key);
    let api = MetricsAPIV2::with_config(config);

    let queries: Vec<TimeseriesQuery> = named_queries
        .iter()
        .map(|nq| {
            TimeseriesQuery::MetricsTimeseriesQuery(Box::new(
                MetricsTimeseriesQuery::new(MetricsDataSource::METRICS, nq.query.clone())
                    .name(nq.name.clone()),
            ))
        })
        .collect();

    let formula_objs: Vec<QueryFormula> = formulas
        .iter()
        .map(|f| QueryFormula::new(f.clone()))
        .collect();

    let mut attrs =
        TimeseriesFormulaRequestAttributes::new(from_ms, queries, to_ms)
            .formulas(formula_objs);
    if let Some(ms) = interval_ms {
        attrs = attrs.interval(ms);
    }

    let body = TimeseriesFormulaQueryRequest::new(TimeseriesFormulaRequest::new(
        attrs,
        TimeseriesFormulaRequestType::TIMESERIES_REQUEST,
    ));

    api.query_timeseries_data(body)
        .await
        .context("Failed to query timeseries formula")
}

pub async fn list_tags_by_metric_name(
    api_key: &str,
    app_key: &str,
    metric_name: &str,
) -> anyhow::Result<MetricAllTagsResponse> {
    let config = make_configuration(api_key, app_key);
    let api = MetricsAPIV2::with_config(config);

    let params = ListTagsByMetricNameOptionalParams::default()
        .filter_include_tag_values(true);

    api.list_tags_by_metric_name(metric_name.to_string(), params)
        .await
        .context("Failed to list tags for metric")
}
