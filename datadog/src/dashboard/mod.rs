mod api;

use crate::notebooks;

use anyhow::{anyhow, bail, Context};
use structopt::StructOpt;

#[derive(StructOpt, Debug)]
pub struct UnfurlOpt {
    /// Datadog URL. Supports direct dashboard URLs (/dashboard/ID/...) and
    /// shared short links (/s/TOKEN/ID) which are resolved automatically.
    /// If the URL contains a fullscreen_widget or tile_focus query param,
    /// only that widget is shown.
    url: String,

    /// Output full JSON instead of a human-readable summary.
    #[structopt(long)]
    json: bool,
}

#[derive(Debug)]
#[allow(dead_code)]
struct ParsedDashboardUrl {
    dashboard_id: String,
    /// Widget ID from `fullscreen_widget` or `tile_focus` query param.
    widget_id: Option<i64>,
    /// Epoch-millisecond timestamps from `from_ts` / `to_ts` query params.
    from_ts: Option<i64>,
    to_ts: Option<i64>,
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

    let get_param = |name: &str| -> Option<i64> {
        parsed
            .query_pairs()
            .find_map(|(k, v)| if k == name { v.parse().ok() } else { None })
    };

    let widget_id =
        get_param("fullscreen_widget").or_else(|| get_param("tile_focus"));

    Ok(ParsedDashboardUrl {
        dashboard_id,
        widget_id,
        from_ts: get_param("from_ts"),
        to_ts: get_param("to_ts"),
    })
}

fn is_short_link(url: &str) -> bool {
    url::Url::parse(url)
        .ok()
        .map(|u| u.path().starts_with("/s/"))
        .unwrap_or(false)
}

enum ResolvedUrl {
    Dashboard {
        url: String,
        og_image_url: Option<String>,
    },
    MetricExplorer {
        url: String,
        og_image_url: Option<String>,
    },
    Notebook {
        url: String,
        og_image_url: Option<String>,
    },
}

/// Follow the /s/ short link redirect. Datadog renders these as a SPA, but the
/// initial HTML contains a meta refresh or JS redirect with the real URL.
async fn resolve_short_link(url: &str) -> anyhow::Result<ResolvedUrl> {
    eprintln!("Resolving short link...");
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::limited(10))
        .build()?;

    let resp = client.get(url).send().await.context("Failed to fetch short link")?;
    let final_url = resp.url().to_string();
    let body = resp.text().await?;

    let og_image_url = extract_og_image(&body);

    // Check for /metric/explorer first (redirect or HTML body).
    if final_url.contains("/metric/explorer") {
        let decoded = final_url.replace("&amp;", "&");
        eprintln!("Resolved to: {}", decoded);
        return Ok(ResolvedUrl::MetricExplorer {
            url: decoded,
            og_image_url,
        });
    }
    if let Some(explorer_url) = extract_metric_explorer_url_from_html(&body) {
        eprintln!("Resolved to: {}", explorer_url);
        return Ok(ResolvedUrl::MetricExplorer {
            url: explorer_url,
            og_image_url,
        });
    }

    // If the redirect landed on a /dashboard/ URL, use it directly.
    if final_url.contains("/dashboard/") {
        let decoded = final_url.replace("&amp;", "&");
        eprintln!("Resolved to: {}", decoded);
        return Ok(ResolvedUrl::Dashboard {
            url: decoded,
            og_image_url,
        });
    }

    // Otherwise, Datadog renders a SPA — look for the dashboard URL in the HTML.
    if let Some(dashboard_url) = extract_dashboard_url_from_html(&body) {
        eprintln!("Resolved to: {}", dashboard_url);
        return Ok(ResolvedUrl::Dashboard {
            url: dashboard_url,
            og_image_url,
        });
    }

    // Check if it resolved to a notebook URL.
    if final_url.contains("/notebook/") {
        let decoded = final_url.replace("&amp;", "&");
        eprintln!("Resolved to notebook: {}", decoded);
        return Ok(ResolvedUrl::Notebook {
            url: decoded,
            og_image_url,
        });
    }
    if let Some(notebook_url) = extract_url_from_html(&body, "https://app.datadoghq.com/notebook/") {
        eprintln!("Resolved to notebook: {}", notebook_url);
        return Ok(ResolvedUrl::Notebook {
            url: notebook_url,
            og_image_url,
        });
    }

    Err(anyhow!(
        "Could not resolve short link. The page loaded but no dashboard, metric explorer, or notebook URL was found.\n\
         Try opening {} in a browser and passing the resolved URL directly.",
        url
    ))
}

/// Extract og:image URL from HTML meta tags.
fn extract_og_image(html: &str) -> Option<String> {
    // Look for: <meta property="og:image" content="URL" />
    let marker = "og:image";
    let pos = html.find(marker)?;
    let rest = &html[pos..];
    let content_pos = rest.find("content=\"")?;
    let url_start = content_pos + "content=\"".len();
    let url_rest = &rest[url_start..];
    let url_end = url_rest.find('"')?;
    Some(url_rest[..url_end].to_string())
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

/// Scan HTML body for a /metric/explorer URL and decode HTML entities.
fn extract_metric_explorer_url_from_html(html: &str) -> Option<String> {
    let pattern = "https://app.datadoghq.com/metric/explorer";
    let start = html.find(pattern)?;
    let rest = &html[start..];
    let end = rest
        .find(|c: char| c == '"' || c == '\'' || c == ' ' || c == '>' || c == '\\')
        .unwrap_or(rest.len());
    let raw = &rest[..end];
    Some(raw.replace("&amp;", "&"))
}

/// Generic helper: scan HTML body for a URL with the given prefix.
fn extract_url_from_html(html: &str, prefix: &str) -> Option<String> {
    let start = html.find(prefix)?;
    let rest = &html[start..];
    let end = rest
        .find(|c: char| c == '"' || c == '\'' || c == ' ' || c == '>' || c == '\\')
        .unwrap_or(rest.len());
    let raw = &rest[..end];
    Some(raw.replace("&amp;", "&"))
}

fn is_metric_explorer_url(url: &str) -> bool {
    url::Url::parse(url)
        .ok()
        .map(|u| u.path().starts_with("/metric/explorer"))
        .unwrap_or(false)
}

/// Decode the lz-string-compressed widget definition from a metric explorer URL
/// fragment. The fragment (after `#`) is an lz-string `compressToEncodedURIComponent`
/// payload whose decoded JSON contains `{ "widget": { "definition": { ... } } }`.
fn decode_metric_explorer_fragment(fragment: &str) -> anyhow::Result<serde_json::Value> {
    let decompressed = lz_str::decompress_from_encoded_uri_component(fragment)
        .ok_or_else(|| anyhow!("Failed to decompress lz-string fragment"))?;
    let json_str = String::from_utf16(&decompressed)
        .context("Decompressed data is not valid UTF-16")?;
    let value: serde_json::Value =
        serde_json::from_str(&json_str).context("Failed to parse decompressed JSON")?;
    Ok(value)
}

/// Handle a metric explorer URL: decode the fragment and print the widget.
fn handle_metric_explorer(url: &str, json_mode: bool) -> anyhow::Result<()> {
    let parsed = url::Url::parse(url).context("Invalid URL")?;
    let fragment = parsed
        .fragment()
        .ok_or_else(|| anyhow!("Metric explorer URL has no fragment (expected lz-string data after #)"))?;

    let value = decode_metric_explorer_fragment(fragment)?;

    // Print time range from query params if present.
    let get_param = |name: &str| -> Option<i64> {
        parsed
            .query_pairs()
            .find_map(|(k, v)| if k == name { v.parse().ok() } else { None })
    };
    if let (Some(from_ts), Some(to_ts)) = (get_param("start"), get_param("end")) {
        let from = chrono::DateTime::from_timestamp_millis(from_ts);
        let to = chrono::DateTime::from_timestamp_millis(to_ts);
        if let (Some(f), Some(t)) = (from, to) {
            eprintln!("Time range: {} to {}", f.format("%Y-%m-%d %H:%M UTC"), t.format("%Y-%m-%d %H:%M UTC"));
        }
    }

    // The fragment JSON has shape: { "widget": { "definition": { ... } } }
    // Wrap it to match the shape format_widget expects: { "definition": { ... } }
    let widget = if value.get("widget").is_some() {
        value["widget"].clone()
    } else {
        // Fallback: treat the whole value as the widget.
        value.clone()
    };

    if json_mode {
        println!("{}", serde_json::to_string_pretty(&widget)?);
    } else {
        eprintln!("Metric Explorer widget:");
        print!("{}", format_widget(&widget));
    }

    Ok(())
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

/// Format a notebook cell using the same approach as dashboard widgets.
/// Notebook cell attributes serialize as `{ "definition": {...}, "time": ... }`
/// which is the same shape format_widget expects.
fn format_notebook_cell(cell_json: &serde_json::Value) {
    let def = &cell_json["definition"];
    let widget_type = def["type"].as_str().unwrap_or("unknown");

    match widget_type {
        "markdown" => {
            // Markdown cells: just print the text.
            if let Some(text) = def["text"].as_str() {
                println!("{}", text);
            }
        }
        _ => {
            // Wrap as a widget-like object for format_widget.
            let widget = serde_json::json!({"definition": def});
            print!("{}", format_widget(&widget));

            // Print time override if present.
            if let Some(time) = cell_json.get("time").and_then(|t| t.as_object()) {
                if let Some(live_span) = time.get("live_span").and_then(|v| v.as_str()) {
                    println!("Time: {}", live_span);
                } else if let Some(start) = time.get("start") {
                    let end = time.get("end").and_then(|v| v.as_str()).unwrap_or("?");
                    let start = start.as_str().unwrap_or("?");
                    println!("Time: {} to {}", start, end);
                }
            }
        }
    }
}

fn indent(s: &str, prefix: &str) -> String {
    s.lines()
        .map(|line| format!("{}{}", prefix, line))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Download a URL to a file.
async fn download_to_file(url: &str, path: &str) -> anyhow::Result<()> {
    let client = reqwest::Client::new();
    let resp = client
        .get(url)
        .send()
        .await
        .context("Failed to download snapshot")?;
    let bytes = resp.bytes().await?;
    std::fs::write(path, &bytes).context("Failed to write snapshot file")?;
    Ok(())
}

fn format_widgets(widgets: &[serde_json::Value]) -> String {
    widgets
        .iter()
        .map(format_widget)
        .collect::<Vec<_>>()
        .join("\n")
}

pub async fn run_unfurl(
    api_key: &str,
    app_key: &str,
    opt: UnfurlOpt,
) -> anyhow::Result<()> {
    // Resolve short links and determine the URL type.
    let resolved = if is_short_link(&opt.url) {
        resolve_short_link(&opt.url).await?
    } else if is_metric_explorer_url(&opt.url) {
        ResolvedUrl::MetricExplorer {
            url: opt.url,
            og_image_url: None,
        }
    } else {
        ResolvedUrl::Dashboard {
            url: opt.url,
            og_image_url: None,
        }
    };

    match resolved {
        ResolvedUrl::MetricExplorer { url, og_image_url } => {
            handle_metric_explorer(&url, opt.json)?;

            if let Some(image_url) = &og_image_url {
                let path = "/tmp/dd-metric-explorer.png";
                match download_to_file(image_url, path).await {
                    Ok(()) => {
                        eprintln!("Snapshot: {}", path);
                        eprintln!("(Tip: the snapshot may include cursor annotations — timestamp and count — not shown above)");
                    }
                    Err(e) => eprintln!("Failed to download snapshot: {}", e),
                }
            }
        }
        ResolvedUrl::Dashboard { url, og_image_url } => {
            let parsed = parse_dashboard_url(&url)?;
            let dashboard =
                api::get_dashboard(api_key, app_key, &parsed.dashboard_id).await?;

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
                    if opt.json {
                        println!("{}", serde_json::to_string_pretty(&widget)?);
                    } else {
                        print!("{}", format_widget(&widget));
                    }

                    // Download the og:image from the shared link (same image Slack shows).
                    if let Some(image_url) = &og_image_url {
                        let path = format!("/tmp/dd-widget-{}.png", target_id);
                        match download_to_file(image_url, &path).await {
                            Ok(()) => {
                                eprintln!("Snapshot: {}", path);
                                eprintln!("(Tip: the snapshot may include cursor annotations — timestamp and count — not shown above)");
                            }
                            Err(e) => eprintln!("Failed to download snapshot: {}", e),
                        }
                    }
                } else {
                    return Err(anyhow!("Widget {} not found in dashboard", target_id));
                }
            } else if opt.json {
                println!("{}", serde_json::to_string_pretty(&widgets_array)?);
            } else {
                print!("{}", format_widgets(widgets_array));
            }
        }
        ResolvedUrl::Notebook { url, og_image_url } => {
            // Parse notebook ID and optional cell_id from the URL.
            let parsed_url = url::Url::parse(&url).context("Invalid notebook URL")?;
            let notebook_id = parsed_url
                .path_segments()
                .and_then(|mut s| {
                    while let Some(seg) = s.next() {
                        if seg == "notebook" {
                            return s.next();
                        }
                    }
                    None
                })
                .and_then(|id| id.parse::<i64>().ok());
            let cell_id: Option<String> = parsed_url
                .query_pairs()
                .find(|(k, _)| k == "cell_id")
                .map(|(_, v)| v.to_string());

            if let Some(nb_id) = notebook_id {
                match notebooks::api::get_notebook(api_key, app_key, nb_id).await {
                    Ok(response) => {
                        if let Some(data) = response.data {
                            eprintln!("Notebook: {}", data.attributes.name);

                            let target_cells: Vec<_> = if let Some(ref cid) = cell_id {
                                data.attributes.cells.iter().filter(|c| &c.id == cid).collect()
                            } else {
                                data.attributes.cells.iter().collect()
                            };

                            if target_cells.is_empty() {
                                if let Some(ref cid) = cell_id {
                                    eprintln!("Cell {} not found in notebook", cid);
                                }
                            }

                            for cell in &target_cells {
                                // Serialize cell attributes to JSON and use format_widget
                                // for the same output format as dashboard widgets.
                                let cell_json = serde_json::to_value(&cell.attributes).unwrap_or_default();
                                if opt.json {
                                    println!("{}", serde_json::to_string_pretty(&cell_json)?);
                                } else {
                                    format_notebook_cell(&cell_json);
                                }
                            }
                        }
                    }
                    Err(e) => eprintln!("Failed to fetch notebook: {}", e),
                }
            } else {
                eprintln!("Notebook: {}", url);
            }

            if let Some(image_url) = &og_image_url {
                let path = "/tmp/dd-notebook-snapshot.png";
                match download_to_file(image_url, path).await {
                    Ok(()) => {
                        eprintln!("Snapshot: {}", path);
                        eprintln!("(Tip: the snapshot may include cursor annotations — timestamp and count — not shown above)");
                    }
                    Err(e) => eprintln!("Failed to download snapshot: {}", e),
                }
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

    #[test]
    fn extract_og_image_works() {
        let html = r#"<meta property="og:image" content="https://p.datadoghq.com/s/image/e16e18c08/hkh-d76-9vd.png" />"#;
        assert_eq!(
            extract_og_image(html).unwrap(),
            "https://p.datadoghq.com/s/image/e16e18c08/hkh-d76-9vd.png"
        );
    }

    #[test]
    fn extract_og_image_none() {
        assert!(extract_og_image("<html>no image</html>").is_none());
    }

    #[test]
    fn parse_url_with_timestamps() {
        let url = "https://app.datadoghq.com/dashboard/5iv-bx7-9xp/title?from_ts=1770759395143&to_ts=1771968995143&fullscreen_widget=123";
        let parsed = parse_dashboard_url(url).unwrap();
        assert_eq!(parsed.from_ts, Some(1770759395143));
        assert_eq!(parsed.to_ts, Some(1771968995143));
    }

    #[test]
    fn is_metric_explorer_url_detects_correctly() {
        assert!(is_metric_explorer_url(
            "https://app.datadoghq.com/metric/explorer?start=123&end=456#N4Ig..."
        ));
        assert!(!is_metric_explorer_url(
            "https://app.datadoghq.com/dashboard/abc-def-123/title"
        ));
        assert!(!is_metric_explorer_url(
            "https://app.datadoghq.com/s/e16e18c08/hkh-d76-9vd"
        ));
    }

    #[test]
    fn extract_metric_explorer_url_from_html_works() {
        let html = r#"<script>window.location.href="https://app.datadoghq.com/metric/explorer?start=123&amp;end=456#N4Ig"</script>"#;
        let url = extract_metric_explorer_url_from_html(html).unwrap();
        assert_eq!(
            url,
            "https://app.datadoghq.com/metric/explorer?start=123&end=456#N4Ig"
        );
    }

    #[test]
    fn extract_metric_explorer_url_from_html_none() {
        assert!(extract_metric_explorer_url_from_html("<html>nothing here</html>").is_none());
    }

    #[test]
    fn decode_metric_explorer_fragment_roundtrip() {
        // Build a minimal widget definition, compress it, and verify we can decode it.
        let widget_json = r#"{"widget":{"definition":{"type":"timeseries","requests":[{"queries":[{"data_source":"metrics","name":"q1","query":"avg:system.cpu.user{*}"}],"formulas":[{"formula":"q1"}]}]}}}"#;
        let compressed = lz_str::compress_to_encoded_uri_component(widget_json);
        let decoded = decode_metric_explorer_fragment(&compressed).unwrap();
        assert_eq!(
            decoded["widget"]["definition"]["type"]
                .as_str()
                .unwrap(),
            "timeseries"
        );
        assert_eq!(
            decoded["widget"]["definition"]["requests"][0]["queries"][0]["query"]
                .as_str()
                .unwrap(),
            "avg:system.cpu.user{*}"
        );
    }

    #[test]
    fn handle_metric_explorer_prints_widget() {
        let widget_json = r#"{"widget":{"definition":{"type":"timeseries","title":"CPU Usage","requests":[{"queries":[{"data_source":"metrics","name":"q1","query":"avg:system.cpu.user{*}"}],"formulas":[{"formula":"q1"}]}]}}}"#;
        let compressed = lz_str::compress_to_encoded_uri_component(widget_json);
        let url = format!(
            "https://app.datadoghq.com/metric/explorer?start=1000&end=2000#{}",
            compressed
        );
        // Should not error.
        handle_metric_explorer(&url, false).unwrap();
    }

    #[test]
    fn handle_metric_explorer_json_mode() {
        let widget_json = r#"{"widget":{"definition":{"type":"timeseries","title":"Test","requests":[]}}}"#;
        let compressed = lz_str::compress_to_encoded_uri_component(widget_json);
        let url = format!(
            "https://app.datadoghq.com/metric/explorer#{}",
            compressed
        );
        handle_metric_explorer(&url, true).unwrap();
    }

    #[test]
    fn handle_metric_explorer_no_fragment_errors() {
        let url = "https://app.datadoghq.com/metric/explorer?start=1000&end=2000";
        assert!(handle_metric_explorer(url, false).is_err());
    }
}
