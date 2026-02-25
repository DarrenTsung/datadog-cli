use anyhow::anyhow;
use datadog_api_client::datadog::{self, APIKey, Configuration};
use datadog_api_client::datadogV1::api::api_events::EventsAPI as EventsAPIV1;
use datadog_api_client::datadogV1::model::EventResponse;
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

pub async fn get_event(
    api_key: &str,
    app_key: &str,
    event_id: i64,
) -> anyhow::Result<EventResponse> {
    let config = make_configuration(api_key, app_key);
    let api = EventsAPIV1::with_config(config);

    api.get_event(event_id).await.map_err(|e| match &e {
        datadog::Error::ResponseError(resp) => {
            anyhow!(
                "Failed to get event {} ({}): {}",
                event_id,
                resp.status,
                resp.content
            )
        }
        _ => anyhow!(e).context(format!("Failed to get event {}", event_id)),
    })
}
