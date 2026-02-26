---
name: datadog-events
description: Search Datadog events from the command line. Use when the user wants to query events (deploys, alerts, custom events) or correlate events with logs from the terminal.
---

# Datadog Events CLI

The `datadog events` subcommand searches Datadog events using the V2 Events API and outputs each event as a JSON line. It paginates automatically with cursor-based pagination.

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

### Search events by time range

```bash
datadog events --time-range "last 1 hour"
```

### Search with a query

```bash
datadog events --time-range "last 1 day" --query "source:deploy"
```

### Limit output

```bash
datadog events --time-range "last 4 hours" --limit 5
```

### Sort by oldest first

```bash
datadog events --time-range "last 1 day" --query "source:pagerduty" --sort-by oldest
```

### Tag filtering

By default, output includes `timestamp`, `title`, `message` plus all tags **except** common infrastructure noise (aws, kube, karpenter, security-group, etc.). This surfaces meaningful tags like `env`, `event_type`, `commit_hash`, `pipeline_id`, `cluster_name`, `version`.

To show only specific tags (whitelist mode), use `--tags`:

```bash
datadog events --time-range "last 1 day" --query "source:deploy" \
  --tags "env,commit_hash,version"
```

To force-include an excluded infra tag, use `--add-tags`:

```bash
datadog events --time-range "last 1 day" --add-tags "pod_name,kube_namespace"
```

Use `--all-tags` to show everything including the full raw tags array:

```bash
datadog events --time-range "last 1 hour" --all-tags
```

### Resume pagination

If a search is cut off, the CLI prints the last cursor. Resume with `--cursor`:

```bash
datadog events --time-range "last 1 day" --cursor "eyJhZnRlciI6..."
```

## Flags reference

| Flag              | Required                          | Description                                                                     |
|-------------------|-----------------------------------|---------------------------------------------------------------------------------|
| `--dd-api-key`    | Yes (or `DD_API_KEY` env)         | Datadog API key                                                                 |
| `--dd-application-key` | Yes (or `DD_APPLICATION_KEY` env) | Datadog application key                                                    |
| `--time-range`    | Yes                               | Time range string, e.g. `"last 1 day"`, `"last 30 minutes"`                    |
| `--query`         | No                                | Event search query (e.g. `"source:deploy"`)                                     |
| `--sort-by`       | No                                | Sort order: `newest` (default) or `oldest`                                      |
| `--limit`         | No                                | Maximum number of events to output                                              |
| `--cursor`        | No                                | Pagination cursor to resume a previous search                                   |
| `--tags`          | No                                | Whitelist specific tags (omit to auto-show all non-infra tags)                  |
| `--add-tags`      | No                                | Force-include additional tags (even excluded infra tags)                         |
| `--all-tags`      | No                                | Output all event attributes and the full tags array                             |

## Output format

Each event is printed as a single-line JSON object with `timestamp`, `title`, `message`, plus non-infrastructure tag values:

```json
{"timestamp":"2026-02-25T15:00:05+00:00","title":"Pipeline Started","message":"Pipeline Started","env":"production","event_type":"pipeline_started","commit_hash":"40d879be...","pipeline_id":"1257070","cluster_name":"eks-production-us-west-2-core-1","version":"0e4767ef...","service":"deploysv2","project":"multiplayer"}
```

Pipe to `jq` for further processing:

```bash
datadog events --time-range "last 1 day" --query "source:deploy" | jq '.title'
```

## Time range formats

- `"last 15 minutes"`, `"last 30 mins"`, `"last 30m"`
- `"last 1 hour"`, `"last 4 hours"`, `"last 1h"`
- `"last 1 day"`, `"last 7 days"`, `"last 1d"`
- `"last 1 week"`, `"last 2 weeks"`, `"last 1w"`
- Absolute ISO 8601 range: `"2026-02-19T17:35:00Z to 2026-02-19T23:00:00Z"`

## Tag gotchas for `service:deploysv2`

When querying deploy events with `service:deploysv2`, the `env` and `version` tags are **rejected** if explicitly requested via `--tags` or `--add-tags`:

- `env` is a Datadog reserved tag for the deploysv2 service environment, not the deploy target
- `version` is the deploysv2 service version, not the source commit — use `commit_hash` instead for the actual source SHA (for `git log`, etc.)

## Examples

```bash
# Recent deploy events for multiplayer in prod
datadog events --time-range "last 1 day" \
  --query 'service:deploysv2 project:multiplayer env:production "Pipeline Started"'

# PagerDuty alerts in the last week with extra tags
datadog events --time-range "last 1 week" --query "source:pagerduty" \
  --limit 50 --add-tags "service,version"

# All events in the last hour, oldest first, all tags
datadog events --time-range "last 1 hour" --sort-by oldest --all-tags

# Pipe to jq for filtering
datadog events --time-range "last 4 hours" | jq 'select(.event_type == "pipeline_started")'
```
