use anyhow::anyhow;
use datadog_api_client::datadog::{self, APIKey, Configuration};
use datadog_api_client::datadogV1::api::api_monitors::{GetMonitorOptionalParams, MonitorsAPI};
use datadog_api_client::datadogV1::model::Monitor;

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

pub async fn get_monitor(
    api_key: &str,
    app_key: &str,
    monitor_id: i64,
) -> anyhow::Result<Monitor> {
    let config = make_configuration(api_key, app_key);
    let api = MonitorsAPI::with_config(config);

    api.get_monitor(monitor_id, GetMonitorOptionalParams::default())
        .await
        .map_err(|e| match &e {
            datadog::Error::ResponseError(resp) => {
                anyhow!(
                    "Failed to get monitor {} ({}): {}",
                    monitor_id,
                    resp.status,
                    resp.content
                )
            }
            _ => anyhow!(e).context(format!("Failed to get monitor {}", monitor_id)),
        })
}
