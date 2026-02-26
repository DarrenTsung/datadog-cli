use anyhow::{anyhow, Context};
use datadog_api_client::datadog::{self, APIKey, Configuration};
use datadog_api_client::datadogV1::api::api_notebooks::{
    ListNotebooksOptionalParams, NotebooksAPI,
};
use datadog_api_client::datadogV1::model::{
    NotebookCellCreateRequest, NotebookCellResourceType, NotebookCellUpdateRequest,
    NotebookCreateData, NotebookCreateDataAttributes, NotebookCreateRequest, NotebookGlobalTime,
    NotebookRelativeTime, NotebookResourceType, NotebookResponse, NotebookUpdateCell,
    NotebookUpdateData, NotebookUpdateDataAttributes, NotebookUpdateRequest, NotebooksResponse,
    WidgetLiveSpan,
};

use super::cells;
use super::cells::Cell;

fn make_configuration(api_key: &str, app_key: &str) -> Configuration {
    let mut config = Configuration::new();
    config.set_auth_key(
        "apiKeyAuth",
        APIKey {
            key: api_key.to_string(),
            prefix: String::new(),
        },
    );
    config.set_auth_key(
        "appKeyAuth",
        APIKey {
            key: app_key.to_string(),
            prefix: String::new(),
        },
    );
    config
}

pub fn parse_live_span(time: &str) -> anyhow::Result<WidgetLiveSpan> {
    match time {
        "1m" => Ok(WidgetLiveSpan::PAST_ONE_MINUTE),
        "5m" => Ok(WidgetLiveSpan::PAST_FIVE_MINUTES),
        "10m" => Ok(WidgetLiveSpan::PAST_TEN_MINUTES),
        "15m" => Ok(WidgetLiveSpan::PAST_FIFTEEN_MINUTES),
        "30m" => Ok(WidgetLiveSpan::PAST_THIRTY_MINUTES),
        "1h" => Ok(WidgetLiveSpan::PAST_ONE_HOUR),
        "4h" => Ok(WidgetLiveSpan::PAST_FOUR_HOURS),
        "1d" => Ok(WidgetLiveSpan::PAST_ONE_DAY),
        "2d" => Ok(WidgetLiveSpan::PAST_TWO_DAYS),
        "1w" => Ok(WidgetLiveSpan::PAST_ONE_WEEK),
        _ => Err(anyhow!(
            "Unsupported time span: {time}. Supported: 1m, 5m, 10m, 15m, 30m, 1h, 4h, 1d, 2d, 1w"
        )),
    }
}

fn make_global_time(live_span: WidgetLiveSpan) -> NotebookGlobalTime {
    NotebookGlobalTime::NotebookRelativeTime(Box::new(NotebookRelativeTime::new(live_span)))
}

pub async fn create_notebook(
    api_key: &str,
    app_key: &str,
    title: &str,
    parsed_cells: &[Cell],
    live_span: WidgetLiveSpan,
) -> anyhow::Result<NotebookResponse> {
    let config = make_configuration(api_key, app_key);
    let api = NotebooksAPI::with_config(config);

    let cell_requests: Vec<NotebookCellCreateRequest> =
        cells::cells_to_create_requests(parsed_cells);

    let body = NotebookCreateRequest::new(NotebookCreateData::new(
        NotebookCreateDataAttributes::new(cell_requests, title.to_string(), make_global_time(live_span)),
        NotebookResourceType::NOTEBOOKS,
    ));

    api.create_notebook(body).await.map_err(|e| match &e {
        datadog::Error::ResponseError(resp) => {
            anyhow!("Failed to create notebook ({}): {}", resp.status, resp.content)
        }
        _ => anyhow!(e).context("Failed to create notebook"),
    })
}

pub async fn update_notebook(
    api_key: &str,
    app_key: &str,
    notebook_id: i64,
    title: &str,
    parsed_cells: &[Cell],
    live_span: WidgetLiveSpan,
) -> anyhow::Result<NotebookResponse> {
    let config = make_configuration(api_key, app_key);
    let api = NotebooksAPI::with_config(config);

    // Fetch existing notebook to get cell IDs — the update API treats cells
    // without IDs as new additions rather than replacements.
    let existing = api
        .get_notebook(notebook_id)
        .await
        .context("Failed to fetch existing notebook for update")?;
    let existing_ids: Vec<String> = existing
        .data
        .map(|d| d.attributes.cells.iter().map(|c| c.id.clone()).collect())
        .unwrap_or_default();

    let new_cells: Vec<NotebookCellCreateRequest> =
        cells::cells_to_create_requests(parsed_cells);

    let cell_requests: Vec<NotebookUpdateCell> = new_cells
        .into_iter()
        .enumerate()
        .map(|(i, create_req)| {
            if let Some(cell_id) = existing_ids.get(i) {
                // Reuse the existing cell ID so the API replaces it.
                let update_attrs = cells::create_attrs_to_update_attrs(create_req.attributes);
                NotebookUpdateCell::NotebookCellUpdateRequest(Box::new(
                    NotebookCellUpdateRequest::new(
                        update_attrs,
                        cell_id.clone(),
                        NotebookCellResourceType::NOTEBOOK_CELLS,
                    ),
                ))
            } else {
                // More cells than before — create new ones.
                NotebookUpdateCell::NotebookCellCreateRequest(Box::new(create_req))
            }
        })
        .collect();

    let body = NotebookUpdateRequest::new(NotebookUpdateData::new(
        NotebookUpdateDataAttributes::new(
            cell_requests,
            title.to_string(),
            make_global_time(live_span),
        ),
        NotebookResourceType::NOTEBOOKS,
    ));

    api.update_notebook(notebook_id, body).await.map_err(|e| match &e {
        datadog::Error::ResponseError(resp) => {
            anyhow!("Failed to update notebook ({}): {}", resp.status, resp.content)
        }
        _ => anyhow!(e).context("Failed to update notebook"),
    })
}

pub async fn list_notebooks(
    api_key: &str,
    app_key: &str,
) -> anyhow::Result<NotebooksResponse> {
    let config = make_configuration(api_key, app_key);
    let api = NotebooksAPI::with_config(config);

    api.list_notebooks(ListNotebooksOptionalParams::default())
        .await
        .context("Failed to list notebooks")
}

pub async fn delete_notebook(
    api_key: &str,
    app_key: &str,
    notebook_id: i64,
) -> anyhow::Result<()> {
    let config = make_configuration(api_key, app_key);
    let api = NotebooksAPI::with_config(config);

    api.delete_notebook(notebook_id)
        .await
        .context("Failed to delete notebook")
}

pub async fn get_notebook(
    api_key: &str,
    app_key: &str,
    notebook_id: i64,
) -> anyhow::Result<NotebookResponse> {
    let config = make_configuration(api_key, app_key);
    let api = NotebooksAPI::with_config(config);

    api.get_notebook(notebook_id)
        .await
        .context("Failed to get notebook")
}
