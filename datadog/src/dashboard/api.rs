use anyhow::Context;
use datadog_api_client::datadog::{APIKey, Configuration};
use datadog_api_client::datadogV1::api::api_dashboards::DashboardsAPI;
use datadog_api_client::datadogV1::model::Dashboard;

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

pub async fn get_dashboard(
    api_key: &str,
    app_key: &str,
    dashboard_id: &str,
) -> anyhow::Result<Dashboard> {
    let config = make_configuration(api_key, app_key);
    let api = DashboardsAPI::with_config(config);

    api.get_dashboard(dashboard_id.to_string())
        .await
        .context("Failed to get dashboard")
}
