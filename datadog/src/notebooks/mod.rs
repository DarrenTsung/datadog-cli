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
                println!("Created notebook {} ({})", data.id, data.attributes.name);
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
                println!("Updated notebook {} ({})", data.id, data.attributes.name);
            }
        }
        NotebooksCommand::Delete { id } => {
            api::delete_notebook(api_key, app_key, id).await?;
            println!("Deleted notebook {id}");
        }
    }

    Ok(())
}
