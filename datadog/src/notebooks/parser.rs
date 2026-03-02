use anyhow::{bail, Context};

use super::cells::{Cell, LogQueryCell, MetricQueryCell};

#[derive(Debug)]
enum State {
    Normal,
    InRegularFence,
    InLogQuery,
    InMetricQuery,
}

/// Parse a markdown document into a sequence of notebook cells.
///
/// Prose becomes `Cell::Markdown`, and ` ```log-query ` fenced blocks become
/// `Cell::LogQuery` (their body is parsed as JSON).
pub fn parse_markdown(input: &str) -> anyhow::Result<Vec<Cell>> {
    let mut cells = Vec::new();
    let mut state = State::Normal;
    let mut buffer = String::new();

    for line in input.lines() {
        let trimmed = line.trim();

        match state {
            State::Normal => {
                if trimmed.strip_prefix("```").is_some_and(|rest| {
                    let tag = rest.trim();
                    tag.eq_ignore_ascii_case("log-query")
                }) {
                    flush_markdown(&mut buffer, &mut cells);
                    state = State::InLogQuery;
                } else if trimmed.strip_prefix("```").is_some_and(|rest| {
                    let tag = rest.trim();
                    tag.eq_ignore_ascii_case("metric-query")
                }) {
                    flush_markdown(&mut buffer, &mut cells);
                    state = State::InMetricQuery;
                } else if trimmed.starts_with("```") && trimmed.len() > 3 {
                    // Opening a regular fenced code block (e.g. ```python).
                    buffer.push_str(line);
                    buffer.push('\n');
                    state = State::InRegularFence;
                } else {
                    buffer.push_str(line);
                    buffer.push('\n');
                }
            }
            State::InRegularFence => {
                buffer.push_str(line);
                buffer.push('\n');
                if trimmed == "```" {
                    state = State::Normal;
                }
            }
            State::InLogQuery => {
                if trimmed == "```" {
                    let json_str = buffer.trim();
                    let log_query: LogQueryCell = serde_json::from_str(json_str)
                        .with_context(|| {
                            format!("Invalid JSON in log-query block: {json_str}")
                        })?;
                    cells.push(Cell::LogQuery(log_query));
                    buffer.clear();
                    state = State::Normal;
                } else {
                    buffer.push_str(line);
                    buffer.push('\n');
                }
            }
            State::InMetricQuery => {
                if trimmed == "```" {
                    let json_str = buffer.trim();
                    let metric_query: MetricQueryCell = serde_json::from_str(json_str)
                        .with_context(|| {
                            format!("Invalid JSON in metric-query block: {json_str}")
                        })?;
                    cells.push(Cell::MetricQuery(metric_query));
                    buffer.clear();
                    state = State::Normal;
                } else {
                    buffer.push_str(line);
                    buffer.push('\n');
                }
            }
        }
    }

    match state {
        State::InLogQuery => bail!("Unterminated log-query code block"),
        State::InMetricQuery => bail!("Unterminated metric-query code block"),
        State::InRegularFence => {
            // Unterminated regular fence — just treat everything as markdown.
            flush_markdown(&mut buffer, &mut cells);
        }
        State::Normal => {
            flush_markdown(&mut buffer, &mut cells);
        }
    }

    Ok(cells)
}

/// Validate that all `[text](#slug)` section links point to a heading that
/// exists in the document. Returns a list of broken link slugs.
pub fn validate_section_links(cells: &[Cell]) -> Vec<String> {
    // Collect all heading slugs
    let mut heading_slugs = std::collections::HashSet::new();
    for cell in cells {
        if let Cell::Markdown(text) = cell {
            for line in text.lines() {
                let trimmed = line.trim();
                let hash_count = trimmed.bytes().take_while(|&b| b == b'#').count();
                if hash_count >= 1
                    && hash_count <= 6
                    && trimmed.as_bytes().get(hash_count) == Some(&b' ')
                {
                    let heading = trimmed[hash_count..].trim();
                    if !heading.is_empty() {
                        let slug = slugify(heading);
                        heading_slugs.insert(slug);
                    }
                }
            }
        }
    }

    // Find all link targets and check against headings
    let mut broken = Vec::new();
    for cell in cells {
        if let Cell::Markdown(text) = cell {
            // Find ](#slug) patterns
            let mut rest = text.as_str();
            while let Some(pos) = rest.find("](#") {
                let after = &rest[pos + 3..];
                if let Some(end) = after.find(')') {
                    let slug = &after[..end];
                    let normalized = slug.replace(|c: char| !c.is_alphanumeric() && c != '-', "-")
                        .to_lowercase();
                    let normalized = normalized.trim_matches('-');
                    let collapsed = collapsed_hyphens(&normalized);
                    if !heading_slugs.contains(&collapsed) {
                        broken.push(slug.to_string());
                    }
                    rest = &after[end..];
                } else {
                    break;
                }
            }
        }
    }
    broken
}

fn slugify(heading: &str) -> String {
    let raw: String = heading
        .to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '-' })
        .collect();
    collapsed_hyphens(&raw)
}

fn collapsed_hyphens(s: &str) -> String {
    let mut result = String::new();
    let mut prev_dash = true;
    for c in s.chars() {
        if c == '-' {
            if !prev_dash {
                result.push('-');
                prev_dash = true;
            }
        } else {
            result.push(c);
            prev_dash = false;
        }
    }
    if result.ends_with('-') {
        result.pop();
    }
    result
}

/// If `buffer` contains non-whitespace content, push it as a `Cell::Markdown`
/// and clear the buffer. Leading/trailing blank lines are stripped.
fn flush_markdown(buffer: &mut String, cells: &mut Vec<Cell>) {
    let trimmed = trim_blank_lines(buffer);
    if !trimmed.is_empty() {
        cells.push(Cell::Markdown(trimmed));
    }
    buffer.clear();
}

/// Strip leading and trailing blank lines, but preserve internal structure.
fn trim_blank_lines(s: &str) -> String {
    let lines: Vec<&str> = s.lines().collect();
    let start = lines.iter().position(|l| !l.trim().is_empty()).unwrap_or(0);
    let end = lines
        .iter()
        .rposition(|l| !l.trim().is_empty())
        .map_or(0, |i| i + 1);
    if start >= end {
        return String::new();
    }
    lines[start..end].join("\n")
}

#[cfg(test)]
mod tests {
    use super::super::cells;
    use super::*;

    #[test]
    fn pure_markdown() {
        let input = "# Hello\nText";
        let cells = parse_markdown(input).unwrap();
        assert_eq!(cells, vec![Cell::Markdown("# Hello\nText".to_string())]);
    }

    #[test]
    fn single_log_query() {
        let input = "```log-query\n{\"query\":\"env:prod\"}\n```";
        let cells = parse_markdown(input).unwrap();
        assert_eq!(
            cells,
            vec![Cell::LogQuery(LogQueryCell {
                query: "env:prod".to_string(),
                indexes: None,
                columns: None,
                time: None,
            })]
        );
    }

    #[test]
    fn mixed_md_logquery_md() {
        let input = "# Title\n\n```log-query\n{\"query\":\"env:prod\"}\n```\n\nFooter text";
        let cells = parse_markdown(input).unwrap();
        assert_eq!(cells.len(), 3);
        assert_eq!(cells[0], Cell::Markdown("# Title".to_string()));
        assert_eq!(
            cells[1],
            Cell::LogQuery(LogQueryCell {
                query: "env:prod".to_string(),
                indexes: None,
                columns: None,
                time: None,
            })
        );
        assert_eq!(cells[2], Cell::Markdown("Footer text".to_string()));
    }

    #[test]
    fn multiple_log_query_blocks() {
        let input =
            "```log-query\n{\"query\":\"a\"}\n```\n\nMiddle\n\n```log-query\n{\"query\":\"b\"}\n```";
        let cells = parse_markdown(input).unwrap();
        assert_eq!(cells.len(), 3);
        assert_eq!(
            cells[0],
            Cell::LogQuery(LogQueryCell {
                query: "a".to_string(),
                indexes: None,
                columns: None,
                time: None,
            })
        );
        assert!(matches!(&cells[1], Cell::Markdown(_)));
        assert_eq!(
            cells[2],
            Cell::LogQuery(LogQueryCell {
                query: "b".to_string(),
                indexes: None,
                columns: None,
                time: None,
            })
        );
    }

    #[test]
    fn adjacent_log_query_blocks_no_empty_markdown() {
        let input = "```log-query\n{\"query\":\"a\"}\n```\n```log-query\n{\"query\":\"b\"}\n```";
        let cells = parse_markdown(input).unwrap();
        assert_eq!(cells.len(), 2);
        assert!(matches!(&cells[0], Cell::LogQuery(_)));
        assert!(matches!(&cells[1], Cell::LogQuery(_)));
    }

    #[test]
    fn log_query_with_indexes() {
        let input = "```log-query\n{\"query\":\"env:prod\",\"indexes\":[\"main\"]}\n```";
        let cells = parse_markdown(input).unwrap();
        assert_eq!(
            cells,
            vec![Cell::LogQuery(LogQueryCell {
                query: "env:prod".to_string(),
                indexes: Some(vec!["main".to_string()]),
                columns: None,
                time: None,
            })]
        );
    }

    #[test]
    fn regular_code_fence_preserved_as_markdown() {
        let input = "```python\nprint(\"hi\")\n```";
        let cells = parse_markdown(input).unwrap();
        assert_eq!(cells.len(), 1);
        match &cells[0] {
            Cell::Markdown(text) => {
                assert!(text.contains("```python"));
                assert!(text.contains("print(\"hi\")"));
                assert!(text.contains("```"));
            }
            _ => panic!("Expected Markdown cell"),
        }
    }

    #[test]
    fn log_query_tag_inside_regular_fence_not_special() {
        let input = "```markdown\n```log-query\n{\"query\":\"a\"}\n```\n```";
        let cells = parse_markdown(input).unwrap();
        assert_eq!(cells.len(), 1);
        assert!(matches!(&cells[0], Cell::Markdown(_)));
    }

    #[test]
    fn invalid_json_in_log_query() {
        let input = "```log-query\nnot json\n```";
        let result = parse_markdown(input);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("Invalid JSON in log-query block"), "{}", err);
    }

    #[test]
    fn missing_query_field() {
        let input = "```log-query\n{\"indexes\":[\"main\"]}\n```";
        let result = parse_markdown(input);
        assert!(result.is_err());
    }

    #[test]
    fn unterminated_log_query_block() {
        let input = "```log-query\n{\"query\":\"a\"}";
        let result = parse_markdown(input);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("Unterminated log-query code block"),
            "{}",
            err
        );
    }

    #[test]
    fn empty_document() {
        let cells = parse_markdown("").unwrap();
        assert!(cells.is_empty());
    }

    #[test]
    fn whitespace_only() {
        let cells = parse_markdown("  \n  ").unwrap();
        assert!(cells.is_empty());
    }

    #[test]
    fn trailing_spaces_on_fence() {
        let input = "```log-query   \n{\"query\":\"env:prod\"}\n```";
        let cells = parse_markdown(input).unwrap();
        assert_eq!(
            cells,
            vec![Cell::LogQuery(LogQueryCell {
                query: "env:prod".to_string(),
                indexes: None,
                columns: None,
                time: None,
            })]
        );
    }

    #[test]
    fn log_query_with_relative_time() {
        let input = "```log-query\n{\"query\":\"env:prod\",\"time\":\"4h\"}\n```";
        let cells = parse_markdown(input).unwrap();
        assert_eq!(
            cells,
            vec![Cell::LogQuery(LogQueryCell {
                query: "env:prod".to_string(),
                indexes: None,
                columns: None,
                time: Some(cells::CellTime::Relative("4h".to_string())),
            })]
        );
    }

    #[test]
    fn log_query_with_absolute_time() {
        let input = "```log-query\n{\"query\":\"env:prod\",\"time\":{\"start\":\"2026-02-20T00:00:00Z\",\"end\":\"2026-02-24T00:00:00Z\"}}\n```";
        let cells = parse_markdown(input).unwrap();
        assert_eq!(
            cells,
            vec![Cell::LogQuery(LogQueryCell {
                query: "env:prod".to_string(),
                indexes: None,
                columns: None,
                time: Some(cells::CellTime::Absolute {
                    start: "2026-02-20T00:00:00Z".parse().unwrap(),
                    end: "2026-02-24T00:00:00Z".parse().unwrap(),
                }),
            })]
        );
    }

    #[test]
    fn single_metric_query() {
        let input =
            "```metric-query\n{\"query\":\"avg:system.cpu.user{env:production}\"}\n```";
        let cells = parse_markdown(input).unwrap();
        assert_eq!(
            cells,
            vec![Cell::MetricQuery(MetricQueryCell {
                query: "avg:system.cpu.user{env:production}".to_string(),
                time: None,
                title: None,
                aliases: None,
                display_type: None,
            })]
        );
    }

    #[test]
    fn metric_query_with_time() {
        let input =
            "```metric-query\n{\"query\":\"avg:system.cpu.user{*}\",\"time\":\"4h\"}\n```";
        let cells = parse_markdown(input).unwrap();
        assert_eq!(
            cells,
            vec![Cell::MetricQuery(MetricQueryCell {
                query: "avg:system.cpu.user{*}".to_string(),
                time: Some(cells::CellTime::Relative("4h".to_string())),
                title: None,
                aliases: None,
                display_type: None,
            })]
        );
    }

    #[test]
    fn mixed_log_and_metric_queries() {
        let input = "# Title\n\n```log-query\n{\"query\":\"env:prod\"}\n```\n\n```metric-query\n{\"query\":\"avg:system.cpu.user{*}\"}\n```";
        let cells = parse_markdown(input).unwrap();
        assert_eq!(cells.len(), 3);
        assert!(matches!(&cells[0], Cell::Markdown(_)));
        assert!(matches!(&cells[1], Cell::LogQuery(_)));
        assert!(matches!(&cells[2], Cell::MetricQuery(_)));
    }

    #[test]
    fn unterminated_metric_query_block() {
        let input = "```metric-query\n{\"query\":\"avg:system.cpu.user{*}\"}";
        let result = parse_markdown(input);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("Unterminated metric-query code block"),
            "{}",
            err
        );
    }

    #[test]
    fn invalid_json_in_metric_query() {
        let input = "```metric-query\nnot json\n```";
        let result = parse_markdown(input);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("Invalid JSON in metric-query block"),
            "{}",
            err
        );
    }

    // -- validate_section_links --

    #[test]
    fn validate_links_all_valid() {
        let cells = vec![
            Cell::Markdown("## Intro\n[go](#details)".to_string()),
            Cell::Markdown("## Details\ntext".to_string()),
        ];
        assert!(validate_section_links(&cells).is_empty());
    }

    #[test]
    fn validate_links_broken() {
        let cells = vec![
            Cell::Markdown("## Intro\n[go](#nonexistent)\n[also](#details)".to_string()),
            Cell::Markdown("## Details\ntext".to_string()),
        ];
        let broken = validate_section_links(&cells);
        assert_eq!(broken, vec!["nonexistent"]);
    }

    #[test]
    fn validate_links_normalizes_hyphens() {
        // Link has multiple hyphens, heading slug has single — should match
        let cells = vec![
            Cell::Markdown("[go](#regression-onset----wed-feb-18)".to_string()),
            Cell::Markdown("## Regression Onset — Wed Feb 18\ntext".to_string()),
        ];
        assert!(validate_section_links(&cells).is_empty());
    }
}
