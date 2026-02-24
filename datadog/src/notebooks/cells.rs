use chrono::{DateTime, Utc};
use datadog_api_client::datadogV1::model::{
    LogStreamWidgetDefinition, LogStreamWidgetDefinitionType, NotebookAbsoluteTime,
    NotebookCellCreateRequest, NotebookCellCreateRequestAttributes, NotebookCellResourceType,
    NotebookCellTime, NotebookLogStreamCellAttributes, NotebookMarkdownCellAttributes,
    NotebookMarkdownCellDefinition, NotebookMarkdownCellDefinitionType, NotebookRelativeTime,
    NotebookTimeseriesCellAttributes, TimeseriesWidgetDefinition,
    TimeseriesWidgetDefinitionType, TimeseriesWidgetRequest, WidgetLiveSpan,
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
            let request = TimeseriesWidgetRequest::new().q(metric_query.query.clone());
            let definition = TimeseriesWidgetDefinition::new(
                vec![request],
                TimeseriesWidgetDefinitionType::TIMESERIES,
            );
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
}
