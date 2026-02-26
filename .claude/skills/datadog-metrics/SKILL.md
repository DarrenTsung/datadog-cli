---
name: datadog-metrics
description: Query Datadog metrics from the command line. Use when the user wants to query metric data, view timeseries graphs, or inspect metric values from the terminal.
---

# Datadog Metrics CLI

The `datadog metrics` command has two subcommands:

- **`query`** — Query Datadog metrics and display a summary with a terminal chart, or output raw data points as JSON lines.
- **`tag-values`** — List known tag values for a given metric and tag key.

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

### Rollup into fixed buckets

```bash
# Hourly buckets over the last day
datadog metrics query --query "avg:system.cpu.user{env:production}" --time "last 1 day" --rollup hourly

# Custom 5-minute buckets
datadog metrics query --query "avg:system.cpu.user{env:production}" --time "last 4 hours" --rollup 5m
```

### Compare before/after a pivot timestamp

```bash
datadog metrics query --query "avg:system.cpu.user{env:production}" \
  --time "2026-02-19T14:00:00Z to 2026-02-20T02:00:00Z" \
  --compare "2026-02-19T17:35:00Z"
```

### Combined rollup + compare

```bash
datadog metrics query --query "avg:system.cpu.user{env:production}" \
  --time "2026-02-19T14:00:00Z to 2026-02-20T02:00:00Z" \
  --rollup hourly --compare "2026-02-19T17:35:00Z"
```

## Flags

| Flag        | Required | Description                                                       |
|-------------|----------|-------------------------------------------------------------------|
| `--query`   | Yes      | Datadog metric query string (e.g. `"avg:system.cpu.user{*}"`). Repeatable with `name=` prefix for formula queries (e.g. `--query "a=count:metric{*}"`) |
| `--formula` | No       | Combine named queries with arithmetic (e.g. `--formula "a * b"`). Requires all `--query` values to have a `name=` prefix |
| `--time`    | Yes      | Time range — `"last 1 hour"`, `"last 4 hours"`, or `from_ts=...&to_ts=...` from a Datadog URL |
| `--raw`     | No       | Output raw `(timestamp, value)` JSON lines instead of summary     |
| `--rollup`  | No       | Roll up data points into fixed-size buckets. Accepts `"hourly"`, `"daily"`, or a duration like `"5m"`, `"4h"`, `"2d"` |
| `--compare` | No       | Compare before/after a pivot timestamp. Accepts ISO 8601 (e.g. `"2026-02-19T17:35:00Z"`) or epoch seconds |

## Default output (summary)

For each series, prints:
- Display name and tags
- Point count
- Min / Max / Avg / Last values
- A braille line chart (via `textplots`) with local timestamps on the X axis

When a query returns multiple series (e.g. `by {host}`), all charts share the same Y axis for easy comparison, and each series is plotted at its correct position within the full time range.

## Rollup output (`--rollup`)

Aggregates data points into fixed-size time buckets and prints a table:

```
Bucket                |       Avg |       Min |       Max |     n
----------------------+-----------+-----------+-----------+------
Mon 14:00 - 15:00     |    9012.3 |    8500.1 |    9800.8 |    30
Mon 15:00 - 16:00     |    8750.5 |    8100.0 |    9400.2 |    30
```

## Compare output (`--compare`)

Splits data points into before/after windows around a pivot timestamp:

```
            |       Avg |       Min |       Max |     n
------------+-----------+-----------+-----------+------
Before      |    9012.3 |    7879.0 |   10001.0 |   107
After       |    8493.7 |    6479.0 |   13175.0 |   253
Delta       |    -518.6 |           |           |
```

## Combined rollup + compare

Same rollup table with a `=== PIVOT ===` separator line between the before and after windows.

## Raw output (`--raw`)

Each data point as a JSON line:

```json
{"series":"env:production,host:web-01","timestamp":"2026-02-24T12:00:00+00:00","value":12.3}
```

With `--raw --rollup`, outputs bucketed aggregates:

```json
{"series":"env:production","bucket_start":"2026-02-19T14:00:00+00:00","bucket_end":"2026-02-19T15:00:00+00:00","avg":9012.3,"min":8500.1,"max":9800.8,"count":30}
```

With `--raw --compare`, outputs before/after stats:

```json
{"series":"env:production","period":"before","avg":9012.3,"min":7879.0,"max":10001.0,"count":107}
{"series":"env:production","period":"after","avg":8493.7,"min":6479.0,"max":13175.0,"count":253}
```

With `--raw --rollup --compare`, bucketed aggregates include a `period` field (`"before"`, `"after"`, or `"pivot"`).

## Formula queries (`--formula`)

Combine multiple named queries with arithmetic in a single API call. Each `--query` must have a short `name=` prefix (e.g. `a=`, `b=`), and `--formula` defines the arithmetic expression.

```bash
# Compute worker-seconds: count * avg_latency
datadog metrics query \
  --query "a=count:sinatra.async_worker.jobs{env:production}.as_count()" \
  --query "b=avg:sinatra.async_worker.jobs.execution_time_distrib{env:production}" \
  --formula "a * b" \
  --time "2026-02-17T00:00:00Z to 2026-02-22T00:00:00Z"

# With rollup and compare
datadog metrics query \
  --query "a=count:sinatra.async_worker.jobs{env:production}.as_count()" \
  --query "b=avg:sinatra.async_worker.jobs.execution_time_distrib{env:production}" \
  --formula "a * b" \
  --time "2026-02-17T00:00:00Z to 2026-02-22T00:00:00Z" \
  --rollup hourly --compare "2026-02-19T17:00:00Z"

# Multiple formulas in one call
datadog metrics query \
  --query "a=sum:requests.count{service:web}.as_count()" \
  --query "b=sum:errors.count{service:web}.as_count()" \
  --formula "a" --formula "b / a * 100" \
  --time "last 4 hours"
```

All output modes (`--raw`, `--rollup`, `--compare`, and combinations) work with formula queries.

## Tag values

List known tag values for a given metric. Useful for discovering valid tag values before constructing a metrics query.

```bash
# List all job_name values for a metric
datadog metrics tag-values \
  --metric "sinatra.async_worker.jobs.execution_time_distrib" \
  --tag "job_name"

# Filter with a glob pattern
datadog metrics tag-values \
  --metric "sinatra.async_worker.jobs.execution_time_distrib" \
  --tag "job_name" \
  --filter "*file_chunk*"
```

### Tag values flags

| Flag       | Required | Description                                                   |
|------------|----------|---------------------------------------------------------------|
| `--metric` | Yes      | Metric name (e.g. `"sinatra.async_worker.jobs.execution_time_distrib"`) |
| `--tag`    | Yes      | Tag key to list values for (e.g. `"job_name"`)                |
| `--filter` | No       | Glob filter on tag values (e.g. `"*file_chunk*"`)             |

## Time range formats

- `"last 15 minutes"`, `"last 30 mins"`, `"last 30m"`
- `"last 1 hour"`, `"last 4 hours"`, `"last 1h"`
- `"last 1 day"`, `"last 7 days"`, `"last 1d"`
- `"last 1 week"`, `"last 2 weeks"`, `"last 1w"`
- `"last 1 month"`, `"last 6 months"`
- `"last 1 year"`, `"last 1y"`
- Absolute ISO 8601 range: `"2026-02-19T17:35:00Z to 2026-02-19T23:00:00Z"`
- Datadog URLs with `from_ts` / `to_ts` query params (epoch milliseconds)

## Metric query syntax

Standard Datadog metric query syntax:

- `avg:system.cpu.user{env:production}` — average CPU by env tag
- `sum:requests.count{service:web}.as_count()` — sum of request counts
- `avg:system.cpu.user{env:production} by {host}` — grouped by host
- `sum:multiplayer.docs.load_failed{env:production} by {error}.as_count()` — grouped errors

## Common Patterns

### Rate / ratio calculations (ALWAYS use --formula)

When computing a rate, ratio, or percentage from two metrics, ALWAYS use `--formula` to combine them in a single API call. Do NOT query the metrics separately and compute mentally.

```bash
# OOM rate: terminations / executions * 100
datadog metrics query \
  --query "a=sum:sinatra.async_worker.jobs.terminated{env:production,reason:oom}.as_count()" \
  --query "b=count:sinatra.async_worker.jobs.execution_time_distrib{env:production}.as_count()" \
  --formula "a / b * 100" \
  --time "last 1 day" --rollup daily

# Error rate as a percentage
datadog metrics query \
  --query "a=sum:requests.count{service:web}.as_count()" \
  --query "b=sum:errors.count{service:web}.as_count()" \
  --formula "b / a * 100" \
  --time "last 4 hours"
```

### Batch across tag values with `by {tag}` (AVOID per-value loops)

When you need the same metric or formula for multiple tag values (e.g. multiple job names, hosts, services), ALWAYS use `by {tag}` grouping instead of issuing separate queries per tag value. This works with `--formula`, `--compare`, and `--rollup` — all compose together.

```bash
# GOOD: one query returns all job types at once
datadog metrics query \
  --query "a=count:sinatra.async_worker.jobs.full_latency_distrib{service:high-memory-worker, env:production} by {job_name}.as_count()" \
  --query "b=avg:sinatra.async_worker.jobs.execution_time_distrib{service:high-memory-worker, env:production} by {job_name}" \
  --formula "a * b" \
  --time "2026-02-17T08:00:00Z to 2026-02-26T08:00:00Z" \
  --rollup daily --compare "2026-02-22T00:00:00Z"

# BAD: querying each job_name separately — DO NOT DO THIS
datadog metrics query --query "a=count:metric{job_name:job1} ..." --formula "a * b" --time "..."
datadog metrics query --query "a=count:metric{job_name:job2} ..." --formula "a * b" --time "..."
datadog metrics query --query "a=count:metric{job_name:job3} ..." --formula "a * b" --time "..."
# ... repeating N times = N unnecessary API calls
```

### Discovering tag values before querying

Use `tag-values` to find valid tag values instead of guessing. This avoids wasted queries from incorrect tag names.

```bash
# Find job names containing "file_chunk"
datadog metrics tag-values \
  --metric "sinatra.async_worker.jobs.execution_time_distrib" \
  --tag "job_name" \
  --filter "*file_chunk*"

# Then use the exact value in a query
datadog metrics query \
  --query "avg:sinatra.async_worker.jobs.execution_time_distrib{job_name:ml_file_chunks_index_job}" \
  --time "last 4 hours"
```

### Before/after comparison (ALWAYS use --compare)

When comparing metrics before vs after a specific point in time (deploy, config change, incident, regression), ALWAYS use `--compare` with a pivot timestamp instead of issuing two separate queries with different `--time` ranges. This halves the number of API calls and gives you a clean before/after delta automatically.

```bash
# GOOD: single query with --compare
datadog metrics query \
  --query "avg:system.cpu.user{env:production}" \
  --time "2026-02-19T14:00:00Z to 2026-02-20T02:00:00Z" \
  --compare "2026-02-19T17:35:00Z"

# BAD: two separate queries — DO NOT DO THIS
datadog metrics query --query "avg:system.cpu.user{env:production}" --time "2026-02-19T14:00:00Z to 2026-02-19T17:35:00Z"
datadog metrics query --query "avg:system.cpu.user{env:production}" --time "2026-02-19T17:35:00Z to 2026-02-20T02:00:00Z"

# With hourly rollup for more detail
datadog metrics query \
  --query "avg:system.cpu.user{env:production}" \
  --time "2026-02-19T14:00:00Z to 2026-02-20T02:00:00Z" \
  --rollup hourly --compare "2026-02-19T17:35:00Z"

# Works with --formula too — compare derived metrics in one call
datadog metrics query \
  --query "a=count:sinatra.async_worker.jobs{env:production}.as_count()" \
  --query "b=avg:sinatra.async_worker.jobs.execution_time_distrib{env:production}" \
  --formula "a * b" \
  --time "2026-02-17T00:00:00Z to 2026-02-26T00:00:00Z" \
  --rollup daily --compare "2026-02-22T00:00:00Z"
```

## Examples

```bash
# CPU usage in production over the last hour
datadog metrics query --query "avg:system.cpu.user{env:production}" --time "last 1 hour"

# Current users by version over the last day
datadog metrics query --query "sum:multiplayer.users.current{env:production} by {version}" --time "last 1 day"

# Raw data points for piping to other tools
datadog metrics query --query "avg:system.memory.used{*}" --time "last 4 hours" --raw | jq '.value'

# Hourly rollup over the last day
datadog metrics query --query "avg:system.cpu.user{env:production}" --time "last 1 day" --rollup hourly

# Compare before/after a deploy
datadog metrics query --query "avg:system.cpu.user{env:production}" \
  --time "2026-02-19T14:00:00Z to 2026-02-20T02:00:00Z" \
  --compare "2026-02-19T17:35:00Z"

# Hourly rollup with pivot separator at deploy time
datadog metrics query --query "avg:system.cpu.user{env:production}" \
  --time "2026-02-19T14:00:00Z to 2026-02-20T02:00:00Z" \
  --rollup hourly --compare "2026-02-19T17:35:00Z"

# Raw bucketed JSON for scripting
datadog metrics query --query "avg:system.cpu.user{env:production}" \
  --time "last 1 day" --rollup daily --raw

# Formula: compute derived metric from two queries
datadog metrics query \
  --query "a=count:sinatra.async_worker.jobs{env:production}.as_count()" \
  --query "b=avg:sinatra.async_worker.jobs.execution_time_distrib{env:production}" \
  --formula "a * b" \
  --time "last 1 day" --rollup hourly

# Formula: error rate as a percentage
datadog metrics query \
  --query "a=sum:requests.count{service:web}.as_count()" \
  --query "b=sum:errors.count{service:web}.as_count()" \
  --formula "b / a * 100" \
  --time "last 4 hours"
```
