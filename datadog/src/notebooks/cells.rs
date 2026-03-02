use chrono::{DateTime, Utc};
use datadog_api_client::datadogV1::model::{
    FormulaAndFunctionQueryDefinition, LogQueryDefinition, LogQueryDefinitionGroupBy,
    LogQueryDefinitionSearch, LogStreamWidgetDefinition, LogStreamWidgetDefinitionType,
    LogsQueryCompute, NotebookAbsoluteTime, NotebookCellCreateRequest,
    NotebookCellCreateRequestAttributes, NotebookCellResourceType,
    NotebookCellResponseAttributes, NotebookCellTime, NotebookCellUpdateRequestAttributes,
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
    EventQuery(EventQueryCell),
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

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct EventQueryCell {
    pub data_source: String,
    pub search: String,
    pub compute: String,
    pub metric: Option<String>,
    pub group_by: Option<Vec<EventQueryGroupBy>>,
    pub title: Option<String>,
    pub display_type: Option<String>,
    pub time: Option<CellTime>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct EventQueryGroupBy {
    pub facet: String,
    pub limit: Option<i64>,
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

/// Build a `LogQueryDefinition` from an `EventQueryCell`.
fn build_event_log_query(eq: &EventQueryCell) -> LogQueryDefinition {
    let mut compute = LogsQueryCompute::new(eq.compute.clone());
    if let Some(ref m) = eq.metric {
        compute = compute.facet(m.clone());
    }

    let mut def = LogQueryDefinition::new()
        .compute(compute)
        .search(LogQueryDefinitionSearch::new(eq.search.clone()));

    if let Some(ref groups) = eq.group_by {
        let sdk_groups: Vec<LogQueryDefinitionGroupBy> = groups
            .iter()
            .map(|g| {
                let mut gb = LogQueryDefinitionGroupBy::new(g.facet.clone());
                if let Some(limit) = g.limit {
                    gb = gb.limit(limit);
                }
                gb
            })
            .collect();
        def = def.group_by(sdk_groups);
    }

    def
}

/// Map a data_source string to the appropriate setter on `TimeseriesWidgetRequest`.
fn set_data_source_query(
    request: TimeseriesWidgetRequest,
    data_source: &str,
    query: LogQueryDefinition,
) -> TimeseriesWidgetRequest {
    match data_source.to_lowercase().as_str() {
        "logs" => request.log_query(query),
        "rum" => request.rum_query(query),
        "security_signals" => request.security_query(query),
        "audit" => request.audit_query(query),
        "profiles" => request.profile_metrics_query(query),
        "network" => request.network_query(query),
        "apm" | "spans" => request.apm_query(query),
        // "events" and anything else
        _ => request.event_query(query),
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
        Cell::EventQuery(eq) => {
            let log_query = build_event_log_query(eq);

            let mut request = set_data_source_query(
                TimeseriesWidgetRequest::new(),
                &eq.data_source,
                log_query,
            );

            if let Some(ref dt) = eq.display_type {
                request.display_type = Some(match dt.to_lowercase().as_str() {
                    "bars" | "bar" => WidgetDisplayType::BARS,
                    "area" => WidgetDisplayType::AREA,
                    _ => WidgetDisplayType::LINE,
                });
            }

            let mut definition = TimeseriesWidgetDefinition::new(
                vec![request],
                TimeseriesWidgetDefinitionType::TIMESERIES,
            );

            if let Some(ref title) = eq.title {
                definition.title = Some(title.clone());
            }

            let mut attrs = NotebookTimeseriesCellAttributes::new(definition);
            if let Some(cell_time) = &eq.time {
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

/// Convert CreateRequestAttributes to UpdateRequestAttributes.
/// The inner types are identical — only the enum wrapper differs.
pub fn create_attrs_to_update_attrs(
    attrs: &NotebookCellCreateRequestAttributes,
) -> NotebookCellUpdateRequestAttributes {
    match attrs {
        NotebookCellCreateRequestAttributes::NotebookMarkdownCellAttributes(a) => {
            NotebookCellUpdateRequestAttributes::NotebookMarkdownCellAttributes(a.clone())
        }
        NotebookCellCreateRequestAttributes::NotebookTimeseriesCellAttributes(a) => {
            NotebookCellUpdateRequestAttributes::NotebookTimeseriesCellAttributes(a.clone())
        }
        NotebookCellCreateRequestAttributes::NotebookToplistCellAttributes(a) => {
            NotebookCellUpdateRequestAttributes::NotebookToplistCellAttributes(a.clone())
        }
        NotebookCellCreateRequestAttributes::NotebookHeatMapCellAttributes(a) => {
            NotebookCellUpdateRequestAttributes::NotebookHeatMapCellAttributes(a.clone())
        }
        NotebookCellCreateRequestAttributes::NotebookDistributionCellAttributes(a) => {
            NotebookCellUpdateRequestAttributes::NotebookDistributionCellAttributes(a.clone())
        }
        NotebookCellCreateRequestAttributes::NotebookLogStreamCellAttributes(a) => {
            NotebookCellUpdateRequestAttributes::NotebookLogStreamCellAttributes(a.clone())
        }
        _ => panic!("Unsupported cell type for update"),
    }
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

/// Try to extract an event-query JSON object from a timeseries cell. Returns
/// `Some(map)` when the first request uses one of the data-source-specific
/// query fields (`event_query`, `log_query`, `rum_query`, etc.).
fn extract_event_query_json(
    ts: &NotebookTimeseriesCellAttributes,
) -> Option<serde_json::Map<String, serde_json::Value>> {
    let req = ts.definition.requests.first()?;

    // Check each data-source-specific field and map to a data_source string.
    let candidates: [(&str, &Option<LogQueryDefinition>); 8] = [
        ("events", &req.event_query),
        ("logs", &req.log_query),
        ("rum", &req.rum_query),
        ("security_signals", &req.security_query),
        ("audit", &req.audit_query),
        ("profiles", &req.profile_metrics_query),
        ("network", &req.network_query),
        ("apm", &req.apm_query),
    ];
    let (data_source, def) = candidates
        .iter()
        .find_map(|(ds, field)| field.as_ref().map(|d| (ds, d)))?;

    let mut obj = serde_json::Map::new();
    obj.insert(
        "data_source".into(),
        serde_json::Value::String(data_source.to_string()),
    );
    if let Some(ref search) = def.search {
        obj.insert(
            "search".into(),
            serde_json::Value::String(search.query.clone()),
        );
    }
    if let Some(ref compute) = def.compute {
        obj.insert(
            "compute".into(),
            serde_json::Value::String(compute.aggregation.clone()),
        );
        if let Some(ref facet) = compute.facet {
            obj.insert(
                "metric".into(),
                serde_json::Value::String(facet.clone()),
            );
        }
    }
    if let Some(ref groups) = def.group_by {
        let arr: Vec<serde_json::Value> = groups
            .iter()
            .map(|g| {
                let mut m = serde_json::Map::new();
                m.insert(
                    "facet".into(),
                    serde_json::Value::String(g.facet.clone()),
                );
                if let Some(limit) = g.limit {
                    m.insert(
                        "limit".into(),
                        serde_json::Value::Number(serde_json::Number::from(limit)),
                    );
                }
                serde_json::Value::Object(m)
            })
            .collect();
        if !arr.is_empty() {
            obj.insert("group_by".into(), serde_json::Value::Array(arr));
        }
    }
    if let Some(ref dt) = req.display_type {
        obj.insert(
            "display_type".into(),
            serde_json::Value::String(dt.to_string()),
        );
    }
    Some(obj)
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
            // Check if this is an event-query cell by looking for an event
            // query definition in the requests.
            if let Some(event_obj) = extract_event_query_json(ts) {
                let mut obj = event_obj;
                if let Some(title) = &ts.definition.title {
                    obj.insert("title".into(), serde_json::Value::String(title.clone()));
                }
                if let Some(Some(time)) = &ts.time {
                    obj.insert("time".into(), notebook_cell_time_to_json_value(time));
                }
                return format!(
                    "```event-query\n{}\n```",
                    serde_json::to_string_pretty(&obj).unwrap()
                );
            }

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

    #[test]
    fn event_query_cell_count() {
        let cell = Cell::EventQuery(EventQueryCell {
            data_source: "events".to_string(),
            search: "source:deploy env:production".to_string(),
            compute: "count".to_string(),
            metric: None,
            group_by: None,
            title: Some("Deploy Events".to_string()),
            display_type: None,
            time: None,
        });
        let request = cell_to_create_request(&cell);

        assert_eq!(request.type_, NotebookCellResourceType::NOTEBOOK_CELLS);
        match &request.attributes {
            NotebookCellCreateRequestAttributes::NotebookTimeseriesCellAttributes(attrs) => {
                assert_eq!(attrs.definition.requests.len(), 1);
                let req = &attrs.definition.requests[0];
                // Should use event_query, not q or queries/formulas
                assert!(req.q.is_none());
                assert!(req.queries.is_none());
                assert!(req.event_query.is_some());
                assert_eq!(attrs.definition.title.as_deref(), Some("Deploy Events"));
            }
            _ => panic!("Expected NotebookTimeseriesCellAttributes"),
        }
    }

    #[test]
    fn event_query_cell_with_metric_and_group_by() {
        let cell = Cell::EventQuery(EventQueryCell {
            data_source: "events".to_string(),
            search: "source:deploy".to_string(),
            compute: "avg".to_string(),
            metric: Some("@duration".to_string()),
            group_by: Some(vec![EventQueryGroupBy {
                facet: "service".to_string(),
                limit: Some(10),
            }]),
            title: None,
            display_type: Some("bars".to_string()),
            time: Some(CellTime::Relative("4h".to_string())),
        });
        let request = cell_to_create_request(&cell);

        match &request.attributes {
            NotebookCellCreateRequestAttributes::NotebookTimeseriesCellAttributes(attrs) => {
                let req = &attrs.definition.requests[0];
                assert_eq!(req.display_type, Some(WidgetDisplayType::BARS));
                let def = req.event_query.as_ref().unwrap();
                let compute = def.compute.as_ref().unwrap();
                assert_eq!(compute.aggregation, "avg");
                assert_eq!(compute.facet.as_deref(), Some("@duration"));
                let groups = def.group_by.as_ref().unwrap();
                assert_eq!(groups.len(), 1);
                assert_eq!(groups[0].facet, "service");
                assert_eq!(groups[0].limit, Some(10));
                // Verify time
                match &attrs.time {
                    Some(Some(NotebookCellTime::NotebookRelativeTime(rt))) => {
                        assert_eq!(rt.live_span, WidgetLiveSpan::PAST_FOUR_HOURS);
                    }
                    other => panic!("Expected NotebookRelativeTime, got {:?}", other),
                }
            }
            _ => panic!("Expected NotebookTimeseriesCellAttributes"),
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
    fn reverse_event_query_cell() {
        let event_def = LogQueryDefinition::new()
            .compute(LogsQueryCompute::new("count".to_string()))
            .search(LogQueryDefinitionSearch::new("source:deploy".to_string()));

        let request = TimeseriesWidgetRequest::new().event_query(event_def);

        let mut def = TimeseriesWidgetDefinition::new(
            vec![request],
            TimeseriesWidgetDefinitionType::TIMESERIES,
        );
        def.title = Some("Deploy Events".to_string());

        let attrs = NotebookCellResponseAttributes::NotebookTimeseriesCellAttributes(Box::new(
            NotebookTimeseriesCellAttributes::new(def),
        ));
        let md = notebook_cell_to_markdown(&attrs);
        assert!(md.starts_with("```event-query\n"), "got: {}", md);
        assert!(md.ends_with("\n```"));
        assert!(md.contains("\"data_source\": \"events\""));
        assert!(md.contains("\"search\": \"source:deploy\""));
        assert!(md.contains("\"compute\": \"count\""));
        assert!(md.contains("\"title\": \"Deploy Events\""));
    }

    #[test]
    fn reverse_event_query_cell_with_group_by() {
        let event_def = LogQueryDefinition::new()
            .compute(LogsQueryCompute::new("avg".to_string()).facet("@duration".to_string()))
            .search(LogQueryDefinitionSearch::new("source:deploy".to_string()))
            .group_by(vec![
                LogQueryDefinitionGroupBy::new("service".to_string()).limit(10),
            ]);

        let request = TimeseriesWidgetRequest::new().event_query(event_def);

        let def = TimeseriesWidgetDefinition::new(
            vec![request],
            TimeseriesWidgetDefinitionType::TIMESERIES,
        );

        let attrs = NotebookCellResponseAttributes::NotebookTimeseriesCellAttributes(Box::new(
            NotebookTimeseriesCellAttributes::new(def),
        ));
        let md = notebook_cell_to_markdown(&attrs);
        assert!(md.starts_with("```event-query\n"), "got: {}", md);
        assert!(md.contains("\"compute\": \"avg\""));
        assert!(md.contains("\"metric\": \"@duration\""));
        assert!(md.contains("\"group_by\""));
        assert!(md.contains("\"facet\": \"service\""));
        assert!(md.contains("\"limit\": 10"));
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
