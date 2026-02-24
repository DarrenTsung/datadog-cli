---
name: datadog-metrics
description: Query Datadog metrics from the command line. Use when the user wants to query metric data, view timeseries graphs, or inspect metric values from the terminal.
---

# Datadog Metrics CLI

The `datadog metrics query` subcommand queries Datadog metrics and displays a summary with a terminal chart, or outputs raw data points as JSON lines.

## Authentication

Set these environment variables (or pass as flags):

```bash
export DD_API_KEY="your-api-key"
export DD_APPLICATION_KEY="your-app-key"
```

## Usage

### Basic query

```bash
datadog metrics query --query "avg:system.cpu.user{env:production}" --time "last 1 hour"
```

### Query with grouping

```bash
datadog metrics query --query "sum:multiplayer.users.current{env:production} by {version}" --time "last 4 hours"
```

### Raw JSON output

```bash
datadog metrics query --query "avg:system.cpu.user{*}" --time "last 1 hour" --raw
```

### Using a Datadog URL time range

```bash
datadog metrics query --query "avg:system.cpu.user{*}" --time "from_ts=1771886671256&to_ts=1771973071256"
```

## Flags

| Flag      | Required | Description                                                       |
|-----------|----------|-------------------------------------------------------------------|
| `--query` | Yes      | Datadog metric query string (e.g. `"avg:system.cpu.user{*}"`)     |
| `--time`  | Yes      | Time range — `"last 1 hour"`, `"last 4 hours"`, or `from_ts=...&to_ts=...` from a Datadog URL |
| `--raw`   | No       | Output raw `(timestamp, value)` JSON lines instead of summary     |

## Default output (summary)

For each series, prints:
- Display name and tags
- Point count and interval
- Min / Max / Avg / Last values
- A braille line chart (via `textplots`) with local timestamps on the X axis

When a query returns multiple series (e.g. `by {host}`), all charts share the same Y axis for easy comparison, and each series is plotted at its correct position within the full time range.

## Raw output (`--raw`)

Each data point as a JSON line:

```json
{"series":"env:production,host:web-01","timestamp":"2026-02-24T12:00:00+00:00","value":12.3}
```

## Time range formats

- `"last 15 minutes"`, `"last 30 mins"`, `"last 30m"`
- `"last 1 hour"`, `"last 4 hours"`, `"last 1h"`
- `"last 1 day"`, `"last 7 days"`, `"last 1d"`
- `"last 1 week"`, `"last 2 weeks"`, `"last 1w"`
- Datadog URLs with `from_ts` / `to_ts` query params (epoch milliseconds)

## Metric query syntax

Standard Datadog metric query syntax:

- `avg:system.cpu.user{env:production}` — average CPU by env tag
- `sum:requests.count{service:web}.as_count()` — sum of request counts
- `avg:system.cpu.user{env:production} by {host}` — grouped by host
- `sum:multiplayer.docs.load_failed{env:production} by {error}.as_count()` — grouped errors

## Examples

```bash
# CPU usage in production over the last hour
datadog metrics query --query "avg:system.cpu.user{env:production}" --time "last 1 hour"

# Current users by version over the last day
datadog metrics query --query "sum:multiplayer.users.current{env:production} by {version}" --time "last 1 day"

# Raw data points for piping to other tools
datadog metrics query --query "avg:system.memory.used{*}" --time "last 4 hours" --raw | jq '.value'
```
