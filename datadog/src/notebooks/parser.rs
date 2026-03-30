use anyhow::{bail, Context};

use super::cells::{Cell, EventQueryCell, LogQueryCell, MetricQueryCell};

/// Result of parsing a markdown document: cells plus optional frontmatter metadata.
#[derive(Debug)]
pub struct ParseResult {
    pub cells: Vec<Cell>,
    pub template_variables: Option<serde_json::Value>,
}

#[derive(Debug)]
enum State {
    Normal,
    InRegularFence,
    InLogQuery,
    InMetricQuery,
    InEventQuery,
}

/// Strip a YAML frontmatter block from the top of the input (delimited by `---`).
/// Returns the parsed `variables` value (if present) and the remaining content.
fn parse_frontmatter(input: &str) -> anyhow::Result<(Option<serde_json::Value>, &str)> {
    let trimmed = input.trim_start();
    if !trimmed.starts_with("---") {
        return Ok((None, input));
    }

    // Find the closing `---`.
    let after_open = &trimmed[3..];
    // Skip the rest of the opening `---` line.
    let after_open = match after_open.find('\n') {
        Some(pos) => &after_open[pos + 1..],
        None => return Ok((None, input)), // only `---` with no closing
    };

    let close_pos = match after_open.find("\n---") {
        Some(pos) => pos,
        None => return Ok((None, input)),
    };

    let yaml_block = &after_open[..close_pos];
    let after_close = &after_open[close_pos + 4..]; // skip `\n---`
    // Skip the rest of the closing `---` line.
    let remaining = match after_close.find('\n') {
        Some(pos) => &after_close[pos + 1..],
        None => "",
    };

    let parsed: serde_json::Value = serde_yaml::from_str(yaml_block)
        .with_context(|| format!("Invalid YAML in frontmatter: {yaml_block}"))?;

    let template_variables = parsed.get("variables").cloned();

    Ok((template_variables, remaining))
}

/// Parse a markdown document into a sequence of notebook cells.
///
/// Prose becomes `Cell::Markdown`, and ` ```log-query ` fenced blocks become
/// `Cell::LogQuery` (their body is parsed as JSON).
///
/// If the document starts with a YAML frontmatter block (`---` delimited),
/// the `variables` key is extracted as template variables.
pub fn parse_markdown(input: &str) -> anyhow::Result<ParseResult> {
    let (template_variables, content) = parse_frontmatter(input)?;

    let mut cells = Vec::new();
    let mut state = State::Normal;
    let mut buffer = String::new();

    for line in content.lines() {
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
                } else if trimmed.strip_prefix("```").is_some_and(|rest| {
                    let tag = rest.trim();
                    tag.eq_ignore_ascii_case("event-query")
                }) {
                    flush_markdown(&mut buffer, &mut cells);
                    state = State::InEventQuery;
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
            State::InEventQuery => {
                if trimmed == "```" {
                    let json_str = buffer.trim();
                    let event_query: EventQueryCell = serde_json::from_str(json_str)
                        .with_context(|| {
                            format!("Invalid JSON in event-query block: {json_str}")
                        })?;
                    cells.push(Cell::EventQuery(event_query));
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
        State::InEventQuery => bail!("Unterminated event-query code block"),
        State::InRegularFence => {
            // Unterminated regular fence — just treat everything as markdown.
            flush_markdown(&mut buffer, &mut cells);
        }
        State::Normal => {
            flush_markdown(&mut buffer, &mut cells);
        }
    }

    Ok(ParseResult {
        cells,
        template_variables,
    })
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

/// Collect all query strings from cells as (label, value) pairs.
fn collect_query_strings(cells: &[Cell]) -> Vec<(&str, &str)> {
    let mut out = Vec::new();
    for cell in cells {
        match cell {
            Cell::LogQuery(lq) => {
                out.push(("log-query", lq.query.as_str()));
            }
            Cell::MetricQuery(mq) => {
                if let Some(ref queries) = mq.queries {
                    for q in queries {
                        out.push(("metric-query", q.as_str()));
                    }
                } else {
                    out.push(("metric-query", mq.query.as_str()));
                }
                if let Some(ref events) = mq.events {
                    for e in events {
                        out.push(("metric-query event overlay", e.q.as_str()));
                    }
                }
            }
            Cell::EventQuery(eq) => {
                out.push(("event-query search", eq.search.as_str()));
                if let Some(ref events) = eq.events {
                    for e in events {
                        out.push(("event-query event overlay", e.q.as_str()));
                    }
                }
            }
            Cell::Markdown(_) => {}
        }
    }
    out
}

/// Find all `$name` references in a string. Returns a vec of variable names
/// (without the `$` prefix).
fn find_variable_refs(s: &str) -> Vec<&str> {
    let mut refs = Vec::new();
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'$' {
            let start = i + 1;
            let mut end = start;
            // Variable names: alphanumeric + underscore.
            while end < bytes.len()
                && (bytes[end].is_ascii_alphanumeric() || bytes[end] == b'_')
            {
                end += 1;
            }
            if end > start {
                refs.push(&s[start..end]);
            }
            i = end;
        } else {
            i += 1;
        }
    }
    refs
}

/// Validate template variable usage in queries. Returns warnings for:
///
/// 1. **Redundant prefix** — when a variable has `prefix: env`, writing
///    `env:$env` doubles the prefix to `env:env:staging`. The correct form
///    is just `$env`.
///
/// 2. **Undefined variable** — using `$foo` in a query when no template
///    variable named `foo` is defined in the frontmatter.
pub fn validate_template_variables(
    cells: &[Cell],
    template_variables: Option<&serde_json::Value>,
) -> Vec<String> {
    let vars = template_variables.and_then(|v| v.as_array());

    // Collect defined variable names and (name, prefix) pairs.
    let mut defined_names: std::collections::HashSet<&str> = std::collections::HashSet::new();
    let mut prefixed_vars: Vec<(&str, &str)> = Vec::new();

    if let Some(arr) = vars {
        for v in arr {
            if let Some(name) = v.get("name").and_then(|n| n.as_str()) {
                defined_names.insert(name);
                if let Some(prefix) = v.get("prefix").and_then(|p| p.as_str()) {
                    prefixed_vars.push((name, prefix));
                }
            }
        }
    }

    let query_strings = collect_query_strings(cells);
    let mut warnings = Vec::new();

    for (label, query) in &query_strings {
        // Check for redundant prefix usage.
        for &(name, prefix) in &prefixed_vars {
            let bad_pattern = format!("{}:${}", prefix, name);
            if query.contains(&bad_pattern) {
                warnings.push(format!(
                    "{} contains \"{}\": the ${} variable already applies \
                     the \"{}:\" prefix, so this becomes \"{}:{}:<value>\". \
                     Use just \"${}\" instead.",
                    label, bad_pattern, name, prefix, prefix, prefix, name,
                ));
            }
        }

        // Check for undefined variable references (only when frontmatter
        // declares a variables array, since without frontmatter the variables
        // may be defined server-side on the notebook).
        if vars.is_some() {
            for var_name in find_variable_refs(query) {
                if !defined_names.contains(var_name) {
                    warnings.push(format!(
                        "{} references undefined variable \"${}\"",
                        label, var_name,
                    ));
                }
            }
        }
    }

    warnings
}

/// Warn when queries hardcode a value for a tag that has a corresponding
/// template variable with a prefix. For example, if `$env` is defined with
/// `prefix: env`, writing `env:staging` in a query defeats the purpose of
/// the variable. Returns warnings (not errors) that can be bypassed with
/// `--ack-warnings`.
pub fn warn_hardcoded_variable_values(
    cells: &[Cell],
    template_variables: Option<&serde_json::Value>,
) -> Vec<String> {
    let vars = template_variables.and_then(|v| v.as_array());

    let mut prefixed_vars: Vec<(&str, &str)> = Vec::new();
    if let Some(arr) = vars {
        for v in arr {
            if let (Some(name), Some(prefix)) = (
                v.get("name").and_then(|n| n.as_str()),
                v.get("prefix").and_then(|p| p.as_str()),
            ) {
                if !prefix.is_empty() {
                    prefixed_vars.push((name, prefix));
                }
            }
        }
    }

    if prefixed_vars.is_empty() {
        return Vec::new();
    }

    let query_strings = collect_query_strings(cells);
    let mut warnings = Vec::new();

    for (label, query) in &query_strings {
        for &(name, prefix) in &prefixed_vars {
            let pattern = format!("{prefix}:");
            let mut search_from = 0;
            while let Some(rel_pos) = query[search_from..].find(&pattern) {
                let abs_pos = search_from + rel_pos;
                let value_start = abs_pos + pattern.len();

                // Ensure the prefix is at a word boundary (not inside "myenv:").
                let at_word_boundary = abs_pos == 0 || {
                    let prev = query.as_bytes()[abs_pos - 1];
                    !prev.is_ascii_alphanumeric() && prev != b'_'
                };

                if at_word_boundary && value_start < query.len() {
                    let next_byte = query.as_bytes()[value_start];
                    // Skip $variable refs (caught by redundant-prefix check) and wildcards.
                    if next_byte != b'$'
                        && next_byte != b'*'
                        && (next_byte.is_ascii_alphanumeric() || next_byte == b'"')
                    {
                        let value_end = query[value_start..]
                            .find(|c: char| {
                                !c.is_ascii_alphanumeric() && c != '_' && c != '-' && c != '.'
                            })
                            .map(|e| value_start + e)
                            .unwrap_or(query.len());
                        let hardcoded = &query[value_start..value_end];
                        warnings.push(format!(
                            "{label} contains hardcoded \"{prefix}:{hardcoded}\" \
                             but template variable ${name} is defined with prefix \
                             \"{prefix}\"; use ${name} instead, or pass \
                             --ack-warnings to proceed anyway",
                        ));
                    }
                }

                search_from = value_start.max(search_from + 1);
            }
        }
    }

    warnings
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
        let cells = parse_markdown(input).unwrap().cells;
        assert_eq!(cells, vec![Cell::Markdown("# Hello\nText".to_string())]);
    }

    #[test]
    fn single_log_query() {
        let input = "```log-query\n{\"query\":\"env:prod\"}\n```";
        let cells = parse_markdown(input).unwrap().cells;
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
        let cells = parse_markdown(input).unwrap().cells;
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
        let cells = parse_markdown(input).unwrap().cells;
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
        let cells = parse_markdown(input).unwrap().cells;
        assert_eq!(cells.len(), 2);
        assert!(matches!(&cells[0], Cell::LogQuery(_)));
        assert!(matches!(&cells[1], Cell::LogQuery(_)));
    }

    #[test]
    fn log_query_with_indexes() {
        let input = "```log-query\n{\"query\":\"env:prod\",\"indexes\":[\"main\"]}\n```";
        let cells = parse_markdown(input).unwrap().cells;
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
        let cells = parse_markdown(input).unwrap().cells;
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
        let cells = parse_markdown(input).unwrap().cells;
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
        let cells = parse_markdown("").unwrap().cells;
        assert!(cells.is_empty());
    }

    #[test]
    fn whitespace_only() {
        let cells = parse_markdown("  \n  ").unwrap().cells;
        assert!(cells.is_empty());
    }

    #[test]
    fn trailing_spaces_on_fence() {
        let input = "```log-query   \n{\"query\":\"env:prod\"}\n```";
        let cells = parse_markdown(input).unwrap().cells;
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
        let cells = parse_markdown(input).unwrap().cells;
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
        let cells = parse_markdown(input).unwrap().cells;
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
        let cells = parse_markdown(input).unwrap().cells;
        assert_eq!(
            cells,
            vec![Cell::MetricQuery(MetricQueryCell {
                query: "avg:system.cpu.user{env:production}".to_string(),
                time: None,
                title: None,
                aliases: None,
                display_type: None,
                events: None,
                queries: None,
            })]
        );
    }

    #[test]
    fn metric_query_with_time() {
        let input =
            "```metric-query\n{\"query\":\"avg:system.cpu.user{*}\",\"time\":\"4h\"}\n```";
        let cells = parse_markdown(input).unwrap().cells;
        assert_eq!(
            cells,
            vec![Cell::MetricQuery(MetricQueryCell {
                query: "avg:system.cpu.user{*}".to_string(),
                time: Some(cells::CellTime::Relative("4h".to_string())),
                title: None,
                aliases: None,
                display_type: None,
                events: None,
                queries: None,
            })]
        );
    }

    #[test]
    fn mixed_log_and_metric_queries() {
        let input = "# Title\n\n```log-query\n{\"query\":\"env:prod\"}\n```\n\n```metric-query\n{\"query\":\"avg:system.cpu.user{*}\"}\n```";
        let cells = parse_markdown(input).unwrap().cells;
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

    // -- event-query parsing --

    #[test]
    fn single_event_query() {
        let input = "```event-query\n{\"data_source\":\"events\",\"search\":\"source:deploy\",\"compute\":\"count\"}\n```";
        let cells = parse_markdown(input).unwrap().cells;
        assert_eq!(
            cells,
            vec![Cell::EventQuery(cells::EventQueryCell {
                data_source: "events".to_string(),
                search: "source:deploy".to_string(),
                compute: "count".to_string(),
                metric: None,
                group_by: None,
                title: None,
                display_type: None,
                time: None,
                events: None,
            })]
        );
    }

    #[test]
    fn event_query_with_all_fields() {
        let input = r#"```event-query
{"data_source":"events","search":"source:deploy","compute":"avg","metric":"@duration","group_by":[{"facet":"service","limit":10}],"title":"Deploy Duration","display_type":"bars","time":"4h"}
```"#;
        let cells = parse_markdown(input).unwrap().cells;
        assert_eq!(
            cells,
            vec![Cell::EventQuery(cells::EventQueryCell {
                data_source: "events".to_string(),
                search: "source:deploy".to_string(),
                compute: "avg".to_string(),
                metric: Some("@duration".to_string()),
                group_by: Some(vec![cells::EventQueryGroupBy {
                    facet: "service".to_string(),
                    limit: Some(10),
                }]),
                title: Some("Deploy Duration".to_string()),
                display_type: Some("bars".to_string()),
                time: Some(cells::CellTime::Relative("4h".to_string())),
                events: None,
            })]
        );
    }

    #[test]
    fn mixed_all_query_types() {
        let input = "# Title\n\n```log-query\n{\"query\":\"env:prod\"}\n```\n\n```metric-query\n{\"query\":\"avg:system.cpu.user{*}\"}\n```\n\n```event-query\n{\"data_source\":\"events\",\"search\":\"source:deploy\",\"compute\":\"count\"}\n```";
        let cells = parse_markdown(input).unwrap().cells;
        assert_eq!(cells.len(), 4);
        assert!(matches!(&cells[0], Cell::Markdown(_)));
        assert!(matches!(&cells[1], Cell::LogQuery(_)));
        assert!(matches!(&cells[2], Cell::MetricQuery(_)));
        assert!(matches!(&cells[3], Cell::EventQuery(_)));
    }

    #[test]
    fn unterminated_event_query_block() {
        let input = "```event-query\n{\"data_source\":\"events\",\"search\":\"x\",\"compute\":\"count\"}";
        let result = parse_markdown(input);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("Unterminated event-query code block"),
            "{}",
            err
        );
    }

    #[test]
    fn invalid_json_in_event_query() {
        let input = "```event-query\nnot json\n```";
        let result = parse_markdown(input);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("Invalid JSON in event-query block"),
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

    // -- frontmatter parsing --

    #[test]
    fn frontmatter_with_variables() {
        let input = "---\nvariables:\n  - name: env\n    prefix: env\n    default: production\n---\n\n# My Notebook\nText";
        let result = parse_markdown(input).unwrap();
        assert!(result.template_variables.is_some());
        let vars = result.template_variables.unwrap();
        assert!(vars.is_array());
        let arr = vars.as_array().unwrap();
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0]["name"], "env");
        assert_eq!(arr[0]["prefix"], "env");
        assert_eq!(arr[0]["default"], "production");
        // The cells should only contain the content after the frontmatter.
        assert_eq!(result.cells.len(), 1);
        assert_eq!(result.cells[0], Cell::Markdown("# My Notebook\nText".to_string()));
    }

    #[test]
    fn frontmatter_multiple_variables() {
        let input = "---\nvariables:\n  - name: env\n    prefix: env\n    default: production\n  - name: service\n    prefix: service\n    default: \"*\"\n---\n\nContent";
        let result = parse_markdown(input).unwrap();
        let vars = result.template_variables.unwrap();
        assert_eq!(vars.as_array().unwrap().len(), 2);
    }

    #[test]
    fn no_frontmatter() {
        let input = "# Just markdown\nNo frontmatter here";
        let result = parse_markdown(input).unwrap();
        assert!(result.template_variables.is_none());
        assert_eq!(result.cells.len(), 1);
    }

    #[test]
    fn frontmatter_without_variables() {
        let input = "---\ntitle: Something\n---\n\n# Content";
        let result = parse_markdown(input).unwrap();
        assert!(result.template_variables.is_none());
        assert_eq!(result.cells.len(), 1);
    }

    #[test]
    fn frontmatter_with_leading_whitespace() {
        let input = "  \n---\nvariables:\n  - name: env\n    prefix: env\n    default: prod\n---\n\nContent";
        let result = parse_markdown(input).unwrap();
        assert!(result.template_variables.is_some());
    }

    // -- validate_template_variables --

    #[test]
    fn template_var_redundant_prefix_detected() {
        let vars = serde_json::json!([
            {"name": "env", "prefix": "env", "default": "production"}
        ]);
        let cells = vec![Cell::LogQuery(LogQueryCell {
            query: "service:web env:$env".to_string(),
            indexes: None,
            columns: None,
            time: None,
        })];
        let warnings = validate_template_variables(&cells, Some(&vars));
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("env:$env"), "{}", warnings[0]);
        assert!(warnings[0].contains("Use just \"$env\""), "{}", warnings[0]);
    }

    #[test]
    fn template_var_correct_usage_no_warning() {
        let vars = serde_json::json!([
            {"name": "env", "prefix": "env", "default": "production"}
        ]);
        let cells = vec![Cell::LogQuery(LogQueryCell {
            query: "service:web $env".to_string(),
            indexes: None,
            columns: None,
            time: None,
        })];
        let warnings = validate_template_variables(&cells, Some(&vars));
        assert!(warnings.is_empty());
    }

    #[test]
    fn template_var_undefined_variable_detected() {
        let vars = serde_json::json!([
            {"name": "env", "prefix": "env", "default": "production"}
        ]);
        let cells = vec![Cell::LogQuery(LogQueryCell {
            query: "service:web $env $region".to_string(),
            indexes: None,
            columns: None,
            time: None,
        })];
        let warnings = validate_template_variables(&cells, Some(&vars));
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("$region"), "{}", warnings[0]);
        assert!(warnings[0].contains("undefined"), "{}", warnings[0]);
    }

    #[test]
    fn template_var_no_frontmatter_skips_undefined_check() {
        // Without frontmatter, $var references shouldn't warn since variables
        // may be defined on the notebook server-side.
        let cells = vec![Cell::LogQuery(LogQueryCell {
            query: "service:web $env".to_string(),
            indexes: None,
            columns: None,
            time: None,
        })];
        let warnings = validate_template_variables(&cells, None);
        assert!(warnings.is_empty());
    }

    #[test]
    fn template_var_metric_query_events_checked() {
        let vars = serde_json::json!([
            {"name": "env", "prefix": "env", "default": "production"}
        ]);
        let cells = vec![Cell::MetricQuery(cells::MetricQueryCell {
            query: "avg:system.cpu.user{$env}".to_string(),
            queries: None,
            time: None,
            title: None,
            aliases: None,
            display_type: None,
            events: Some(vec![cells::EventOverlay {
                q: "env:$env source:deploy".to_string(),
            }]),
        })];
        let warnings = validate_template_variables(&cells, Some(&vars));
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("event overlay"), "{}", warnings[0]);
        assert!(warnings[0].contains("env:$env"), "{}", warnings[0]);
    }

    #[test]
    fn template_var_no_variables_defined_but_frontmatter_exists() {
        // Frontmatter exists but has no variables, so $env is undefined.
        let vars = serde_json::json!([]);
        let cells = vec![Cell::LogQuery(LogQueryCell {
            query: "service:web $env".to_string(),
            indexes: None,
            columns: None,
            time: None,
        })];
        let warnings = validate_template_variables(&cells, Some(&vars));
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("$env"), "{}", warnings[0]);
    }

    // -- warn_hardcoded_variable_values --

    #[test]
    fn hardcoded_value_detected() {
        let vars = serde_json::json!([
            {"name": "env", "prefix": "env", "default": "staging"}
        ]);
        let cells = vec![Cell::LogQuery(LogQueryCell {
            query: "service:web env:staging".to_string(),
            indexes: None,
            columns: None,
            time: None,
        })];
        let warnings = warn_hardcoded_variable_values(&cells, Some(&vars));
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("env:staging"), "{}", warnings[0]);
        assert!(warnings[0].contains("$env"), "{}", warnings[0]);
    }

    #[test]
    fn hardcoded_value_in_metric_query_braces() {
        let vars = serde_json::json!([
            {"name": "env", "prefix": "env", "default": "production"}
        ]);
        let cells = vec![Cell::MetricQuery(cells::MetricQueryCell {
            query: "avg:system.cpu.user{env:staging}".to_string(),
            queries: None,
            time: None,
            title: None,
            aliases: None,
            display_type: None,
            events: None,
        })];
        let warnings = warn_hardcoded_variable_values(&cells, Some(&vars));
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("env:staging"), "{}", warnings[0]);
    }

    #[test]
    fn wildcard_not_flagged() {
        let vars = serde_json::json!([
            {"name": "env", "prefix": "env", "default": "production"}
        ]);
        let cells = vec![Cell::LogQuery(LogQueryCell {
            query: "service:web env:*".to_string(),
            indexes: None,
            columns: None,
            time: None,
        })];
        let warnings = warn_hardcoded_variable_values(&cells, Some(&vars));
        assert!(warnings.is_empty());
    }

    #[test]
    fn variable_ref_not_flagged() {
        let vars = serde_json::json!([
            {"name": "env", "prefix": "env", "default": "production"}
        ]);
        let cells = vec![Cell::LogQuery(LogQueryCell {
            query: "service:web $env".to_string(),
            indexes: None,
            columns: None,
            time: None,
        })];
        let warnings = warn_hardcoded_variable_values(&cells, Some(&vars));
        assert!(warnings.is_empty());
    }

    #[test]
    fn partial_prefix_not_flagged() {
        // "myenv:staging" should NOT trigger for a variable with prefix "env".
        let vars = serde_json::json!([
            {"name": "env", "prefix": "env", "default": "production"}
        ]);
        let cells = vec![Cell::LogQuery(LogQueryCell {
            query: "myenv:staging".to_string(),
            indexes: None,
            columns: None,
            time: None,
        })];
        let warnings = warn_hardcoded_variable_values(&cells, Some(&vars));
        assert!(warnings.is_empty());
    }

    #[test]
    fn no_prefix_variable_not_flagged() {
        // Variables without a prefix should not trigger hardcoded warnings.
        let vars = serde_json::json!([
            {"name": "env", "default": "production"}
        ]);
        let cells = vec![Cell::LogQuery(LogQueryCell {
            query: "env:staging".to_string(),
            indexes: None,
            columns: None,
            time: None,
        })];
        let warnings = warn_hardcoded_variable_values(&cells, Some(&vars));
        assert!(warnings.is_empty());
    }

    #[test]
    fn hardcoded_in_event_overlay() {
        let vars = serde_json::json!([
            {"name": "env", "prefix": "env", "default": "production"}
        ]);
        let cells = vec![Cell::MetricQuery(cells::MetricQueryCell {
            query: "avg:system.cpu.user{$env}".to_string(),
            queries: None,
            time: None,
            title: None,
            aliases: None,
            display_type: None,
            events: Some(vec![cells::EventOverlay {
                q: "env:staging source:deploy".to_string(),
            }]),
        })];
        let warnings = warn_hardcoded_variable_values(&cells, Some(&vars));
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("event overlay"), "{}", warnings[0]);
        assert!(warnings[0].contains("env:staging"), "{}", warnings[0]);
    }

    #[test]
    fn no_template_variables_no_warnings() {
        let cells = vec![Cell::LogQuery(LogQueryCell {
            query: "env:staging".to_string(),
            indexes: None,
            columns: None,
            time: None,
        })];
        let warnings = warn_hardcoded_variable_values(&cells, None);
        assert!(warnings.is_empty());
    }
}
