use anyhow::Context;
use datadog_api_client::datadog::{APIKey, Configuration};
use datadog_api_client::datadogV1::api::api_metrics::MetricsAPI as MetricsAPIV1;
use datadog_api_client::datadogV1::model::MetricsQueryResponse;
use datadog_api_client::datadogV2::api::api_metrics::MetricsAPI as MetricsAPIV2;
use datadog_api_client::datadogV2::model::{
    MetricsDataSource, MetricsTimeseriesQuery, QueryFormula, TimeseriesFormulaQueryRequest,
    TimeseriesFormulaQueryResponse, TimeseriesFormulaRequest,
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

pub async fn query_metrics(
    api_key: &str,
    app_key: &str,
    from_ms: i64,
    to_ms: i64,
    query: &str,
) -> anyhow::Result<MetricsQueryResponse> {
    let config = make_configuration(api_key, app_key);
    let api = MetricsAPIV1::with_config(config);

    api.query_metrics(from_ms, to_ms, query.to_string())
        .await
        .context("Failed to query metrics")
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
