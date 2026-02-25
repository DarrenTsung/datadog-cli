pub mod api;
pub mod cells;
pub mod parser;

use anyhow::{anyhow, Context};
use structopt::StructOpt;

#[derive(StructOpt, Debug)]
pub struct NotebooksOpt {
    #[structopt(subcommand)]
    cmd: NotebooksCommand,
}

#[derive(StructOpt, Debug)]
pub enum NotebooksCommand {
    /// List all notebooks.
    List,

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

pub async fn run_notebooks(
    api_key: &str,
    app_key: &str,
    opt: NotebooksOpt,
) -> anyhow::Result<()> {
    match opt.cmd {
        NotebooksCommand::List => {
            let response = api::list_notebooks(api_key, app_key).await?;
            if let Some(data) = response.data {
                for nb in &data {
                    println!("{}\t{}", nb.id, nb.attributes.name);
                }
                if data.is_empty() {
                    eprintln!("No notebooks found.");
                }
            } else {
                eprintln!("No notebooks found.");
            }
        }
        NotebooksCommand::Create { file, title, time } => {
            let live_span = api::parse_live_span(&time)?;
            let content = std::fs::read_to_string(&file)
                .with_context(|| format!("Failed to read file: {file}"))?;
            let cells = parser::parse_markdown(&content)?;
            if cells.is_empty() {
                return Err(anyhow!("No cells parsed from {file}"));
            }
            let response =
                api::create_notebook(api_key, app_key, &title, &cells, live_span).await?;
            if let Some(data) = response.data {
                println!("Created notebook: https://app.datadoghq.com/notebook/{}", data.id);
            }
        }
        NotebooksCommand::Update {
            id,
            file,
            title,
            time,
        } => {
            let live_span = api::parse_live_span(&time)?;
            let content = std::fs::read_to_string(&file)
                .with_context(|| format!("Failed to read file: {file}"))?;
            let cells = parser::parse_markdown(&content)?;
            if cells.is_empty() {
                return Err(anyhow!("No cells parsed from {file}"));
            }

            let title = match title {
                Some(t) => t,
                None => {
                    let existing = api::get_notebook(api_key, app_key, id).await?;
                    existing
                        .data
                        .map(|d| d.attributes.name)
                        .unwrap_or_else(|| "Untitled".to_string())
                }
            };

            let response =
                api::update_notebook(api_key, app_key, id, &title, &cells, live_span).await?;
            if let Some(data) = response.data {
                println!("Updated notebook: https://app.datadoghq.com/notebook/{}", data.id);
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
}
