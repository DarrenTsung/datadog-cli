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

/// Validate a Datadog log query and return tips for fixing common syntax issues.
///
/// Returns an empty Vec when the query looks fine.
pub fn validate_query(query: &str) -> Vec<String> {
    let mut tips = Vec::new();
    for token in tokenize(query) {
        // Strip leading `-` or `NOT ` negation prefix.
        let token = token.strip_prefix('-').unwrap_or(token);

        // Must contain an unquoted key:value separator.
        let Some((key, value)) = token.split_once(':') else {
            continue;
        };

        // Skip range syntax like `[500 TO 600]` or wildcard-only values.
        if value.starts_with('[') || value.starts_with('"') {
            continue;
        }

        // The value itself contains additional colons — needs quoting.
        if value.contains(':') {
            tips.push(format!(
                "Value \"{}\" in \"{}:...\" contains unquoted colons.\n    Wrap the value in double quotes: {}:\"{}\"",
                value, key, key, value
            ));
        }
    }
    tips
}

/// Split a query string into whitespace-separated tokens while keeping quoted
/// strings intact (double-quotes only).
fn tokenize(query: &str) -> Vec<&str> {
    let mut tokens = Vec::new();
    let mut start = None;
    let mut in_quotes = false;
    for (i, ch) in query.char_indices() {
        match ch {
            '"' => {
                if start.is_none() {
                    start = Some(i);
                }
                in_quotes = !in_quotes;
            }
            ' ' if !in_quotes => {
                if let Some(s) = start {
                    tokens.push(&query[s..i]);
                    start = None;
                }
            }
            _ => {
                if start.is_none() {
                    start = Some(i);
                }
            }
        }
    }
    if let Some(s) = start {
        tokens.push(&query[s..]);
    }
    tokens
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

    #[test]
    fn validate_detects_unquoted_colons() {
        let tips = validate_query("@job_name:figma::highprifilethumbnailjob");
        assert_eq!(tips.len(), 1);
        assert!(tips[0].contains("unquoted colons"));
        assert!(tips[0].contains("@job_name:\"figma::highprifilethumbnailjob\""));
    }

    #[test]
    fn validate_accepts_quoted_value() {
        let tips = validate_query(r#"@job_name:"figma::highprifilethumbnailjob""#);
        assert!(tips.is_empty());
    }

    #[test]
    fn validate_accepts_single_colon_value() {
        let tips = validate_query("service:sinatra-async-worker");
        assert!(tips.is_empty());
    }

    #[test]
    fn validate_only_flags_bad_token() {
        let tips =
            validate_query("service:sinatra-async-worker @job_name:figma::foo status:error");
        assert_eq!(tips.len(), 1);
        assert!(tips[0].contains("@job_name"));
    }

    #[test]
    fn validate_detects_negated_facet() {
        let tips = validate_query("-@job_name:foo::bar");
        assert_eq!(tips.len(), 1);
        assert!(tips[0].contains("unquoted colons"));
    }
}
