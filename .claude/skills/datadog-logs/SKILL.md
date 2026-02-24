---
name: datadog-logs
description: Search and export Datadog logs from the command line. Use when the user wants to query logs, export log data, or search Datadog from the terminal.
---

# Datadog Logs CLI

The `datadog logs` subcommand searches Datadog logs and outputs each row as a JSON object. It paginates automatically and handles rate limits with retries. It uses the **flex storage tier** by default, which covers both recent and older logs (beyond the ~3 day online/standard tier window).

## Authentication

Set these environment variables (or pass as flags):

```bash
export DD_API_KEY="your-api-key"
export DD_APPLICATION_KEY="your-app-key"
```

Keys can be managed at:
- API keys: https://app.datadoghq.com/organization-settings/api-keys
- App keys: https://app.datadoghq.com/organization-settings/application-keys

## Usage

### Query with time range and query string

```bash
datadog logs --time-range "last 1 hour" --query "service:web env:production"
```

### Query from a Datadog URL

Copy a log search URL from the Datadog UI and pass it directly — the CLI extracts the query and time range automatically:

```bash
datadog logs --datadog-url "https://app.datadoghq.com/logs?query=service%3Aweb&from_ts=1605055459837&to_ts=1605228259837"
```

If the URL has no time range, it defaults to the last 15 minutes.

### Limit output rows

```bash
datadog logs --time-range "last 4 hours" --query "status:error" --limit 100
```

### Choose columns

By default, output includes `timestamp`, `service`, and `message`. Override with `--columns`:

```bash
datadog logs --time-range "last 1 hour" --query "env:production" \
  --columns "timestamp,service,@version,@file.key"
```

Or append extra columns to the defaults with `--add-columns`:

```bash
datadog logs --time-range "last 1 hour" --query "env:production" \
  --add-columns "@version,@file.key"
```

### Resume pagination

If a search is cut off by rate limiting, the CLI prints the last cursor. Resume with `--cursor`:

```bash
datadog logs --time-range "last 1 day" --query "service:web" \
  --cursor "eyJhZnRlciI6..."
```

## Flags reference

| Flag              | Required                          | Description                                                                |
|-------------------|-----------------------------------|----------------------------------------------------------------------------|
| `--dd-api-key`    | Yes (or `DD_API_KEY` env)         | Datadog API key                                                            |
| `--dd-application-key` | Yes (or `DD_APPLICATION_KEY` env) | Datadog application key                                                |
| `--datadog-url`   | No*                               | Datadog log search URL (extracts query and time range)                     |
| `--time-range`    | No*                               | Time range string, e.g. `"last 5 days"`, `"last 30 minutes"`              |
| `--query`         | No*                               | Datadog log query string                                                   |
| `--cursor`        | No                                | Pagination cursor to resume a previous search                              |
| `--limit`         | No                                | Maximum number of log rows to output                                       |
| `--columns`       | No                                | Comma-separated columns to include (default: `timestamp,service,message`)  |
| `--add-columns`   | No                                | Comma-separated columns to append to `--columns`                           |
| `--all-columns`   | No                                | Output all attributes for each log entry instead of selected columns       |
| `--sort`          | No                                | Sort order: `newest` (default) or `oldest`                                 |

*Either `--datadog-url` or both `--time-range` and `--query` must be provided.

## Column syntax

- Plain names resolve under `attributes`: `service` -> `attributes.service`
- `@` prefix is shorthand for `attributes.`: `@version` -> `attributes.version`
- Nested paths work: `@file.key` -> `attributes.file.key`
- Tag fallback: if a column isn't found in `attributes`, it's looked up in the `tags` array (e.g. `pod_name`, `kube_namespace`, `env`)

## Output format

Each log row is printed as a single-line JSON object with only the selected columns:

```json
{"timestamp":"2026-02-24T12:00:00Z","service":"web","message":"request completed"}
{"timestamp":"2026-02-24T12:00:01Z","service":"web","message":"timeout error"}
```

Pipe to `jq` for further processing:

```bash
datadog logs --time-range "last 1 hour" --query "status:error" | jq '.message'
```

## Time range formats

- `"last 15 minutes"`, `"last 30 mins"`, `"last 30m"`
- `"last 1 hour"`, `"last 4 hours"`, `"last 1h"`
- `"last 1 day"`, `"last 7 days"`, `"last 1d"`
- `"last 1 week"`, `"last 2 weeks"`, `"last 1w"`
- Datadog URLs with `from_ts` / `to_ts` query params (epoch milliseconds)

## Examples

```bash
# Search for errors in the last hour
datadog logs --time-range "last 1 hour" --query "status:error"

# Export production logs with custom columns
datadog logs --time-range "last 30 minutes" \
  --query "env:production service:auth" \
  --columns "timestamp,service,message,@duration,@status_code"

# Get first 50 logs from a Datadog URL
datadog logs --datadog-url "https://app.datadoghq.com/logs?..." --limit 50

# Pipe to jq for analysis
datadog logs --time-range "last 4 hours" --query "service:api" \
  --add-columns "@endpoint,@duration" \
  | jq 'select(.["@duration"] > 1000)'
```
