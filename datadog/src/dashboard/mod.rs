mod api;

use anyhow::{anyhow, bail, Context};
use structopt::StructOpt;

#[derive(StructOpt, Debug)]
pub struct DashboardOpt {
    #[structopt(subcommand)]
    cmd: DashboardCommand,
}

#[derive(StructOpt, Debug)]
pub enum DashboardCommand {
    /// Extract widget definitions from a dashboard URL.
    Widgets {
        /// Dashboard URL. Supports direct URLs (/dashboard/ID/...) and shared
        /// short links (/s/TOKEN/ID) which are resolved automatically.
        /// If the URL contains a fullscreen_widget or tile_focus query param,
        /// only that widget is printed.
        #[structopt(long)]
        url: String,

        /// Output full JSON instead of a human-readable summary.
        #[structopt(long)]
        json: bool,
    },
}

#[derive(Debug)]
struct ParsedDashboardUrl {
    dashboard_id: String,
    /// Widget ID from `fullscreen_widget` or `tile_focus` query param.
    widget_id: Option<i64>,
}

fn parse_dashboard_url(url: &str) -> anyhow::Result<ParsedDashboardUrl> {
    let parsed = url::Url::parse(url).context("Invalid URL")?;
    let segments: Vec<&str> = parsed
        .path_segments()
        .map(|s| s.collect())
        .unwrap_or_default();

    let dashboard_id = match segments.as_slice() {
        ["dashboard", id, ..] => id.to_string(),
        ["s", ..] => bail!("Short link detected — use resolve_short_link() first"),
        _ => {
            return Err(anyhow!(
                "Unrecognized URL format. Expected /dashboard/ID/... or /s/TOKEN/ID"
            ));
        }
    };

    // Extract widget ID from fullscreen_widget or tile_focus query params.
    let widget_id = parsed
        .query_pairs()
        .find_map(|(k, v)| {
            if k == "fullscreen_widget" || k == "tile_focus" {
                v.parse::<i64>().ok()
            } else {
                None
            }
        });

    Ok(ParsedDashboardUrl {
        dashboard_id,
        widget_id,
    })
}

fn is_short_link(url: &str) -> bool {
    url::Url::parse(url)
        .ok()
        .map(|u| u.path().starts_with("/s/"))
        .unwrap_or(false)
}

/// Follow the /s/ short link redirect. Datadog renders these as a SPA, but the
/// initial HTML contains a meta refresh or JS redirect with the real URL.
async fn resolve_short_link(url: &str) -> anyhow::Result<String> {
    eprintln!("Resolving short link...");
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::limited(10))
        .build()?;

    let resp = client.get(url).send().await.context("Failed to fetch short link")?;
    let final_url = resp.url().to_string();
    let body = resp.text().await?;

    // If the redirect landed on a /dashboard/ URL, use it directly.
    if final_url.contains("/dashboard/") {
        let decoded = final_url.replace("&amp;", "&");
        eprintln!("Resolved to: {}", decoded);
        return Ok(decoded);
    }

    // Otherwise, Datadog renders a SPA — look for the dashboard URL in the HTML.
    // Common patterns: window.location.href, meta refresh, or data attributes.
    if let Some(dashboard_url) = extract_dashboard_url_from_html(&body) {
        eprintln!("Resolved to: {}", dashboard_url);
        return Ok(dashboard_url);
    }

    Err(anyhow!(
        "Could not resolve short link. The page loaded but no /dashboard/ URL was found.\n\
         Try opening {} in a browser and passing the resolved URL directly.",
        url
    ))
}

/// Scan HTML body for a /dashboard/ URL and decode HTML entities.
fn extract_dashboard_url_from_html(html: &str) -> Option<String> {
    // Look for URLs like https://app.datadoghq.com/dashboard/xxx-yyy-zzz/...
    let pattern = "https://app.datadoghq.com/dashboard/";
    let start = html.find(pattern)?;
    let rest = &html[start..];
    // Find the end of the URL (quote, space, or angle bracket).
    let end = rest
        .find(|c: char| c == '"' || c == '\'' || c == ' ' || c == '>' || c == '\\')
        .unwrap_or(rest.len());
    let raw = &rest[..end];
    // Decode HTML entities (e.g. &amp; -> &).
    Some(raw.replace("&amp;", "&"))
}

/// Recursively search for a widget by ID within a widget tree (groups contain
/// nested widgets in their definition).
fn find_widget_by_id(
    widgets: &[serde_json::Value],
    target_id: i64,
) -> Option<serde_json::Value> {
    for widget in widgets {
        if let Some(id) = widget.get("id").and_then(|v| v.as_i64()) {
            if id == target_id {
                return Some(widget.clone());
            }
        }
        // Group widgets nest children under definition.widgets.
        if let Some(children) = widget
            .pointer("/definition/widgets")
            .and_then(|v| v.as_array())
        {
            if let Some(found) = find_widget_by_id(children, target_id) {
                return Some(found);
            }
        }
    }
    None
}

fn format_widget(widget: &serde_json::Value) -> String {
    let mut out = String::new();
    let def = &widget["definition"];

    let title = def["title"].as_str().unwrap_or("(untitled)");
    let widget_type = def["type"].as_str().unwrap_or("unknown");
    let id = widget.get("id").and_then(|v| v.as_i64());

    out.push_str(&format!("## {}", title));
    if let Some(id) = id {
        out.push_str(&format!("  [id: {}]", id));
    }
    out.push('\n');
    out.push_str(&format!("Type: {}\n", widget_type));

    // For group widgets, recurse into children.
    if widget_type == "group" {
        if let Some(children) = def["widgets"].as_array() {
            for child in children {
                out.push('\n');
                out.push_str(&indent(&format_widget(child), "  "));
            }
        }
        return out;
    }

    // Print queries from requests.
    if let Some(requests) = def["requests"].as_array() {
        for (i, req) in requests.iter().enumerate() {
            let display_type = req["display_type"].as_str().unwrap_or("");
            // Skip overlay/event requests — they're decoration, not the main data.
            if display_type == "overlay" {
                continue;
            }

            if requests.len() > 1 {
                out.push_str(&format!("\nRequest {}:\n", i + 1));
            }

            // Formulas
            if let Some(formulas) = req["formulas"].as_array() {
                for f in formulas {
                    let formula = f["formula"].as_str().unwrap_or("");
                    let alias = f["alias"].as_str().unwrap_or("");
                    if !formula.is_empty() {
                        out.push_str(&format!("  Formula: {}", formula));
                        if !alias.is_empty() {
                            out.push_str(&format!(" (as \"{}\")", alias));
                        }
                        out.push('\n');
                    }
                }
            }

            // Queries
            if let Some(queries) = req["queries"].as_array() {
                for q in queries {
                    let data_source = q["data_source"].as_str().unwrap_or("");
                    let name = q["name"].as_str().unwrap_or("");
                    match data_source {
                        "metrics" => {
                            let query = q["query"].as_str().unwrap_or("");
                            out.push_str(&format!("  Query ({}): {}\n", name, query));
                        }
                        "events" | "logs" => {
                            let search = q["search"]["query"].as_str().unwrap_or("");
                            let compute = q["compute"]["aggregation"].as_str().unwrap_or("");
                            if !search.is_empty() || !compute.is_empty() {
                                out.push_str(&format!(
                                    "  Query ({}): {} search=\"{}\"\n",
                                    name, compute, search
                                ));
                            }
                        }
                        _ => {
                            let query = q["query"].as_str().unwrap_or("");
                            if !query.is_empty() {
                                out.push_str(&format!("  Query ({}): {}\n", name, query));
                            }
                        }
                    }
                }
            }

            // Simple query field (e.g. manage_status widgets).
            if req["queries"].is_null() {
                if let Some(q) = req["query"].as_str() {
                    out.push_str(&format!("  Query: {}\n", q));
                }
            }
        }
    }

    // Top-level query (some widget types like manage_status).
    if def["requests"].is_null() {
        if let Some(q) = def["query"].as_str() {
            out.push_str(&format!("Query: {}\n", q));
        }
    }

    out
}

fn indent(s: &str, prefix: &str) -> String {
    s.lines()
        .map(|line| format!("{}{}", prefix, line))
        .collect::<Vec<_>>()
        .join("\n")
}

fn format_widgets(widgets: &[serde_json::Value]) -> String {
    widgets
        .iter()
        .map(format_widget)
        .collect::<Vec<_>>()
        .join("\n")
}

pub async fn run_dashboard(
    api_key: &str,
    app_key: &str,
    opt: DashboardOpt,
) -> anyhow::Result<()> {
    match opt.cmd {
        DashboardCommand::Widgets { url, json } => {
            let resolved_url = if is_short_link(&url) {
                resolve_short_link(&url).await?
            } else {
                url
            };
            let parsed = parse_dashboard_url(&resolved_url)?;
            let dashboard = api::get_dashboard(api_key, app_key, &parsed.dashboard_id).await?;

            eprintln!(
                "Dashboard: {} ({} widgets)",
                dashboard.title,
                dashboard.widgets.len()
            );

            // Serialize to serde_json::Value so we can search by widget ID.
            let widgets_value = serde_json::to_value(&dashboard.widgets)?;
            let widgets_array = widgets_value
                .as_array()
                .ok_or_else(|| anyhow!("Expected widgets array"))?;

            if let Some(target_id) = parsed.widget_id {
                if let Some(widget) = find_widget_by_id(widgets_array, target_id) {
                    if json {
                        println!("{}", serde_json::to_string_pretty(&widget)?);
                    } else {
                        print!("{}", format_widget(&widget));
                    }
                } else {
                    return Err(anyhow!("Widget {} not found in dashboard", target_id));
                }
            } else if json {
                println!("{}", serde_json::to_string_pretty(&widgets_array)?);
            } else {
                print!("{}", format_widgets(widgets_array));
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_direct_dashboard_url() {
        let url = "https://app.datadoghq.com/dashboard/abc-def-123/my-dashboard-title";
        let parsed = parse_dashboard_url(url).unwrap();
        assert_eq!(parsed.dashboard_id, "abc-def-123");
        assert_eq!(parsed.widget_id, None);
    }

    #[test]
    fn parse_direct_dashboard_url_no_slug() {
        let url = "https://app.datadoghq.com/dashboard/abc-def-123";
        let parsed = parse_dashboard_url(url).unwrap();
        assert_eq!(parsed.dashboard_id, "abc-def-123");
        assert_eq!(parsed.widget_id, None);
    }

    #[test]
    fn parse_url_with_fullscreen_widget() {
        let url = "https://app.datadoghq.com/dashboard/5iv-bx7-9xp/multiplayer-v2?fromUser=false&refresh_mode=paused&from_ts=123&to_ts=456&fullscreen_widget=3737456056966802";
        let parsed = parse_dashboard_url(url).unwrap();
        assert_eq!(parsed.dashboard_id, "5iv-bx7-9xp");
        assert_eq!(parsed.widget_id, Some(3737456056966802));
    }

    #[test]
    fn parse_url_with_tile_focus() {
        let url = "https://app.datadoghq.com/dashboard/5iv-bx7-9xp/title?tile_focus=12345";
        let parsed = parse_dashboard_url(url).unwrap();
        assert_eq!(parsed.dashboard_id, "5iv-bx7-9xp");
        assert_eq!(parsed.widget_id, Some(12345));
    }

    #[test]
    fn parse_invalid_url() {
        let url = "https://app.datadoghq.com/monitors/12345";
        assert!(parse_dashboard_url(url).is_err());
    }

    #[test]
    fn is_short_link_detects_s_urls() {
        assert!(is_short_link("https://app.datadoghq.com/s/e16e18c08/hkh-d76-9vd"));
        assert!(!is_short_link("https://app.datadoghq.com/dashboard/abc-def-123/title"));
    }

    #[test]
    fn extract_dashboard_url_from_html_works() {
        let html = r#"<script>window.location.href="https://app.datadoghq.com/dashboard/5iv-bx7-9xp/multiplayer-v2?fullscreen_widget=123"</script>"#;
        let url = extract_dashboard_url_from_html(html).unwrap();
        assert_eq!(url, "https://app.datadoghq.com/dashboard/5iv-bx7-9xp/multiplayer-v2?fullscreen_widget=123");
    }

    #[test]
    fn extract_dashboard_url_from_html_none() {
        assert!(extract_dashboard_url_from_html("<html>nothing here</html>").is_none());
    }

    #[test]
    fn find_widget_top_level() {
        let widgets: Vec<serde_json::Value> = serde_json::from_str(
            r#"[{"id": 1, "definition": {"type": "timeseries"}}, {"id": 2, "definition": {"type": "query_value"}}]"#
        ).unwrap();
        let found = find_widget_by_id(&widgets, 2).unwrap();
        assert_eq!(found["id"], 2);
    }

    #[test]
    fn find_widget_nested_in_group() {
        let widgets: Vec<serde_json::Value> = serde_json::from_str(
            r#"[{"id": 1, "definition": {"type": "group", "widgets": [{"id": 99, "definition": {"type": "timeseries"}}]}}]"#
        ).unwrap();
        let found = find_widget_by_id(&widgets, 99).unwrap();
        assert_eq!(found["id"], 99);
    }

    #[test]
    fn find_widget_not_found() {
        let widgets: Vec<serde_json::Value> =
            serde_json::from_str(r#"[{"id": 1, "definition": {"type": "timeseries"}}]"#).unwrap();
        assert!(find_widget_by_id(&widgets, 999).is_none());
    }
}
