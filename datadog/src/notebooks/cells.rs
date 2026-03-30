use chrono::{DateTime, Utc};
use datadog_api_client::datadogV1::model::{
    FormulaAndFunctionMetricDataSource, FormulaAndFunctionMetricQueryDefinition,
    FormulaAndFunctionQueryDefinition, FormulaAndFunctionResponseFormat, LogQueryDefinition, LogQueryDefinitionGroupBy,
    LogQueryDefinitionSearch, LogStreamWidgetDefinition, LogStreamWidgetDefinitionType,
    LogsQueryCompute, NotebookAbsoluteTime, NotebookCellCreateRequest,
    NotebookCellCreateRequestAttributes, NotebookCellResourceType,
    NotebookCellResponseAttributes, NotebookCellTime, NotebookCellUpdateRequestAttributes,
    NotebookLogStreamCellAttributes, NotebookMarkdownCellAttributes,
    NotebookMarkdownCellDefinition, NotebookMarkdownCellDefinitionType, NotebookRelativeTime,
    NotebookTimeseriesCellAttributes, TimeseriesWidgetDefinition,
    TimeseriesWidgetDefinitionType, TimeseriesWidgetExpressionAlias, TimeseriesWidgetRequest,
    WidgetDisplayType, WidgetEvent, WidgetFormula, WidgetLiveSpan,
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
    /// Single metric query string (legacy API path). When `queries` is also
    /// present, this field is ignored.
    #[serde(default)]
    pub query: String,
    /// Multiple metric queries for a single widget (formula-and-functions API).
    /// Each string is a Datadog metric query expression. When present, the
    /// write path builds `FormulaAndFunctionMetricQueryDefinition` objects
    /// with auto-generated names (`query1`, `query2`, ...) and formulas.
    pub queries: Option<Vec<String>>,
    pub time: Option<CellTime>,
    /// Graph title displayed above the timeseries widget.
    pub title: Option<String>,
    /// Display aliases for metric expressions. Maps the query expression
    /// to a human-readable name shown in the legend.
    /// Example: `{"avg:system.cpu.user{*}": "CPU Usage"}`
    pub aliases: Option<std::collections::HashMap<String, String>>,
    /// Display type: "line" (default), "bars", or "area".
    pub display_type: Option<String>,
    /// Event overlays shown as vertical markers on the timeseries graph.
    pub events: Option<Vec<EventOverlay>>,
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
    /// Event overlays shown as vertical markers on the timeseries graph.
    pub events: Option<Vec<EventOverlay>>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct EventQueryGroupBy {
    pub facet: String,
    pub limit: Option<i64>,
}

/// Event overlay query for timeseries widgets. Renders vertical markers on
/// the graph when matching events occur (e.g., deploys, flag changes).
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct EventOverlay {
    pub q: String,
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

/// Split a comma-separated metric query string into individual queries.
///
/// Commas inside `{}` (tag scopes) are left alone; only top-level commas are
/// treated as separators. This lets the `query` field hold multiple Datadog
/// metric expressions the same way the Datadog UI displays them.
fn split_metric_queries(query: &str) -> Vec<String> {
    let mut result = Vec::new();
    let mut current = String::new();
    let mut depth = 0u32;
    for ch in query.chars() {
        match ch {
            '{' => { depth += 1; current.push(ch); }
            '}' => { depth = depth.saturating_sub(1); current.push(ch); }
            ',' if depth == 0 => {
                let trimmed = current.trim().to_string();
                if !trimmed.is_empty() {
                    result.push(trimmed);
                }
                current.clear();
            }
            _ => current.push(ch),
        }
    }
    let trimmed = current.trim().to_string();
    if !trimmed.is_empty() {
        result.push(trimmed);
    }
    result
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
            // Resolve the effective queries list: either the explicit `queries`
            // array, or by splitting a comma-separated `query` string at
            // top-level commas (i.e. commas not inside `{}`).
            let resolved_queries: Vec<String> = if let Some(qs) = metric_query.queries.as_ref() {
                qs.clone()
            } else {
                split_metric_queries(&metric_query.query)
            };
            let use_formulas = resolved_queries.len() > 1;

            let mut request = if use_formulas {
                // Multi-query: use the formula-and-functions API.
                let queries_vec = &resolved_queries;
                let ff_queries: Vec<FormulaAndFunctionQueryDefinition> = queries_vec
                    .iter()
                    .enumerate()
                    .map(|(i, q)| {
                        FormulaAndFunctionQueryDefinition::FormulaAndFunctionMetricQueryDefinition(
                            Box::new(FormulaAndFunctionMetricQueryDefinition::new(
                                FormulaAndFunctionMetricDataSource::METRICS,
                                format!("query{}", i + 1),
                                q.clone(),
                            )),
                        )
                    })
                    .collect();
                let aliases = metric_query.aliases.as_ref();
                let formulas: Vec<WidgetFormula> = queries_vec
                    .iter()
                    .enumerate()
                    .map(|(i, q)| {
                        let mut f = WidgetFormula::new(format!("query{}", i + 1));
                        if let Some(alias) = aliases.and_then(|a| a.get(q.as_str())) {
                            f.alias = Some(alias.clone());
                        }
                        f
                    })
                    .collect();
                TimeseriesWidgetRequest::new()
                    .queries(ff_queries)
                    .formulas(formulas)
                    .response_format(FormulaAndFunctionResponseFormat::TIMESERIES)
            } else {
                // Single query: use the legacy `q` field.
                let q = resolved_queries.first().cloned()
                    .unwrap_or_else(|| metric_query.query.clone());
                let mut req = TimeseriesWidgetRequest::new().q(q);

                // Set aliases via metadata (legacy path).
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
                        req.metadata = Some(metadata);
                    }
                }
                req
            };

            // Set display type (line/bars/area).
            // Default to bars for .as_count() queries since count data
            // reads better as a bar chart.
            let any_as_count = resolved_queries.iter().any(|q| q.contains(".as_count()"));
            if let Some(ref dt) = metric_query.display_type {
                request.display_type = Some(match dt.to_lowercase().as_str() {
                    "bars" | "bar" => WidgetDisplayType::BARS,
                    "area" => WidgetDisplayType::AREA,
                    _ => WidgetDisplayType::LINE,
                });
            } else if any_as_count {
                request.display_type = Some(WidgetDisplayType::BARS);
            }

            let mut definition = TimeseriesWidgetDefinition::new(
                vec![request],
                TimeseriesWidgetDefinitionType::TIMESERIES,
            );

            // Set graph title.
            if let Some(ref title) = metric_query.title {
                definition.title = Some(title.clone());
            }

            // Set event overlays.
            if let Some(ref overlays) = metric_query.events {
                definition.events =
                    Some(overlays.iter().map(|e| WidgetEvent::new(e.q.clone())).collect());
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

            request.display_type = Some(match eq.display_type.as_deref() {
                Some(dt) => match dt.to_lowercase().as_str() {
                    "line" => WidgetDisplayType::LINE,
                    "area" => WidgetDisplayType::AREA,
                    _ => WidgetDisplayType::BARS,
                },
                // Default to bars — event data (especially counts) reads
                // better as a bar chart than a line.
                None => WidgetDisplayType::BARS,
            });

            let mut definition = TimeseriesWidgetDefinition::new(
                vec![request],
                TimeseriesWidgetDefinitionType::TIMESERIES,
            );

            if let Some(ref title) = eq.title {
                definition.title = Some(title.clone());
            }

            // Set event overlays.
            if let Some(ref overlays) = eq.events {
                definition.events =
                    Some(overlays.iter().map(|e| WidgetEvent::new(e.q.clone())).collect());
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
/// Extract all metric queries from a formula-and-functions `queries` array.
/// Returns `(name, query_expression)` pairs preserving order.
fn extract_all_metric_queries(
    queries: &[FormulaAndFunctionQueryDefinition],
) -> Vec<(String, String)> {
    queries
        .iter()
        .filter_map(|q| {
            if let FormulaAndFunctionQueryDefinition::FormulaAndFunctionMetricQueryDefinition(def) = q {
                Some((def.name.clone(), def.query.clone()))
            } else {
                None
            }
        })
        .collect()
}

/// Try to extract an event-query JSON object from the formula-and-functions
/// `queries` array (the newer API path). This handles timeseries cells that
/// use `FormulaAndFunctionEventQueryDefinition` rather than the legacy
/// `log_query`/`event_query` fields.
fn extract_ff_event_query_json(
    req: &TimeseriesWidgetRequest,
) -> Option<serde_json::Map<String, serde_json::Value>> {
    let queries = req.queries.as_ref()?;
    for q in queries {
        if let FormulaAndFunctionQueryDefinition::FormulaAndFunctionEventQueryDefinition(def) = q {
            let mut obj = serde_json::Map::new();
            obj.insert(
                "data_source".into(),
                serde_json::Value::String(def.data_source.to_string()),
            );
            if let Some(ref search) = def.search {
                obj.insert(
                    "search".into(),
                    serde_json::Value::String(search.query.clone()),
                );
            }
            obj.insert(
                "compute".into(),
                serde_json::Value::String(def.compute.aggregation.to_string()),
            );
            if let Some(ref metric) = def.compute.metric {
                obj.insert(
                    "metric".into(),
                    serde_json::Value::String(metric.clone()),
                );
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
            return Some(obj);
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

/// Try to extract a log-query JSON object from a `list_stream` cell that was
/// misparsed as a timeseries cell by the SDK. The list_stream request fields
/// (`query`, `columns`, `response_format`) end up in the `TimeseriesWidgetRequest`
/// `additional_properties` since they don't map to named timeseries fields.
fn extract_list_stream_log_query(
    ts: &NotebookTimeseriesCellAttributes,
) -> Option<serde_json::Map<String, serde_json::Value>> {
    // Only applies when the definition type is not actually "timeseries".
    if matches!(ts.definition.type_, TimeseriesWidgetDefinitionType::TIMESERIES) {
        return None;
    }
    let req = ts.definition.requests.first()?;
    // list_stream requests store the query object in additional_properties.
    let query_val = req.additional_properties.get("query")?;
    let query_obj = query_val.as_object()?;
    let query_string = query_obj.get("query_string")?.as_str()?;

    let mut obj = serde_json::Map::new();
    obj.insert(
        "query".into(),
        serde_json::Value::String(query_string.to_string()),
    );

    // Extract columns (list_stream format: [{field, width}, ...] → just the field names).
    if let Some(columns_val) = req.additional_properties.get("columns") {
        if let Some(columns_arr) = columns_val.as_array() {
            let col_names: Vec<serde_json::Value> = columns_arr
                .iter()
                .filter_map(|c| c.get("field")?.as_str().map(|s| serde_json::Value::String(s.to_string())))
                // Skip the standard columns that log-query always shows.
                .filter(|v| !matches!(v.as_str(), Some("status_line" | "timestamp" | "content")))
                .collect();
            if !col_names.is_empty() {
                obj.insert("columns".into(), serde_json::Value::Array(col_names));
            }
        }
    }

    // Extract indexes from the query object if non-empty.
    if let Some(indexes_val) = query_obj.get("indexes") {
        if let Some(indexes_arr) = indexes_val.as_array() {
            if !indexes_arr.is_empty() {
                obj.insert("indexes".into(), serde_json::Value::Array(indexes_arr.clone()));
            }
        }
    }

    Some(obj)
}

/// Convert a list of `WidgetEvent` to a JSON array value for embedding in
/// the fenced code block output.
fn widget_events_to_json(events: &[WidgetEvent]) -> serde_json::Value {
    let arr: Vec<serde_json::Value> = events
        .iter()
        .map(|e| serde_json::json!({ "q": e.q }))
        .collect();
    serde_json::Value::Array(arr)
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
            // The SDK may misparse list_stream cells as timeseries because the
            // NotebookCellResponseAttributes deserializer tries timeseries
            // before checking the inner definition type. Detect and handle.
            if let Some(log_obj) = extract_list_stream_log_query(ts) {
                let mut obj = log_obj;
                if let Some(Some(time)) = &ts.time {
                    obj.insert("time".into(), notebook_cell_time_to_json_value(time));
                }
                return format!(
                    "```log-query\n{}\n```",
                    serde_json::to_string_pretty(&obj).unwrap()
                );
            }

            // Check if this is an event-query cell by looking for an event
            // query definition in the requests (legacy fields first, then the
            // newer formula-and-functions queries array).
            let event_obj = extract_event_query_json(ts).or_else(|| {
                ts.definition.requests.first().and_then(extract_ff_event_query_json)
            });
            if let Some(event_obj) = event_obj {
                let mut obj = event_obj;
                if let Some(title) = &ts.definition.title {
                    obj.insert("title".into(), serde_json::Value::String(title.clone()));
                }
                if let Some(ref events) = ts.definition.events {
                    if !events.is_empty() {
                        obj.insert("events".into(), widget_events_to_json(events));
                    }
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
                } else if let Some(queries) = &req.queries {
                    // Extract all metric queries from the formula-and-functions array.
                    let metric_queries = extract_all_metric_queries(queries);
                    if metric_queries.len() > 1 {
                        // Multi-query: emit as "queries" array.
                        let arr: Vec<serde_json::Value> = metric_queries.iter()
                            .map(|(_, q)| serde_json::Value::String(q.clone()))
                            .collect();
                        obj.insert("queries".into(), serde_json::Value::Array(arr));

                        // Build aliases from formulas (formula name → alias).
                        if let Some(formulas) = &req.formulas {
                            let mut aliases = serde_json::Map::new();
                            for formula in formulas {
                                if let Some(alias) = &formula.alias {
                                    // Map formula name (e.g. "query1") back to the
                                    // actual query expression.
                                    if let Some((_, query_expr)) = metric_queries.iter()
                                        .find(|(name, _)| *name == formula.formula)
                                    {
                                        aliases.insert(
                                            query_expr.clone(),
                                            serde_json::Value::String(alias.clone()),
                                        );
                                    }
                                }
                            }
                            if !aliases.is_empty() {
                                obj.insert("aliases".into(), serde_json::Value::Object(aliases));
                            }
                        }
                    } else if let Some((_, q)) = metric_queries.first() {
                        // Single query in the array: emit as legacy "query" string.
                        obj.insert("query".into(), serde_json::Value::String(q.clone()));

                        // Single-query formula alias.
                        if let Some(formulas) = &req.formulas {
                            let mut aliases = serde_json::Map::new();
                            for formula in formulas {
                                if let Some(alias) = &formula.alias {
                                    if let Some((_, query_expr)) = metric_queries.iter()
                                        .find(|(name, _)| *name == formula.formula)
                                    {
                                        aliases.insert(
                                            query_expr.clone(),
                                            serde_json::Value::String(alias.clone()),
                                        );
                                    }
                                }
                            }
                            if !aliases.is_empty() {
                                obj.insert("aliases".into(), serde_json::Value::Object(aliases));
                            }
                        }
                    }
                }
                if let Some(dt) = &req.display_type {
                    obj.insert("display_type".into(), serde_json::Value::String(dt.to_string()));
                }
                // Legacy metadata aliases (only when using the `q` field path).
                if obj.contains_key("query") && !obj.contains_key("aliases") {
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
            }
            if let Some(title) = &ts.definition.title {
                obj.insert("title".into(), serde_json::Value::String(title.clone()));
            }
            if let Some(ref events) = ts.definition.events {
                if !events.is_empty() {
                    obj.insert("events".into(), widget_events_to_json(events));
                }
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

/// Emit template variables as a YAML frontmatter block for round-tripping.
pub fn template_variables_to_frontmatter(vars: &serde_json::Value) -> String {
    // Wrap in a mapping with a "variables" key so the YAML matches the
    // frontmatter format the parser expects.
    let wrapper = serde_json::json!({ "variables": vars });
    let yaml = serde_yaml::to_string(&wrapper).unwrap_or_default();
    format!("---\n{yaml}---\n\n")
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
            events: None,
            queries: None,
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
            events: None,
            queries: None,
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
    fn metric_query_as_count_defaults_to_bars() {
        let cell = Cell::MetricQuery(MetricQueryCell {
            query: "count:my.metric{env:production}.as_count()".to_string(),
            time: None,
            title: None,
            aliases: None,
            display_type: None,
            events: None,
            queries: None,
        });
        let request = cell_to_create_request(&cell);

        match &request.attributes {
            NotebookCellCreateRequestAttributes::NotebookTimeseriesCellAttributes(attrs) => {
                assert_eq!(
                    attrs.definition.requests[0].display_type,
                    Some(WidgetDisplayType::BARS)
                );
            }
            _ => panic!("Expected NotebookTimeseriesCellAttributes"),
        }
    }

    #[test]
    fn metric_query_as_count_explicit_line_overrides() {
        let cell = Cell::MetricQuery(MetricQueryCell {
            query: "count:my.metric{env:production}.as_count()".to_string(),
            time: None,
            title: None,
            aliases: None,
            display_type: Some("line".to_string()),
            events: None,
            queries: None,
        });
        let request = cell_to_create_request(&cell);

        match &request.attributes {
            NotebookCellCreateRequestAttributes::NotebookTimeseriesCellAttributes(attrs) => {
                assert_eq!(
                    attrs.definition.requests[0].display_type,
                    Some(WidgetDisplayType::LINE)
                );
            }
            _ => panic!("Expected NotebookTimeseriesCellAttributes"),
        }
    }

    #[test]
    fn split_metric_queries_single() {
        let result = split_metric_queries("avg:system.cpu.user{env:prod}");
        assert_eq!(result, vec!["avg:system.cpu.user{env:prod}"]);
    }

    #[test]
    fn split_metric_queries_comma_separated() {
        let result = split_metric_queries(
            "sum:foo{$env}.as_count(), sum:bar{$env}.as_count()"
        );
        assert_eq!(result, vec![
            "sum:foo{$env}.as_count()",
            "sum:bar{$env}.as_count()",
        ]);
    }

    #[test]
    fn split_metric_queries_preserves_commas_in_braces() {
        let result = split_metric_queries(
            "avg:metric{service:web,env:prod}, avg:metric{service:api,env:prod}"
        );
        assert_eq!(result, vec![
            "avg:metric{service:web,env:prod}",
            "avg:metric{service:api,env:prod}",
        ]);
    }

    #[test]
    fn split_metric_queries_four_percentiles() {
        let result = split_metric_queries(
            "p50:m{$env}, p90:m{$env}, p95:m{$env}, p99:m{$env}"
        );
        assert_eq!(result.len(), 4);
    }

    #[test]
    fn comma_separated_query_uses_formulas() {
        // The exact format from the cortex-gate session file.
        let cell = Cell::MetricQuery(MetricQueryCell {
            query: "sum:cortex.a{$env}.as_count(), sum:cortex.b{$env}.as_count()".to_string(),
            time: None,
            title: Some("Test".to_string()),
            aliases: Some(std::collections::HashMap::from([
                ("sum:cortex.a{$env}.as_count()".to_string(), "A".to_string()),
                ("sum:cortex.b{$env}.as_count()".to_string(), "B".to_string()),
            ])),
            display_type: None,
            events: None,
            queries: None,
        });
        let request = cell_to_create_request(&cell);

        match &request.attributes {
            NotebookCellCreateRequestAttributes::NotebookTimeseriesCellAttributes(attrs) => {
                let req = &attrs.definition.requests[0];
                assert!(req.queries.is_some(), "should use formula-and-functions queries");
                assert_eq!(req.queries.as_ref().unwrap().len(), 2);
                assert_eq!(
                    req.response_format,
                    Some(FormulaAndFunctionResponseFormat::TIMESERIES),
                );
                // Verify aliases are mapped to formulas.
                let formulas = req.formulas.as_ref().unwrap();
                let aliased: Vec<_> = formulas.iter().filter(|f| f.alias.is_some()).collect();
                assert_eq!(aliased.len(), 2, "both formulas should have aliases");
            }
            _ => panic!("Expected NotebookTimeseriesCellAttributes"),
        }
    }

    #[test]
    fn multi_query_metric_cell_sets_response_format() {
        let cell = Cell::MetricQuery(MetricQueryCell {
            query: String::new(),
            time: None,
            title: Some("New vs Legacy".to_string()),
            aliases: None,
            display_type: None,
            events: None,
            queries: Some(vec![
                "count:trace.http.request{service:foo}.as_count()".to_string(),
                "count:trace.http.request{service:bar}.as_count()".to_string(),
            ]),
        });
        let request = cell_to_create_request(&cell);

        match &request.attributes {
            NotebookCellCreateRequestAttributes::NotebookTimeseriesCellAttributes(attrs) => {
                let req = &attrs.definition.requests[0];
                assert!(req.queries.is_some(), "should use formula-and-functions queries");
                assert!(req.formulas.is_some(), "should have formulas");
                assert_eq!(
                    req.response_format,
                    Some(FormulaAndFunctionResponseFormat::TIMESERIES),
                    "response_format must be TIMESERIES for formula queries"
                );
                // Inspect the serialized JSON to verify the payload structure.
                let json = serde_json::to_string_pretty(&attrs.definition).unwrap();
                eprintln!("Serialized multi-query definition:\n{json}");
                // Verify the JSON contains the expected fields.
                let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
                let req_json = &parsed["requests"][0];
                assert!(req_json["queries"].is_array(), "queries should be an array in JSON");
                assert!(req_json["formulas"].is_array(), "formulas should be an array in JSON");
                assert_eq!(req_json["response_format"], "timeseries");
                // Ensure no legacy `q` field is present.
                assert!(req_json["q"].is_null(), "legacy q field should not be present");
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
            events: None,
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
            events: None,
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
