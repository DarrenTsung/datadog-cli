# datadog-cli

A CLI for interacting with the Datadog API. Supports logs, events, metrics, monitors, notebooks, and dashboard unfurling.

## Installation

```bash
cargo install --path datadog
```

Requires `DD_API_KEY` and `DD_APPLICATION_KEY` environment variables (or pass via `--dd-api-key` and `--dd-application-key` flags).

## Commands

### Logs

Search and export logs from Datadog's log API.

```bash
# Search logs
datadog logs --time-range "last 4 hours" --query "service:api status:error"

# From a Datadog URL
datadog logs --datadog-url "https://app.datadoghq.com/logs?..."

# Limit results and select columns
datadog logs --time-range "last 1 hour" --query "env:production" --limit 100 --columns "@host,@status"
```

### Events

Search Datadog events (deploys, alerts, custom events).

```bash
datadog events --time-range "last 1 day" --query "source:deploy"
```

### Metrics

Query Datadog metric timeseries data.

```bash
datadog metrics --query "avg:system.cpu.user{env:production}" --time-range "last 4 hours"
```

### Monitors

Inspect Datadog monitors — metadata, underlying metric chart, and events.

```bash
datadog monitors --id 12345
```

### Unfurl

Resolve and display Datadog dashboard, metric explorer, or shared link URLs.

```bash
datadog unfurl "https://app.datadoghq.com/s/e16e18c08/yry-azg-bva"
```

### Notebooks

Create and update Datadog notebooks from markdown files. Write prose, log queries, metric queries, section links, and annotations — all in a single `.md` file.

```bash
# Create a notebook from markdown
datadog notebooks create --file investigation.md --title "Latency Investigation"

# Update an existing notebook
datadog notebooks update --id 12345 --file investigation.md

# Read a notebook back as markdown
datadog notebooks read --id 12345

# List notebooks
datadog notebooks list --limit 20

# Delete a notebook
datadog notebooks delete --id 12345
```

#### Markdown format

Regular markdown becomes prose cells. Fenced code blocks tagged ` ```log-query ` or ` ```metric-query ` become interactive widgets:

````markdown
# Latency Investigation

We noticed a spike starting Feb 5.

## Error logs

```log-query
{"query": "service:api status:error env:production", "time": "4h"}
```

## CPU during the incident

```metric-query
{"query": "avg:system.cpu.user{service:api}", "time": {"start": "2026-02-05T00:00:00Z", "end": "2026-02-07T00:00:00Z"}, "title": "API CPU"}
```

## Summary

See [Error logs](#error-logs) for details.

## Annotations
- 2026-02-05 13:00 UTC | red | Regression onset — latency spike
- 2026-02-06 09:00 UTC | gray | Deploy abc123
- 2026-02-07 15:30 UTC | green | Recovery — back to baseline
````

#### Section links

Use `[text](#heading-slug)` to link between sections. The CLI validates that link targets match existing headings. After creating the notebook, run the bookmarklet to resolve these into working `?cell_id=` URLs.

#### Annotations

Add an `## Annotations` section to define point-in-time markers on all graphs. Format: `YYYY-MM-DD HH:MM UTC | color | description`. Available colors: `red`, `yellow`, `green`, `blue`, `purple`, `pink`, `orange`, `gray`. Annotations are idempotent — running the bookmarklet again skips existing ones.

#### Bookmarklet

The file [`datadog/src/notebooks/dd-notebook-enhance.js`](datadog/src/notebooks/dd-notebook-enhance.js) is a browser bookmarklet that resolves section links and creates annotations. To install:

```bash
npx terser datadog/src/notebooks/dd-notebook-enhance.js --compress --mangle \
  | tr -d '\n' | sed 's/;$//' \
  | { echo -n 'javascript:void('; cat; echo -n ')'; } \
  | pbcopy
```

Create a Chrome bookmark named **DD Notebook Enhance** and paste the clipboard as the URL. **Workflow**: create/update notebook via CLI, open in browser, click the bookmarklet.

## API keys

- **API Key**: https://app.datadoghq.com/organization-settings/api-keys (copy the Key, not the ID)
- **Application Key**: https://app.datadoghq.com/organization-settings/application-keys (create a new one for yourself)
