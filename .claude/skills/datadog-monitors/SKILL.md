---
name: datadog-monitors
description: Inspect Datadog monitors from the command line. Use when the user shares a Datadog monitor URL or wants to investigate a monitor alert — shows metadata, underlying metric chart, and monitor events.
---

# Datadog Monitors CLI

The `datadog monitors inspect` subcommand fetches a monitor's metadata, queries its underlying metric over a time window, and searches for related monitor events. Accepts a full Datadog monitor URL or a bare numeric ID.

## Authentication

Set these environment variables (or pass as flags):

```bash
export DD_API_KEY="your-api-key"
export DD_APPLICATION_KEY="your-app-key"
```

## Usage

### Inspect from a monitor URL

```bash
datadog monitors inspect "https://app.datadoghq.com/monitors/51915671?event_id=abc123&from_ts=1772058251000&to_ts=1772059403749&live=true"
```

This auto-extracts the monitor ID, time range, and event ID from the URL.

### Inspect by numeric ID

```bash
datadog monitors inspect 51915671
```

Defaults to "last 1 hour" when no time range is available.

### Override time range

```bash
datadog monitors inspect 51915671 --time "last 4 hours"
```

### Specify a trigger event

```bash
datadog monitors inspect 51915671 --event "abc123"
```

### Raw JSON output

```bash
datadog monitors inspect 51915671 --raw
```

## Flags

| Flag      | Required | Description                                                                 |
|-----------|----------|-----------------------------------------------------------------------------|
| `--time`  | No       | Time range override (e.g. `"last 4 hours"`, `"last 1 day"`)                |
| `--event` | No       | Specific event ID to highlight (auto-parsed from URL's `event_id` param)   |
| `--raw`   | No       | Output each section as JSON lines                                           |

## Output sections

### 1. Monitor metadata

Shows name, ID, type, status, creation/modification dates, creator, query, thresholds, tags, and notification message.

### 2. Trigger event (when `--event` is provided or parsed from URL)

Shows the specific event that triggered the notification — timestamp, status, title, groups, and message excerpt.

### 3. Underlying metric (for metric alert and query alert monitors)

Automatically extracts the metric query from the monitor query and displays a summary with a terminal chart (reuses the `datadog metrics query` output format).

### 4. Monitor events

Lists recent monitor state transitions (alert, warn, ok, etc.) within the time range.

## Time range formats

- `"last 15 minutes"`, `"last 30 mins"`, `"last 30m"`
- `"last 1 hour"`, `"last 4 hours"`, `"last 1h"`
- `"last 1 day"`, `"last 7 days"`, `"last 1d"`
- `"last 1 week"`, `"last 2 weeks"`, `"last 1w"`
- Absolute ISO 8601 range: `"2026-02-19T17:35:00Z to 2026-02-19T23:00:00Z"`

## Examples

```bash
# Full investigation from a monitor alert notification URL
datadog monitors inspect "https://app.datadoghq.com/monitors/51915671?event_id=abc&from_ts=1772058251000&to_ts=1772059403749"

# Quick check on a monitor with a wider time window
datadog monitors inspect 51915671 --time "last 4 hours"

# Raw JSON for piping to jq
datadog monitors inspect 51915671 --raw | jq 'select(.section == "metric")'

# Just monitor metadata and events (non-metric monitors)
datadog monitors inspect 98765432 --time "last 1 day"
```
