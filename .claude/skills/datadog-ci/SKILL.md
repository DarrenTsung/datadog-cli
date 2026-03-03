---
name: datadog-ci
description: Query CI Visibility test events from Datadog. Use when the user wants to find failing CI tests, investigate test flakiness, or check test status on a branch.
---

# Datadog CI Visibility CLI

The `datadog ci` subcommand queries CI Visibility test events using the Datadog V2 CI Tests API and outputs each event as a JSON line. It paginates automatically with cursor-based pagination.

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

### Find failing tests on a branch

```bash
datadog ci --time-range "last 7 days" \
  --query '@test.status:fail @git.branch:main' \
  --limit 10
```

### Query specific test failures

```bash
datadog ci --time-range "last 7 days" \
  --query '@test.service:("multiplayer-rust-tests") @test.name:"my_test" @git.branch:master @test.status:fail' \
  --limit 10
```

### Add extra columns

```bash
datadog ci --time-range "last 1 day" \
  --query '@test.status:fail' \
  --add-columns '@ci.pipeline.name,@test.suite' \
  --limit 20
```

### Show all attributes

```bash
datadog ci --time-range "last 1 hour" \
  --query '@test.status:fail @git.branch:main' \
  --all-columns --limit 5
```

### Sort by oldest first

```bash
datadog ci --time-range "last 7 days" \
  --query '@test.status:fail' \
  --sort-by oldest --limit 10
```

### Aggregate: pass/fail counts (server-side)

Use `--group-by` to get counts without fetching individual events:

```bash
datadog ci --time-range "last 7 days" \
  --query '@test.name:"my_test" @git.branch:master' \
  --group-by '@test.status'
```

Output:
```json
{"@test.status":"fail","c0":7.0}
{"@test.status":"pass","c0":340.0}
```

Multi-facet grouping:

```bash
datadog ci --time-range "last 7 days" \
  --query '@test.name:"my_test"' \
  --group-by '@test.status,@git.branch'
```

### Resume pagination

If a search is cut off, the CLI prints the last cursor. Resume with `--cursor`:

```bash
datadog ci --time-range "last 7 days" --cursor "eyJhZnRlciI6..."
```

## Flags reference

| Flag              | Required                          | Description                                                                     |
|-------------------|-----------------------------------|---------------------------------------------------------------------------------|
| `--dd-api-key`    | Yes (or `DD_API_KEY` env)         | Datadog API key                                                                 |
| `--dd-application-key` | Yes (or `DD_APPLICATION_KEY` env) | Datadog application key                                                    |
| `--time-range`    | Yes                               | Time range string, e.g. `"last 7 days"`, `"last 1 hour"`                       |
| `--query`         | No                                | CI test search query (e.g. `'@test.status:fail @git.branch:main'`)              |
| `--sort-by`       | No                                | Sort order: `newest` (default) or `oldest`                                      |
| `--limit`         | Yes*                              | Maximum number of events to output (must be <= 100, or use `--force`)           |
| `--force`         | No                                | Bypass the `--limit <= 100` guard                                               |
| `--cursor`        | No                                | Pagination cursor to resume a previous search                                   |
| `--columns`       | No                                | Comma-separated columns (default: `@test.status,@test.name,@test.service,@git.branch`) |
| `--add-columns`   | No                                | Additional columns to append to `--columns`                                     |
| `--all-columns`   | No                                | Output all attributes for each CI test event                                    |
| `--group-by`      | No                                | Group by facet(s) and return counts server-side (e.g. `'@test.status'`)         |

*`--limit` must be <= 100 unless `--force` is used. Not required when using `--group-by`.

## Output format

Each event is printed as a single-line JSON object with the selected columns:

```json
{"@test.status":"fail","@test.name":"integration_tests::my_test","@test.service":"my-service","@git.branch":"master"}
```

Pipe to `jq` for further processing:

```bash
datadog ci --time-range "last 7 days" --query '@test.status:fail' --limit 10 | jq '.["@test.name"]'
```

## Common query fields

- `@test.status` — `pass`, `fail`, `skip`
- `@test.name` — test name
- `@test.suite` — test suite name
- `@test.service` — CI test service
- `@git.branch` — branch name (supports wildcards: `mq-bot*`)
- `@ci.pipeline.name` — CI pipeline name
- `@test.framework` — test framework (e.g. `nextest`, `jest`)

## Time range formats

- `"last 15 minutes"`, `"last 30 mins"`, `"last 30m"`
- `"last 1 hour"`, `"last 4 hours"`, `"last 1h"`
- `"last 1 day"`, `"last 7 days"`, `"last 1d"`
- `"last 1 week"`, `"last 2 weeks"`, `"last 1w"`
- `"last 1 month"`, `"last 6 months"`
- `"last 1 year"`, `"last 1y"`
- Absolute ISO 8601 range: `"2026-02-19T17:35:00Z to 2026-02-19T23:00:00Z"`

## Examples

```bash
# Failing tests on master in the last week
datadog ci --time-range "last 7 days" \
  --query '@test.status:fail @git.branch:master' \
  --limit 20

# Specific flaky test across branches
datadog ci --time-range "last 7 days" \
  --query '@test.name:"my_flaky_test" @test.status:fail' \
  --add-columns '@git.branch,@ci.pipeline.name' \
  --limit 10

# All test events with full attributes for debugging
datadog ci --time-range "last 1 hour" \
  --query '@test.service:my-service @test.status:fail' \
  --all-columns --limit 5

# Pass/fail ratio for a test (single server-side query)
datadog ci --time-range "last 7 days" \
  --query '@test.name:"my_test" @git.branch:master' \
  --group-by '@test.status'

# Failure counts broken down by branch
datadog ci --time-range "last 7 days" \
  --query '@test.name:"my_test" @test.status:fail' \
  --group-by '@git.branch'
```
