use anyhow::anyhow;
use datadog_api_client::datadog::{self, APIKey, Configuration};
use datadog_api_client::datadogV2::api::api_ci_visibility_tests::{
    CIVisibilityTestsAPI, ListCIAppTestEventsOptionalParams,
};
use datadog_api_client::datadogV2::model::{
    CIAppTestEventsResponse, CIAppTestsAggregateRequest, CIAppTestsAnalyticsAggregateResponse,
};

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

pub async fn list_ci_test_events(
    api_key: &str,
    app_key: &str,
    params: ListCIAppTestEventsOptionalParams,
) -> anyhow::Result<CIAppTestEventsResponse> {
    let config = make_configuration(api_key, app_key);
    let api = CIVisibilityTestsAPI::with_config(config);

    api.list_ci_app_test_events(params)
        .await
        .map_err(|e| match &e {
            datadog::Error::ResponseError(resp) => {
                anyhow!(
                    "Failed to list CI test events ({}): {}",
                    resp.status,
                    resp.content
                )
            }
            _ => anyhow!(e).context("Failed to list CI test events"),
        })
}

pub async fn aggregate_ci_test_events(
    api_key: &str,
    app_key: &str,
    body: CIAppTestsAggregateRequest,
) -> anyhow::Result<CIAppTestsAnalyticsAggregateResponse> {
    let config = make_configuration(api_key, app_key);
    let api = CIVisibilityTestsAPI::with_config(config);

    api.aggregate_ci_app_test_events(body)
        .await
        .map_err(|e| match &e {
            datadog::Error::ResponseError(resp) => {
                anyhow!(
                    "Failed to aggregate CI test events ({}): {}",
                    resp.status,
                    resp.content
                )
            }
            _ => anyhow!(e).context("Failed to aggregate CI test events"),
        })
}
