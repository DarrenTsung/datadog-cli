use anyhow::{anyhow, Context};
use url::Url;

/// Helper function to get a query parameter value from a URL
pub fn get_query_param(url: &str, param_name: &str) -> anyhow::Result<String> {
    let url = Url::parse(url).context("could not parse url")?;
    for (key, value) in url.query_pairs() {
        if key == param_name {
            return Ok(value.to_string());
        }
    }
    Err(anyhow!(
        "Failed to find '{}' query parameter in the URL!",
        param_name
    ))
}

/// Parse the Datadog 'query' from a log URL.
pub fn query_from_url(url: &str) -> anyhow::Result<String> {
    get_query_param(url, "query")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn works_for_example() {
        let url = "https://app.datadoghq.com/logs?query=env%3Aproduction%20service%3Amultiplayer-proxy%20%22Proxied%20request%20failed%22%20%40response_status%3A%5B500%20TO%20600%5D&agg_q=%40file.key&cols=service%2C%40meta.res.statusCode&index=&messageDisplay=inline&sort_m=&sort_t=&stream_sort=time%2Cdesc&top_n=10&top_o=top&viz=pattern&x_missing=true&from_ts=1659903364196&to_ts=1659989764196&live=true";
        let expected_query = r#"env:production service:multiplayer-proxy "Proxied request failed" @response_status:[500 TO 600]"#;
        assert_eq!(query_from_url(url).unwrap(), expected_query);
    }

    #[test]
    fn returns_error_when_no_query_param() {
        let url =
            "https://app.datadoghq.com/logs?from_ts=1659903364196&to_ts=1659989764196&live=true";
        assert!(query_from_url(url).is_err());
    }
}
