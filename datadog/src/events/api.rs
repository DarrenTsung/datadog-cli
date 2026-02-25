use anyhow::anyhow;
use datadog_api_client::datadog::{self, APIKey, Configuration};
use datadog_api_client::datadogV2::api::api_events::{EventsAPI, ListEventsOptionalParams};
use datadog_api_client::datadogV2::model::EventsListResponse;

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

pub async fn list_events(
    api_key: &str,
    app_key: &str,
    params: ListEventsOptionalParams,
) -> anyhow::Result<EventsListResponse> {
    let config = make_configuration(api_key, app_key);
    let api = EventsAPI::with_config(config);

    api.list_events(params).await.map_err(|e| match &e {
        datadog::Error::ResponseError(resp) => {
            anyhow!("Failed to list events ({}): {}", resp.status, resp.content)
        }
        _ => anyhow!(e).context("Failed to list events"),
    })
}
