pub mod api;
pub mod cells;
pub mod parser;

use anyhow::{anyhow, Context};
use chrono::{Datelike, NaiveDate, Utc, Weekday};
use std::time::Instant;
use structopt::StructOpt;

#[derive(StructOpt, Debug)]
pub struct NotebooksOpt {
    /// Print timing information for each step.
    #[structopt(long)]
    verbose: bool,

    #[structopt(subcommand)]
    cmd: NotebooksCommand,
}

#[derive(StructOpt, Debug)]
pub enum NotebooksCommand {
    /// List all notebooks.
    List {
        /// Max notebooks to return.
        #[structopt(long)]
        limit: Option<usize>,

        /// Bypass the --limit <= 100 guard.
        #[structopt(long)]
        force: bool,
    },

    /// Create a notebook from a markdown file.
    Create {
        /// Path to the markdown file.
        #[structopt(long)]
        file: String,

        /// Title of the notebook.
        #[structopt(long)]
        title: String,

        /// Time span for log-stream cells (e.g. 1h, 4h, 1d, 2d, 1w).
        #[structopt(long, default_value = "1h")]
        time: String,

        /// Proceed despite warnings (e.g. hardcoded values for template variables).
        #[structopt(long)]
        ack_warnings: bool,
    },

    /// Update an existing notebook from a markdown file.
    Update {
        /// Notebook ID to update.
        #[structopt(long)]
        id: i64,

        /// Path to the markdown file.
        #[structopt(long)]
        file: String,

        /// New title. If omitted, the existing title is preserved.
        #[structopt(long)]
        title: Option<String>,

        /// Time span for log-stream cells (e.g. 1h, 4h, 1d, 2d, 1w).
        #[structopt(long, default_value = "1h")]
        time: String,

        /// Proceed despite warnings (e.g. hardcoded values for template variables).
        #[structopt(long)]
        ack_warnings: bool,
    },

    /// Delete a notebook.
    Delete {
        /// Notebook ID to delete.
        #[structopt(long)]
        id: i64,
    },

    /// Read a notebook and print it as markdown.
    Read {
        /// Notebook ID or URL (e.g. 12345 or https://app.datadoghq.com/notebook/12345/title).
        #[structopt(long)]
        id: String,
    },

}

// ---------------------------------------------------------------------------
// Day-of-week correction
// ---------------------------------------------------------------------------

const DAY_NAMES: &[(&str, &str, Weekday)] = &[
    ("Mon", "Monday", Weekday::Mon),
    ("Tue", "Tuesday", Weekday::Tue),
    ("Wed", "Wednesday", Weekday::Wed),
    ("Thu", "Thursday", Weekday::Thu),
    ("Fri", "Friday", Weekday::Fri),
    ("Sat", "Saturday", Weekday::Sat),
    ("Sun", "Sunday", Weekday::Sun),
];

const MONTH_NAMES: &[(&str, &str, u32)] = &[
    ("Jan", "January", 1),
    ("Feb", "February", 2),
    ("Mar", "March", 3),
    ("Apr", "April", 4),
    ("May", "May", 5),
    ("Jun", "June", 6),
    ("Jul", "July", 7),
    ("Aug", "August", 8),
    ("Sep", "September", 9),
    ("Oct", "October", 10),
    ("Nov", "November", 11),
    ("Dec", "December", 12),
];

fn parse_weekday(s: &str) -> Option<(Weekday, bool)> {
    for &(abbr, full, wd) in DAY_NAMES {
        if s.eq_ignore_ascii_case(abbr) {
            return Some((wd, false));
        }
        if s.eq_ignore_ascii_case(full) {
            return Some((wd, true));
        }
    }
    None
}

fn parse_month(s: &str) -> Option<u32> {
    for &(abbr, full, num) in MONTH_NAMES {
        if s.eq_ignore_ascii_case(abbr) || s.eq_ignore_ascii_case(full) {
            return Some(num);
        }
    }
    None
}

fn weekday_abbr(wd: Weekday) -> &'static str {
    match wd {
        Weekday::Mon => "Mon",
        Weekday::Tue => "Tue",
        Weekday::Wed => "Wed",
        Weekday::Thu => "Thu",
        Weekday::Fri => "Fri",
        Weekday::Sat => "Sat",
        Weekday::Sun => "Sun",
    }
}

fn weekday_full(wd: Weekday) -> &'static str {
    match wd {
        Weekday::Mon => "Monday",
        Weekday::Tue => "Tuesday",
        Weekday::Wed => "Wednesday",
        Weekday::Thu => "Thursday",
        Weekday::Fri => "Friday",
        Weekday::Sat => "Saturday",
        Weekday::Sun => "Sunday",
    }
}

/// Find the most common 4-digit year in the text, falling back to the current year.
fn infer_year(text: &str) -> i32 {
    let mut counts = std::collections::HashMap::<i32, usize>::new();
    let mut i = 0;
    let bytes = text.as_bytes();
    while i + 3 < bytes.len() {
        if bytes[i] == b'2' && bytes[i + 1] == b'0' && bytes[i + 2].is_ascii_digit() && bytes[i + 3].is_ascii_digit() {
            // Make sure it's not part of a longer number.
            let before_ok = i == 0 || !bytes[i - 1].is_ascii_digit();
            let after_ok = i + 4 >= bytes.len() || !bytes[i + 4].is_ascii_digit();
            if before_ok && after_ok {
                if let Ok(year) = text[i..i + 4].parse::<i32>() {
                    if (2020..=2030).contains(&year) {
                        *counts.entry(year).or_default() += 1;
                    }
                }
            }
        }
        i += 1;
    }
    counts
        .into_iter()
        .max_by_key(|&(_, count)| count)
        .map(|(year, _)| year)
        .unwrap_or_else(|| Utc::now().year())
}

/// Scan text for date patterns like "Wed Feb 19, 2026" or "Wednesday February 19"
/// and fix incorrect day-of-week names in place. Returns (fixed_text, correction_count).
fn fix_day_of_week_dates(content: &str) -> (String, usize) {
    let mut result = String::with_capacity(content.len());
    let mut corrections = 0;
    let default_year = infer_year(content);

    // We'll scan character by character looking for day-of-week names at word
    // boundaries, then try to parse what follows.
    let chars: Vec<char> = content.chars().collect();
    let len = chars.len();
    let mut i = 0;

    while i < len {
        // Check if we're at a word boundary (start of string or after non-alpha).
        let at_word_start = i == 0 || !chars[i - 1].is_alphabetic();
        if !at_word_start || !chars[i].is_alphabetic() {
            result.push(chars[i]);
            i += 1;
            continue;
        }

        // Try to read a word (potential day name).
        let word_start = i;
        while i < len && chars[i].is_alphabetic() {
            i += 1;
        }
        let word: String = chars[word_start..i].iter().collect();

        let (claimed_weekday, is_full) = match parse_weekday(&word) {
            Some(v) => v,
            None => {
                result.push_str(&word);
                continue;
            }
        };

        // Save position after day name — try to parse the rest.
        let after_day = i;

        // Skip optional comma and whitespace: "Wed, Feb 19" or "Wed Feb 19"
        let mut j = i;
        if j < len && chars[j] == ',' {
            j += 1;
        }
        while j < len && chars[j] == ' ' {
            j += 1;
        }

        // Read month name.
        let month_start = j;
        while j < len && chars[j].is_alphabetic() {
            j += 1;
        }
        let month_word: String = chars[month_start..j].iter().collect();
        let month = match parse_month(&month_word) {
            Some(m) => m,
            None => {
                result.push_str(&word);
                i = after_day;
                continue;
            }
        };

        // Skip whitespace.
        while j < len && chars[j] == ' ' {
            j += 1;
        }

        // Read day number.
        let day_start = j;
        while j < len && chars[j].is_ascii_digit() {
            j += 1;
        }
        let day_str: String = chars[day_start..j].iter().collect();
        let day: u32 = match day_str.parse() {
            Ok(d) if (1..=31).contains(&d) => d,
            _ => {
                result.push_str(&word);
                i = after_day;
                continue;
            }
        };

        // Optional: skip comma/whitespace, then try to read year.
        let mut k = j;
        // Skip optional ordinal suffix (st, nd, rd, th).
        if k + 1 < len {
            let suffix: String = chars[k..k + 2].iter().collect();
            if ["st", "nd", "rd", "th"].contains(&suffix.to_lowercase().as_str()) {
                k += 2;
            }
        }
        if k < len && chars[k] == ',' {
            k += 1;
        }
        while k < len && chars[k] == ' ' {
            k += 1;
        }
        let year_start = k;
        while k < len && chars[k].is_ascii_digit() {
            k += 1;
        }
        let year_str: String = chars[year_start..k].iter().collect();
        let (year, end_pos) = if year_str.len() == 4 {
            match year_str.parse::<i32>() {
                Ok(y) => (y, k),
                Err(_) => (default_year, j),
            }
        } else {
            (default_year, j)
        };

        // Now validate.
        let date = match NaiveDate::from_ymd_opt(year, month, day) {
            Some(d) => d,
            None => {
                result.push_str(&word);
                i = after_day;
                continue;
            }
        };

        let actual_weekday = date.weekday();
        if claimed_weekday == actual_weekday {
            // Correct — emit as-is.
            let original: String = chars[word_start..end_pos].iter().collect();
            result.push_str(&original);
        } else {
            // Wrong day name — fix it.
            let correct_name = if is_full {
                weekday_full(actual_weekday)
            } else {
                weekday_abbr(actual_weekday)
            };
            result.push_str(correct_name);
            // Emit everything between the day name and end_pos.
            let rest: String = chars[after_day..end_pos].iter().collect();
            result.push_str(&rest);
            corrections += 1;
        }

        i = end_pos;
    }

    (result, corrections)
}

/// Fix day-of-week dates in a file, writing corrections back in place.
/// Returns the (possibly modified) content.
fn fix_dates_in_file(path: &str, content: &str) -> anyhow::Result<String> {
    let (fixed, count) = fix_day_of_week_dates(content);
    if count > 0 {
        std::fs::write(path, &fixed)
            .with_context(|| format!("Failed to write corrected dates to {path}"))?;
        eprintln!("{count} non-matching day of week{} corrected in {path}",
            if count == 1 { "" } else { "s" });
    }
    Ok(fixed)
}

pub async fn run_notebooks(
    api_key: &str,
    app_key: &str,
    opt: NotebooksOpt,
) -> anyhow::Result<()> {
    let verbose = opt.verbose;
    match opt.cmd {
        NotebooksCommand::List { limit, force } => {
            if !force && !limit.is_some_and(|l| l <= 100) {
                return Err(anyhow!(
                    "Error: --limit is required and must be <= 100 (or use --force to bypass)."
                ));
            }
            let response = api::list_notebooks(api_key, app_key).await?;
            if let Some(data) = response.data {
                let cap = limit.unwrap_or(data.len());
                for nb in data.iter().take(cap) {
                    println!("{}\t{}", nb.id, nb.attributes.name);
                }
                if data.is_empty() {
                    eprintln!("No notebooks found.");
                }
            } else {
                eprintln!("No notebooks found.");
            }
        }
        NotebooksCommand::Create { file, title, time, ack_warnings } => {
            let live_span = api::parse_live_span(&time)?;
            let content = std::fs::read_to_string(&file)
                .with_context(|| format!("Failed to read file: {file}"))?;
            let content = fix_dates_in_file(&file, &content)?;
            let parser::ParseResult { mut cells, template_variables } = parser::parse_markdown(&content)?;
            if cells.is_empty() {
                return Err(anyhow!("No cells parsed from {file}"));
            }
            strip_title_from_cells(&mut cells);
            let broken = parser::validate_section_links(&cells);
            for slug in &broken {
                eprintln!("Warning: section link #{slug} does not match any heading");
            }
            let var_warnings = parser::validate_template_variables(&cells, template_variables.as_ref());
            if !var_warnings.is_empty() {
                for w in &var_warnings {
                    eprintln!("Error: {w}");
                }
                return Err(anyhow!("Template variable validation failed"));
            }
            let hardcoded_warnings = parser::warn_hardcoded_variable_values(&cells, template_variables.as_ref());
            if !hardcoded_warnings.is_empty() && !ack_warnings {
                for w in &hardcoded_warnings {
                    eprintln!("Warning: {w}");
                }
                return Err(anyhow!(
                    "Hardcoded values found for template variables (pass --ack-warnings to proceed)"
                ));
            }
            let response =
                api::create_notebook(api_key, app_key, &title, &cells, live_span, template_variables.as_ref()).await?;
            if let Some(data) = response.data {
                println!("Created notebook: https://app.datadoghq.com/notebook/{}", data.id);
            }
        }
        NotebooksCommand::Update {
            id,
            file,
            title,
            time,
            ack_warnings,
        } => {
            let t0 = Instant::now();

            let live_span = api::parse_live_span(&time)?;
            let content = std::fs::read_to_string(&file)
                .with_context(|| format!("Failed to read file: {file}"))?;
            let content = fix_dates_in_file(&file, &content)?;
            let parser::ParseResult { mut cells, template_variables } = parser::parse_markdown(&content)?;
            if cells.is_empty() {
                return Err(anyhow!("No cells parsed from {file}"));
            }
            if verbose {
                eprintln!("[{:.2}s] parsed {} cells", t0.elapsed().as_secs_f64(), cells.len());
            }

            let broken = parser::validate_section_links(&cells);
            for slug in &broken {
                eprintln!("Warning: section link #{slug} does not match any heading");
            }
            let var_warnings = parser::validate_template_variables(&cells, template_variables.as_ref());
            if !var_warnings.is_empty() {
                for w in &var_warnings {
                    eprintln!("Error: {w}");
                }
                return Err(anyhow!("Template variable validation failed"));
            }
            let hardcoded_warnings = parser::warn_hardcoded_variable_values(&cells, template_variables.as_ref());
            if !hardcoded_warnings.is_empty() && !ack_warnings {
                for w in &hardcoded_warnings {
                    eprintln!("Warning: {w}");
                }
                return Err(anyhow!(
                    "Hardcoded values found for template variables (pass --ack-warnings to proceed)"
                ));
            }

            let title = match title {
                Some(t) => t,
                None => extract_title_from_cells(&cells).unwrap_or_else(|| {
                    eprintln!("Warning: no H1 title found in file and --title not provided; fetching from Datadog");
                    String::new()
                }),
            };
            // If we couldn't find a local title, fetch from the existing notebook.
            let title = if title.is_empty() {
                let t1 = Instant::now();
                let existing = api::get_notebook(api_key, app_key, id).await?;
                if verbose {
                    eprintln!("[{:.2}s] fetched existing title", t1.elapsed().as_secs_f64());
                }
                existing
                    .data
                    .map(|d| d.attributes.name)
                    .unwrap_or_else(|| "Untitled".to_string())
            } else {
                strip_title_from_cells(&mut cells);
                title
            };

            // Fetch existing cell IDs for the update (reuse IDs to avoid
            // cell duplication).
            let t2 = Instant::now();
            let existing_ids = api::get_cell_ids(api_key, app_key, id).await?;
            if verbose {
                eprintln!("[{:.2}s] fetched {} existing cell IDs", t2.elapsed().as_secs_f64(), existing_ids.len());
            }

            let t3 = Instant::now();
            let response =
                api::update_notebook(api_key, app_key, id, &title, &cells, live_span, &existing_ids, template_variables.as_ref()).await?;
            if verbose {
                eprintln!("[{:.2}s] update API call", t3.elapsed().as_secs_f64());
            }

            if let Some(data) = response.data {
                println!("Updated notebook: https://app.datadoghq.com/notebook/{}", data.id);
            }
            if verbose {
                eprintln!("[{:.2}s] total", t0.elapsed().as_secs_f64());
            }
        }
        NotebooksCommand::Delete { id } => {
            api::delete_notebook(api_key, app_key, id).await?;
            println!("Deleted notebook {id}");
        }
        NotebooksCommand::Read { id } => {
            let notebook_id = parse_notebook_id(&id)?;
            let response = api::get_notebook(api_key, app_key, notebook_id).await?;
            let data = response
                .data
                .ok_or_else(|| anyhow!("No data in notebook response"))?;

            // Emit template variables as frontmatter if present.
            if let Some(vars) = data.attributes.additional_properties.get("template_variables") {
                print!("{}", cells::template_variables_to_frontmatter(vars));
            }

            let markdown: Vec<String> = data
                .attributes
                .cells
                .iter()
                .map(|cell| cells::notebook_cell_to_markdown(&cell.attributes))
                .collect();
            println!("{}", markdown.join("\n\n"));
        }
    }

    Ok(())
}

/// Extract a title from the first H1 heading in the parsed cells.
fn extract_title_from_cells(cells: &[cells::Cell]) -> Option<String> {
    for cell in cells {
        if let cells::Cell::Markdown(text) = cell {
            for line in text.lines() {
                let trimmed = line.trim();
                if let Some(title) = trimmed.strip_prefix("# ") {
                    let title = title.trim();
                    if !title.is_empty() {
                        return Some(title.to_string());
                    }
                }
            }
        }
    }
    None
}

/// Remove the first H1 heading from cells (since it becomes the notebook title).
/// If removing the H1 leaves an empty markdown cell, drop it entirely.
fn strip_title_from_cells(cells: &mut Vec<cells::Cell>) {
    for i in 0..cells.len() {
        if let cells::Cell::Markdown(text) = &cells[i] {
            let mut found = false;
            let new_text: Vec<&str> = text
                .lines()
                .filter(|line| {
                    if !found && line.trim().starts_with("# ") {
                        found = true;
                        false
                    } else {
                        true
                    }
                })
                .collect();
            if found {
                let joined = new_text.join("\n").trim().to_string();
                if joined.is_empty() {
                    cells.remove(i);
                } else {
                    cells[i] = cells::Cell::Markdown(joined);
                }
                return;
            }
        }
    }
}

/// Parse a notebook ID from either a plain number or a Datadog notebook URL.
fn parse_notebook_id(id: &str) -> anyhow::Result<i64> {
    // Try plain numeric ID first.
    if let Ok(n) = id.parse::<i64>() {
        return Ok(n);
    }

    // Try extracting from a URL like https://app.datadoghq.com/notebook/12345/some-title
    if let Some(rest) = id
        .strip_prefix("https://app.datadoghq.com/notebook/")
        .or_else(|| id.strip_prefix("http://app.datadoghq.com/notebook/"))
    {
        let segment = rest.split('/').next().unwrap_or("");
        if let Ok(n) = segment.parse::<i64>() {
            return Ok(n);
        }
    }

    Err(anyhow!(
        "Invalid notebook ID: {id}. Provide a numeric ID or a Datadog notebook URL."
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_numeric_id() {
        assert_eq!(parse_notebook_id("12345").unwrap(), 12345);
    }

    #[test]
    fn parse_url_with_title() {
        assert_eq!(
            parse_notebook_id("https://app.datadoghq.com/notebook/67890/my-notebook").unwrap(),
            67890
        );
    }

    #[test]
    fn parse_url_without_title() {
        assert_eq!(
            parse_notebook_id("https://app.datadoghq.com/notebook/67890").unwrap(),
            67890
        );
    }

    #[test]
    fn parse_invalid_id() {
        assert!(parse_notebook_id("not-a-number").is_err());
    }

    // -- fix_day_of_week_dates --

    #[test]
    fn correct_date_unchanged() {
        // Feb 19, 2026 is a Thursday.
        let input = "Thu Feb 19, 2026";
        let (output, count) = fix_day_of_week_dates(input);
        assert_eq!(output, input);
        assert_eq!(count, 0);
    }

    #[test]
    fn wrong_day_corrected() {
        // Feb 19, 2026 is a Thursday, not Wednesday.
        let input = "Wed Feb 19, 2026";
        let (output, count) = fix_day_of_week_dates(input);
        assert_eq!(output, "Thu Feb 19, 2026");
        assert_eq!(count, 1);
    }

    #[test]
    fn full_day_name_corrected() {
        // Feb 19, 2026 is a Thursday.
        let input = "Wednesday February 19, 2026";
        let (output, count) = fix_day_of_week_dates(input);
        assert_eq!(output, "Thursday February 19, 2026");
        assert_eq!(count, 1);
    }

    #[test]
    fn comma_after_day_name() {
        // Feb 19, 2026 is a Thursday.
        let input = "Wed, Feb 19, 2026";
        let (output, count) = fix_day_of_week_dates(input);
        assert_eq!(output, "Thu, Feb 19, 2026");
        assert_eq!(count, 1);
    }

    #[test]
    fn no_year_infers_from_context() {
        // 2026 appears elsewhere in the doc, Feb 19 2026 is Thursday.
        let input = "Some event on 2026-01-01. Meeting on Wed Feb 19.";
        let (output, count) = fix_day_of_week_dates(input);
        assert!(output.contains("Thu Feb 19"));
        assert_eq!(count, 1);
    }

    #[test]
    fn multiple_dates_fixed() {
        // Feb 19 2026 = Thu, Feb 20 2026 = Fri.
        let input = "Wed Feb 19, 2026 and Mon Feb 20, 2026";
        let (output, count) = fix_day_of_week_dates(input);
        assert!(output.contains("Thu Feb 19, 2026"));
        assert!(output.contains("Fri Feb 20, 2026"));
        assert_eq!(count, 2);
    }

    #[test]
    fn non_date_text_preserved() {
        let input = "Monday is the best day. The Wednesday meeting was cancelled.";
        let (output, count) = fix_day_of_week_dates(input);
        assert_eq!(output, input);
        assert_eq!(count, 0);
    }

    #[test]
    fn infer_year_picks_most_common() {
        let text = "Events in 2026. More 2026 stuff. One mention of 2025.";
        assert_eq!(infer_year(text), 2026);
    }

    #[test]
    fn infer_year_defaults_to_current() {
        let text = "No years here at all.";
        let year = infer_year(text);
        assert_eq!(year, Utc::now().year());
    }

}
