---
name: datadog-notebooks
description: Create, read, update, or manage Datadog notebooks. Use when the user wants to write a notebook, convert markdown to a Datadog notebook, read a notebook back as markdown, or when the user pastes a Datadog notebook URL (e.g. https://app.datadoghq.com/notebook/...).
---

# Datadog Notebooks from Markdown

The `datadog notebooks` CLI creates and updates Datadog notebooks from `.md` files. Each markdown file is parsed into notebook cells.

## Markdown format

A notebook markdown file contains two kinds of content:

### Prose (becomes Markdown cells)

Any regular markdown — headings, paragraphs, lists, links, images — becomes a **Markdown cell** in the notebook. Standard fenced code blocks (e.g. ` ```python `) are preserved inside the markdown cell as-is.

### Log queries (becomes Log Stream cells)

A fenced code block tagged ` ```log-query ` is parsed as JSON and becomes a **Log Stream cell**. The JSON body must have a `query` field and optionally `indexes`, `columns`, and `time` fields:

With columns and relative time:

```json
{
  "query": "service:web env:production",
  "indexes": ["main"],
  "columns": ["@backend", "@error", "@resp_type"],
  "time": "4h"
}
```

Absolute time (start/end range):

```json
{
  "query": "service:web env:production",
  "time": {"start": "2026-02-20T00:00:00Z", "end": "2026-02-24T00:00:00Z"}
}
```

| Field     | Required | Description                                                  |
|-----------|----------|--------------------------------------------------------------|
| `query`   | Yes      | Datadog log query string                                     |
| `indexes` | No       | List of log indexes to search (default: all)                 |
| `columns` | No       | List of columns to display (e.g. `["@backend", "@error"]`)   |
| `time`    | No       | Per-cell time override. Either a relative span string (e.g. `"4h"`) or an absolute range object (see below). If omitted, uses the notebook's global time from `--time`. |

### Metric queries (becomes Timeseries cells)

A fenced code block tagged ` ```metric-query ` is parsed as JSON and becomes a **Timeseries cell**. The JSON body must have a `query` field and optionally a `time` field:

```json
{
  "query": "avg:system.cpu.user{env:production}",
  "time": "4h"
}
```

With title, aliases, and display type:

```json
{
  "query": "avg:system.cpu.user{env:production} by {host}",
  "title": "CPU Usage by Host",
  "aliases": {"avg:system.cpu.user{env:production} by {host}": "CPU"},
  "display_type": "area",
  "time": {"start": "2026-02-17T08:00:00Z", "end": "2026-02-26T08:00:00Z"}
}
```

| Field          | Required | Description                                                  |
|----------------|----------|--------------------------------------------------------------|
| `query`        | Yes      | Datadog metric query string (e.g. `"avg:system.cpu.user{*}"`) |
| `time`         | No       | Per-cell time override (same format as log-query `time`). If omitted, uses the notebook's global time from `--time`. |
| `title`        | No       | Graph title displayed above the timeseries widget.           |
| `aliases`      | No       | Map of query expression to display name for the legend. Example: `{"avg:system.cpu.user{*}": "CPU Usage"}` |
| `display_type` | No       | Graph style: `"line"` (default), `"bars"`, or `"area"`.     |

## Example markdown file

````markdown
# Production Error Investigation

We've seen a spike in 5xx errors from the auth service.

## CPU usage during the incident

```metric-query
{"query": "avg:system.cpu.user{service:auth,env:production}", "time": "4h"}
```

## Auth service errors

```log-query
{"query": "service:auth status:error env:production"}
```

Errors appear clustered around 2pm UTC. Let's check the downstream database service:

## Database timeouts

```log-query
{"query": "service:postgres-proxy @duration:>5000 env:production", "indexes": ["main"], "time": "1d"}
```

## Next steps

- Check recent deploys to auth service
- Review connection pool settings
````

This produces 6 notebook cells:

1. **Markdown** — title and intro paragraph
2. **Timeseries** — `avg:system.cpu.user{service:auth,env:production}` (time: 4h)
3. **Log Stream** — `service:auth status:error env:production`
4. **Markdown** — analysis paragraph and "Database timeouts" heading
5. **Log Stream** — `service:postgres-proxy @duration:>5000 env:production` (index: main, time: 1d)
6. **Markdown** — "Next steps" list

## CLI usage

```bash
# Create a notebook
datadog notebooks create --file notebook.md --title "Error Investigation"

# Create with custom time span
datadog notebooks create --file notebook.md --title "Investigation" --time 4h

# List all notebooks
datadog notebooks list

# Read a notebook back as markdown (by ID or URL)
datadog notebooks read --id 12345
datadog notebooks read --id https://app.datadoghq.com/notebook/12345/some-title

# Update an existing notebook (preserves title if --title omitted)
datadog notebooks update --id 12345 --file notebook.md

# Update with new title
datadog notebooks update --id 12345 --file notebook.md --title "New Title"

# Delete a notebook
datadog notebooks delete --id 12345
```

## Time span values

The `--time` CLI flag sets the notebook's global time span (default: `1h`). Individual log-query and metric-query cells can override this with the `"time"` JSON field.

| Value | Meaning          |
|-------|------------------|
| `1m`  | Past 1 minute    |
| `5m`  | Past 5 minutes   |
| `10m` | Past 10 minutes  |
| `15m` | Past 15 minutes  |
| `30m` | Past 30 minutes  |
| `1h`  | Past 1 hour      |
| `4h`  | Past 4 hours     |
| `1d`  | Past 1 day       |
| `2d`  | Past 2 days      |
| `1w`  | Past 1 week      |

## Rules

- Empty or whitespace-only markdown between special blocks is dropped (no empty cells)
- Leading/trailing blank lines on markdown cells are trimmed
- Regular code fences (` ```python `, ` ```json `, etc.) are treated as normal markdown
- A ` ```log-query ` or ` ```metric-query ` inside another fenced block is **not** treated as special
- Unterminated ` ```log-query ` or ` ```metric-query ` blocks produce an error
- Invalid JSON inside a special block produces an error
- The `query` field is required in both log-query and metric-query JSON; missing it is an error
