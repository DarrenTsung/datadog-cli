use anyhow::Context;
use datadog_api_client::datadog::{APIKey, Configuration};
use datadog_api_client::datadogV1::api::api_metrics::MetricsAPI;
use datadog_api_client::datadogV1::model::MetricsQueryResponse;

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
    let api = MetricsAPI::with_config(config);

    api.query_metrics(from_ms, to_ms, query.to_string())
        .await
        .context("Failed to query metrics")
}
