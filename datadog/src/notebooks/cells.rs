use chrono::{DateTime, Utc};
use datadog_api_client::datadogV1::model::{
    FormulaAndFunctionQueryDefinition, LogStreamWidgetDefinition, LogStreamWidgetDefinitionType,
    NotebookAbsoluteTime, NotebookCellCreateRequest, NotebookCellCreateRequestAttributes,
    NotebookCellResourceType, NotebookCellResponseAttributes, NotebookCellTime,
    NotebookLogStreamCellAttributes, NotebookMarkdownCellAttributes,
    NotebookMarkdownCellDefinition, NotebookMarkdownCellDefinitionType, NotebookRelativeTime,
    NotebookTimeseriesCellAttributes, TimeseriesWidgetDefinition,
    TimeseriesWidgetDefinitionType, TimeseriesWidgetExpressionAlias, TimeseriesWidgetRequest,
    WidgetDisplayType, WidgetLiveSpan,
};
use serde_derive::Deserialize;

use super::api::parse_live_span;

#[derive(Debug, Clone, PartialEq)]
pub enum Cell {
    Markdown(String),
    LogQuery(LogQueryCell),
    MetricQuery(MetricQueryCell),
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct LogQueryCell {
    pub query: String,
    pub indexes: Option<Vec<String>>,
    pub columns: Option<Vec<String>>,
    pub time: Option<CellTime>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct MetricQueryCell {
    pub query: String,
    pub time: Option<CellTime>,
    /// Graph title displayed above the timeseries widget.
    pub title: Option<String>,
    /// Display aliases for metric expressions. Maps the query expression
    /// to a human-readable name shown in the legend.
    /// Example: `{"avg:system.cpu.user{*}": "CPU Usage"}`
    pub aliases: Option<std::collections::HashMap<String, String>>,
    /// Display type: "line" (default), "bars", or "area".
    pub display_type: Option<String>,
}

/// Per-cell time override. Either a relative span string like `"4h"` or an
/// absolute range object like `{"start": "...", "end": "..."}`.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(untagged)]
pub enum CellTime {
    Absolute { start: DateTime<Utc>, end: DateTime<Utc> },
    Relative(String),
}

fn cell_time_to_notebook_cell_time(ct: &CellTime) -> NotebookCellTime {
    match ct {
        CellTime::Relative(s) => {
            let live_span = parse_live_span(s).unwrap_or(WidgetLiveSpan::PAST_ONE_HOUR);
            NotebookCellTime::NotebookRelativeTime(Box::new(NotebookRelativeTime::new(live_span)))
        }
        CellTime::Absolute { start, end } => {
            NotebookCellTime::NotebookAbsoluteTime(Box::new(NotebookAbsoluteTime::new(
                *end, *start,
            )))
        }
    }
}

pub fn cell_to_create_request(cell: &Cell) -> NotebookCellCreateRequest {
    let attributes = match cell {
        Cell::Markdown(text) => {
            NotebookCellCreateRequestAttributes::NotebookMarkdownCellAttributes(Box::new(
                NotebookMarkdownCellAttributes::new(NotebookMarkdownCellDefinition::new(
                    text.clone(),
                    NotebookMarkdownCellDefinitionType::MARKDOWN,
                )),
            ))
        }
        Cell::LogQuery(log_query) => {
            let mut definition =
                LogStreamWidgetDefinition::new(LogStreamWidgetDefinitionType::LOG_STREAM);
            definition.query = Some(log_query.query.clone());
            if let Some(indexes) = &log_query.indexes {
                definition.indexes = Some(indexes.clone());
            }
            if let Some(columns) = &log_query.columns {
                definition.columns = Some(columns.clone());
            }
            let mut attrs = NotebookLogStreamCellAttributes::new(definition);
            if let Some(cell_time) = &log_query.time {
                attrs.time = Some(Some(cell_time_to_notebook_cell_time(cell_time)));
            }
            NotebookCellCreateRequestAttributes::NotebookLogStreamCellAttributes(Box::new(attrs))
        }
        Cell::MetricQuery(metric_query) => {
            let mut request = TimeseriesWidgetRequest::new().q(metric_query.query.clone());

            // Set display type (line/bars/area).
            if let Some(ref dt) = metric_query.display_type {
                request.display_type = Some(match dt.to_lowercase().as_str() {
                    "bars" | "bar" => WidgetDisplayType::BARS,
                    "area" => WidgetDisplayType::AREA,
                    _ => WidgetDisplayType::LINE,
                });
            }

            // Set aliases via metadata.
            if let Some(ref aliases) = metric_query.aliases {
                let metadata: Vec<TimeseriesWidgetExpressionAlias> = aliases
                    .iter()
                    .map(|(expr, alias)| {
                        let mut a = TimeseriesWidgetExpressionAlias::new(expr.clone());
                        a.alias_name = Some(alias.clone());
                        a
                    })
                    .collect();
                if !metadata.is_empty() {
                    request.metadata = Some(metadata);
                }
            }

            let mut definition = TimeseriesWidgetDefinition::new(
                vec![request],
                TimeseriesWidgetDefinitionType::TIMESERIES,
            );

            // Set graph title.
            if let Some(ref title) = metric_query.title {
                definition.title = Some(title.clone());
            }

            let mut attrs = NotebookTimeseriesCellAttributes::new(definition);
            if let Some(cell_time) = &metric_query.time {
                attrs.time = Some(Some(cell_time_to_notebook_cell_time(cell_time)));
            }
            NotebookCellCreateRequestAttributes::NotebookTimeseriesCellAttributes(Box::new(attrs))
        }
    };

    NotebookCellCreateRequest::new(attributes, NotebookCellResourceType::NOTEBOOK_CELLS)
}

pub fn cells_to_create_requests(cells: &[Cell]) -> Vec<NotebookCellCreateRequest> {
    cells.iter().map(cell_to_create_request).collect()
}

/// Convert a `NotebookCellTime` back to a JSON-compatible string fragment for
/// embedding inside a fenced code block.
fn notebook_cell_time_to_json_value(time: &NotebookCellTime) -> serde_json::Value {
    match time {
        NotebookCellTime::NotebookRelativeTime(rt) => {
            serde_json::Value::String(rt.live_span.to_string())
        }
        NotebookCellTime::NotebookAbsoluteTime(at) => {
            serde_json::json!({
                "start": at.start.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
                "end": at.end.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
            })
        }
        NotebookCellTime::UnparsedObject(_) | _ => serde_json::Value::Null,
    }
}

/// Extract the metric query string from a `TimeseriesWidgetRequest`'s
/// `queries` array (the newer formula-based format). Returns the `query`
/// field from the first `FormulaAndFunctionMetricQueryDefinition`.
fn extract_metric_query_from_queries(req: &TimeseriesWidgetRequest) -> Option<String> {
    let queries = req.queries.as_ref()?;
    for q in queries {
        if let FormulaAndFunctionQueryDefinition::FormulaAndFunctionMetricQueryDefinition(def) = q {
            return Some(def.query.clone());
        }
    }
    None
}

/// Convert a notebook cell response back to the markdown format the parser
/// understands.
pub fn notebook_cell_to_markdown(attrs: &NotebookCellResponseAttributes) -> String {
    match attrs {
        NotebookCellResponseAttributes::NotebookMarkdownCellAttributes(md) => {
            md.definition.text.clone()
        }
        NotebookCellResponseAttributes::NotebookLogStreamCellAttributes(log) => {
            let mut obj = serde_json::Map::new();
            if let Some(q) = &log.definition.query {
                obj.insert("query".into(), serde_json::Value::String(q.clone()));
            }
            if let Some(indexes) = &log.definition.indexes {
                obj.insert("indexes".into(), serde_json::json!(indexes));
            }
            if let Some(columns) = &log.definition.columns {
                obj.insert("columns".into(), serde_json::json!(columns));
            }
            if let Some(Some(time)) = &log.time {
                obj.insert("time".into(), notebook_cell_time_to_json_value(time));
            }
            format!(
                "```log-query\n{}\n```",
                serde_json::to_string_pretty(&obj).unwrap()
            )
        }
        NotebookCellResponseAttributes::NotebookTimeseriesCellAttributes(ts) => {
            let mut obj = serde_json::Map::new();
            if let Some(req) = ts.definition.requests.first() {
                // Try the legacy `q` field first, then the newer `queries`
                // array (used by formula-based widget definitions).
                if let Some(q) = &req.q {
                    obj.insert("query".into(), serde_json::Value::String(q.clone()));
                } else if let Some(query_str) = extract_metric_query_from_queries(req) {
                    obj.insert("query".into(), serde_json::Value::String(query_str));
                }
                if let Some(dt) = &req.display_type {
                    obj.insert("display_type".into(), serde_json::Value::String(dt.to_string()));
                }
                if let Some(metadata) = &req.metadata {
                    let mut aliases = serde_json::Map::new();
                    for alias in metadata {
                        if let Some(name) = &alias.alias_name {
                            aliases.insert(alias.expression.clone(), serde_json::Value::String(name.clone()));
                        }
                    }
                    if !aliases.is_empty() {
                        obj.insert("aliases".into(), serde_json::Value::Object(aliases));
                    }
                }
            }
            if let Some(title) = &ts.definition.title {
                obj.insert("title".into(), serde_json::Value::String(title.clone()));
            }
            if let Some(Some(time)) = &ts.time {
                obj.insert("time".into(), notebook_cell_time_to_json_value(time));
            }
            format!(
                "```metric-query\n{}\n```",
                serde_json::to_string_pretty(&obj).unwrap()
            )
        }
        NotebookCellResponseAttributes::NotebookToplistCellAttributes(_) => {
            "<!-- Unsupported cell type: toplist -->".to_string()
        }
        NotebookCellResponseAttributes::NotebookHeatMapCellAttributes(_) => {
            "<!-- Unsupported cell type: heatmap -->".to_string()
        }
        NotebookCellResponseAttributes::NotebookDistributionCellAttributes(_) => {
            "<!-- Unsupported cell type: distribution -->".to_string()
        }
        NotebookCellResponseAttributes::UnparsedObject(_) | _ => {
            "<!-- Unsupported cell type: unknown -->".to_string()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markdown_cell_to_create_request() {
        let cell = Cell::Markdown("# Hello".to_string());
        let request = cell_to_create_request(&cell);

        assert_eq!(request.type_, NotebookCellResourceType::NOTEBOOK_CELLS);
        match &request.attributes {
            NotebookCellCreateRequestAttributes::NotebookMarkdownCellAttributes(attrs) => {
                assert_eq!(attrs.definition.text, "# Hello");
                assert_eq!(
                    attrs.definition.type_,
                    NotebookMarkdownCellDefinitionType::MARKDOWN
                );
            }
            _ => panic!("Expected NotebookMarkdownCellAttributes"),
        }
    }

    #[test]
    fn log_query_cell_to_create_request() {
        let cell = Cell::LogQuery(LogQueryCell {
            query: "env:prod".to_string(),
            indexes: Some(vec!["main".to_string()]),
            columns: None,
            time: None,
        });
        let request = cell_to_create_request(&cell);

        assert_eq!(request.type_, NotebookCellResourceType::NOTEBOOK_CELLS);
        match &request.attributes {
            NotebookCellCreateRequestAttributes::NotebookLogStreamCellAttributes(attrs) => {
                assert_eq!(attrs.definition.query.as_deref(), Some("env:prod"));
                assert_eq!(
                    attrs.definition.indexes,
                    Some(vec!["main".to_string()])
                );
                assert_eq!(
                    attrs.definition.type_,
                    LogStreamWidgetDefinitionType::LOG_STREAM
                );
                assert_eq!(attrs.time, None);
            }
            _ => panic!("Expected NotebookLogStreamCellAttributes"),
        }
    }

    #[test]
    fn log_query_cell_without_indexes() {
        let cell = Cell::LogQuery(LogQueryCell {
            query: "service:web".to_string(),
            indexes: None,
            columns: None,
            time: None,
        });
        let request = cell_to_create_request(&cell);

        match &request.attributes {
            NotebookCellCreateRequestAttributes::NotebookLogStreamCellAttributes(attrs) => {
                assert_eq!(attrs.definition.query.as_deref(), Some("service:web"));
                assert_eq!(attrs.definition.indexes, None);
            }
            _ => panic!("Expected NotebookLogStreamCellAttributes"),
        }
    }

    #[test]
    fn cells_to_create_requests_preserves_order() {
        let cells = vec![
            Cell::Markdown("first".to_string()),
            Cell::LogQuery(LogQueryCell {
                query: "env:prod".to_string(),
                indexes: None,
                columns: None,
                time: None,
            }),
            Cell::Markdown("third".to_string()),
        ];
        let requests = cells_to_create_requests(&cells);

        assert_eq!(requests.len(), 3);
        assert!(matches!(
            &requests[0].attributes,
            NotebookCellCreateRequestAttributes::NotebookMarkdownCellAttributes(_)
        ));
        assert!(matches!(
            &requests[1].attributes,
            NotebookCellCreateRequestAttributes::NotebookLogStreamCellAttributes(_)
        ));
        assert!(matches!(
            &requests[2].attributes,
            NotebookCellCreateRequestAttributes::NotebookMarkdownCellAttributes(_)
        ));
    }

    #[test]
    fn log_query_cell_with_relative_time() {
        let cell = Cell::LogQuery(LogQueryCell {
            query: "env:prod".to_string(),
            indexes: None,
            columns: None,
            time: Some(CellTime::Relative("4h".to_string())),
        });
        let request = cell_to_create_request(&cell);

        match &request.attributes {
            NotebookCellCreateRequestAttributes::NotebookLogStreamCellAttributes(attrs) => {
                match &attrs.time {
                    Some(Some(NotebookCellTime::NotebookRelativeTime(rt))) => {
                        assert_eq!(rt.live_span, WidgetLiveSpan::PAST_FOUR_HOURS);
                    }
                    other => panic!("Expected NotebookRelativeTime, got {:?}", other),
                }
            }
            _ => panic!("Expected NotebookLogStreamCellAttributes"),
        }
    }

    #[test]
    fn log_query_cell_with_columns() {
        let cell = Cell::LogQuery(LogQueryCell {
            query: "env:prod".to_string(),
            indexes: None,
            columns: Some(vec!["@backend".to_string(), "@error".to_string()]),
            time: None,
        });
        let request = cell_to_create_request(&cell);

        match &request.attributes {
            NotebookCellCreateRequestAttributes::NotebookLogStreamCellAttributes(attrs) => {
                assert_eq!(
                    attrs.definition.columns,
                    Some(vec!["@backend".to_string(), "@error".to_string()])
                );
            }
            _ => panic!("Expected NotebookLogStreamCellAttributes"),
        }
    }

    #[test]
    fn metric_query_cell_to_create_request() {
        let cell = Cell::MetricQuery(MetricQueryCell {
            query: "avg:system.cpu.user{env:production}".to_string(),
            time: None,
            title: None,
            aliases: None,
            display_type: None,
        });
        let request = cell_to_create_request(&cell);

        assert_eq!(request.type_, NotebookCellResourceType::NOTEBOOK_CELLS);
        match &request.attributes {
            NotebookCellCreateRequestAttributes::NotebookTimeseriesCellAttributes(attrs) => {
                assert_eq!(attrs.definition.requests.len(), 1);
                assert_eq!(
                    attrs.definition.requests[0].q.as_deref(),
                    Some("avg:system.cpu.user{env:production}")
                );
                assert_eq!(
                    attrs.definition.type_,
                    TimeseriesWidgetDefinitionType::TIMESERIES
                );
                assert_eq!(attrs.time, None);
            }
            _ => panic!("Expected NotebookTimeseriesCellAttributes"),
        }
    }

    #[test]
    fn metric_query_cell_with_relative_time() {
        let cell = Cell::MetricQuery(MetricQueryCell {
            query: "avg:system.cpu.user{*}".to_string(),
            time: Some(CellTime::Relative("1h".to_string())),
            title: None,
            aliases: None,
            display_type: None,
        });
        let request = cell_to_create_request(&cell);

        match &request.attributes {
            NotebookCellCreateRequestAttributes::NotebookTimeseriesCellAttributes(attrs) => {
                match &attrs.time {
                    Some(Some(NotebookCellTime::NotebookRelativeTime(rt))) => {
                        assert_eq!(rt.live_span, WidgetLiveSpan::PAST_ONE_HOUR);
                    }
                    other => panic!("Expected NotebookRelativeTime, got {:?}", other),
                }
            }
            _ => panic!("Expected NotebookTimeseriesCellAttributes"),
        }
    }

    #[test]
    fn log_query_cell_with_absolute_time() {
        let start: DateTime<Utc> = "2026-02-20T00:00:00Z".parse().unwrap();
        let end: DateTime<Utc> = "2026-02-24T00:00:00Z".parse().unwrap();
        let cell = Cell::LogQuery(LogQueryCell {
            query: "env:prod".to_string(),
            indexes: None,
            columns: None,
            time: Some(CellTime::Absolute { start, end }),
        });
        let request = cell_to_create_request(&cell);

        match &request.attributes {
            NotebookCellCreateRequestAttributes::NotebookLogStreamCellAttributes(attrs) => {
                match &attrs.time {
                    Some(Some(NotebookCellTime::NotebookAbsoluteTime(at))) => {
                        assert_eq!(at.start, start);
                        assert_eq!(at.end, end);
                    }
                    other => panic!("Expected NotebookAbsoluteTime, got {:?}", other),
                }
            }
            _ => panic!("Expected NotebookLogStreamCellAttributes"),
        }
    }

    // --- reverse conversion tests ---

    #[test]
    fn reverse_markdown_cell() {
        let attrs = NotebookCellResponseAttributes::NotebookMarkdownCellAttributes(Box::new(
            NotebookMarkdownCellAttributes::new(NotebookMarkdownCellDefinition::new(
                "# Hello\nWorld".to_string(),
                NotebookMarkdownCellDefinitionType::MARKDOWN,
            )),
        ));
        assert_eq!(notebook_cell_to_markdown(&attrs), "# Hello\nWorld");
    }

    #[test]
    fn reverse_log_query_cell_basic() {
        let mut def = LogStreamWidgetDefinition::new(LogStreamWidgetDefinitionType::LOG_STREAM);
        def.query = Some("env:prod".to_string());
        let attrs = NotebookCellResponseAttributes::NotebookLogStreamCellAttributes(Box::new(
            NotebookLogStreamCellAttributes::new(def),
        ));
        let md = notebook_cell_to_markdown(&attrs);
        assert!(md.starts_with("```log-query\n"));
        assert!(md.ends_with("\n```"));
        assert!(md.contains("\"query\": \"env:prod\""));
    }

    #[test]
    fn reverse_log_query_cell_with_indexes_and_columns() {
        let mut def = LogStreamWidgetDefinition::new(LogStreamWidgetDefinitionType::LOG_STREAM);
        def.query = Some("env:prod".to_string());
        def.indexes = Some(vec!["main".to_string()]);
        def.columns = Some(vec!["@host".to_string()]);
        let attrs = NotebookCellResponseAttributes::NotebookLogStreamCellAttributes(Box::new(
            NotebookLogStreamCellAttributes::new(def),
        ));
        let md = notebook_cell_to_markdown(&attrs);
        assert!(md.contains("\"indexes\""));
        assert!(md.contains("\"columns\""));
    }

    #[test]
    fn reverse_log_query_cell_with_relative_time() {
        let mut def = LogStreamWidgetDefinition::new(LogStreamWidgetDefinitionType::LOG_STREAM);
        def.query = Some("env:prod".to_string());
        let mut cell_attrs = NotebookLogStreamCellAttributes::new(def);
        cell_attrs.time = Some(Some(NotebookCellTime::NotebookRelativeTime(Box::new(
            NotebookRelativeTime::new(WidgetLiveSpan::PAST_FOUR_HOURS),
        ))));
        let attrs = NotebookCellResponseAttributes::NotebookLogStreamCellAttributes(Box::new(
            cell_attrs,
        ));
        let md = notebook_cell_to_markdown(&attrs);
        assert!(md.contains("\"time\": \"4h\""));
    }

    #[test]
    fn reverse_timeseries_cell() {
        let request = TimeseriesWidgetRequest::new().q("avg:system.cpu.user{*}".to_string());
        let def = TimeseriesWidgetDefinition::new(
            vec![request],
            TimeseriesWidgetDefinitionType::TIMESERIES,
        );
        let attrs = NotebookCellResponseAttributes::NotebookTimeseriesCellAttributes(Box::new(
            NotebookTimeseriesCellAttributes::new(def),
        ));
        let md = notebook_cell_to_markdown(&attrs);
        assert!(md.starts_with("```metric-query\n"));
        assert!(md.ends_with("\n```"));
        assert!(md.contains("avg:system.cpu.user{*}"));
    }

    #[test]
    fn reverse_unsupported_types() {
        use datadog_api_client::datadogV1::model::{
            DistributionWidgetDefinition, DistributionWidgetDefinitionType,
            DistributionWidgetRequest, HeatMapWidgetDefinition, HeatMapWidgetDefinitionType,
            NotebookDistributionCellAttributes, NotebookHeatMapCellAttributes,
            NotebookToplistCellAttributes, ToplistWidgetDefinition, ToplistWidgetDefinitionType,
            ToplistWidgetRequest,
        };

        let toplist_def = ToplistWidgetDefinition::new(
            vec![ToplistWidgetRequest::new()],
            ToplistWidgetDefinitionType::TOPLIST,
        );
        let toplist = NotebookCellResponseAttributes::NotebookToplistCellAttributes(Box::new(
            NotebookToplistCellAttributes::new(toplist_def),
        ));
        assert_eq!(
            notebook_cell_to_markdown(&toplist),
            "<!-- Unsupported cell type: toplist -->"
        );

        let heatmap_def = HeatMapWidgetDefinition::new(
            vec![],
            HeatMapWidgetDefinitionType::HEATMAP,
        );
        let heatmap = NotebookCellResponseAttributes::NotebookHeatMapCellAttributes(Box::new(
            NotebookHeatMapCellAttributes::new(heatmap_def),
        ));
        assert_eq!(
            notebook_cell_to_markdown(&heatmap),
            "<!-- Unsupported cell type: heatmap -->"
        );

        let dist_def = DistributionWidgetDefinition::new(
            vec![DistributionWidgetRequest::new()],
            DistributionWidgetDefinitionType::DISTRIBUTION,
        );
        let dist = NotebookCellResponseAttributes::NotebookDistributionCellAttributes(Box::new(
            NotebookDistributionCellAttributes::new(dist_def),
        ));
        assert_eq!(
            notebook_cell_to_markdown(&dist),
            "<!-- Unsupported cell type: distribution -->"
        );
    }
}
