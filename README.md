# datadog-cli
A tool for exporting raw logs from Datadog's log API. This is very similar to `datadog_export`, however this tool uses Datadog's log API (https://docs.datadoghq.com/logs/guide/collect-multiple-logs-with-pagination/?tab=v2api) and the `datadog_export` scans over the archives in S3.

Datadog's log API is much faster than scanning through the archives because it uses indices when searching. However, we are rate limited to a max of ~~300~~ 1080 requests per hour (1,000 logs per request).

You should usually be using this tool for exporting raw logs, unless you're looking to export logs that aren't present in the main index of datadog (debug logs, logs older than two weeks, etc).

Actually, if you want to export logs outside of the main index, it should be possible. You'd need to [rehydrate the logs](https://docs.datadoghq.com/logs/log_configuration/rehydrating/)and export logs from the temporary index that's created instead of the main index. Take a look at the [API docs](https://docs.datadoghq.com/api/latest/logs/#search-logs) for the endpoint we're using.

## Installation
```
cargo install --path datadog-cli
```

This installs the `datadog-cli` binary to `~/.cargo/bin`.

## Usage
Here's a basic example of how to use this tool:
```
❯ cargo run --release -- --dd-api-key $YOUR_DATADOG_API_KEY --dd-application-key $YOUR_DATADOG_APPLICATION_KEY --time-range "last 4 hours" --query "First request for file, setting up  host:multiplayer-138.prod.figma.com"
```

You can also pass in a datadog url for the time range. This reads the query parameters `from_ts=XXX` and `to_ts=XXX`.
```
❯ cargo run --release -- --dd-api-key $YOUR_DATADOG_API_KEY --dd-application-key $YOUR_DATADOG_APPLICATION_KEY --time-range "https://app.datadoghq.com/logs?cols=service%2C%40meta.res.statusCode&from_ts=1605823753503&index=&live=true&messageDisplay=inline&stream_sort=desc&to_ts=1605824653503&query=encoding%20the%20doc" --query "First request for file, setting up  host:multiplayer-138.prod.figma.com"
```

The tool outputs all matching logs to stdout, if you want to write to a file redirect the output to a file like so:
```
❯ cargo run --release -- --dd-api-key $YOUR_DATADOG_API_KEY --dd-application-key $YOUR_DATADOG_APPLICATION_KEY --time-range "last 4 hours" --query "First request for file, setting up  host:multiplayer-138.prod.figma.com" > files_opened_on_multiplayer_138.txt
```

Please use the --help flag to view all options and arguments:
```
❯ cargo run -- --help
datadog-cli 0.1.0
A tool for collecting logs from the Datadog log API.

USAGE:
    datadog-cli [OPTIONS] --dd-api-key <dd-api-key> --dd-application-key <dd-application-key> --query <query> --time-range <time-range>

FLAGS:
    -h, --help       Prints help information
    -V, --version    Prints version information

OPTIONS:
        --cursor <cursor>
            (Optional) The cursor to provide for the initial API call. Use this to resume pagination after a search was
            cut-off
        --dd-api-key <dd-api-key>
            The Datadog API key, see: https://app.datadoghq.com/organization-settings/api-keys

        --dd-application-key <dd-application-key>
            The Datadog application key, see: https://app.datadoghq.com/organization-settings/application-keys

        --query <query>
            A datadog log query like: "env:production @file.key:YVAxndRJlWC4GoOoGmo8pu"

        --time-range <time-range>
            Time-range of logs to search through. Eg. "last 5 days". You can also provide a datadog url!
```


## Notebooks

Create and update Datadog notebooks from markdown files. Write prose, log queries, metric queries, section links, and annotations — all in a single `.md` file.

### Quick start

```bash
# Create a notebook from markdown
datadog notebooks create --file investigation.md --title "Latency Investigation"

# Update an existing notebook
datadog notebooks update --id 12345 --file investigation.md

# Read a notebook back as markdown
datadog notebooks read --id 12345
```

### Markdown format

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
````

### Section links

Use `[text](#heading-slug)` to link between sections. The CLI validates that all link targets match an existing heading. After creating the notebook, run the **DD Notebook Enhance** bookmarklet in your browser to resolve these into working Datadog `?cell_id=` URLs.

### Annotations

Add an `## Annotations` section to define point-in-time markers on all graphs:

```markdown
## Annotations
- 2026-02-05 13:00 UTC | red | Regression onset — latency spike
- 2026-02-06 09:00 UTC | gray | Deploy abc123
- 2026-02-07 15:30 UTC | green | Recovery — back to baseline
```

Colors: `red`, `yellow`, `green`, `blue`, `purple`, `pink`, `orange`, `gray`

Annotations are created by the bookmarklet (not the CLI) using Datadog's internal annotation API. They are idempotent — running the bookmarklet again skips existing annotations.

### Bookmarklet setup

The file `datadog/src/notebooks/dd-notebook-enhance.js` is a browser bookmarklet that resolves section links and creates annotations. To install:

1. Generate the bookmarklet:
   ```bash
   npx terser datadog/src/notebooks/dd-notebook-enhance.js --compress --mangle \
     | tr -d '\n' | sed 's/;$//' \
     | { echo -n 'javascript:void('; cat; echo -n ')'; } \
     | pbcopy
   ```
2. Create a Chrome bookmark named **DD Notebook Enhance**
3. Paste the clipboard as the bookmark URL

**Workflow**: create/update notebook via CLI, open it in the browser, click the bookmarklet.

## How to get a Datadog API and APP Key

Find an existing API Key here: https://app.datadoghq.com/organization-settings/api-keys (You can use an arbitrary one, make sure to copy the Key and not the ID)

![image](https://user-images.githubusercontent.com/104477175/207144554-2b9d24d5-d7d0-4da9-9ae0-2005f5e3ccfa.png)


Find an Application Key here: https://app.datadoghq.com/organization-settings/application-keys
Create a new key for yourself in the upper right and then copy your Application Key.

![image](https://user-images.githubusercontent.com/104477175/207144594-6cc314f1-5553-40b6-87b6-cdcfb8f334a1.png)
![image](https://user-images.githubusercontent.com/104477175/207144618-7566f467-98c0-4f3d-ac17-026392711c04.png)
