---
name: datadog-unfurl
description: Unfurl a Datadog dashboard, metric explorer, or notebook URL (e.g. https://app.datadoghq.com/s/e16e18c08/yry-azg-bva) — resolve shared links, show widget details, and download the snapshot image. Use when the user pastes a Datadog dashboard, metric explorer, notebook, or shared link and wants to understand what it shows.
---

# Datadog Unfurl

`datadog unfurl` takes a Datadog URL, resolves it, and shows a human-readable summary of the widget(s) along with a snapshot image.

## Usage

```bash
# Shared short link (resolves automatically — works for dashboards, metric explorer, and notebooks)
datadog unfurl "https://app.datadoghq.com/s/e16e18c08/yry-azg-bva"

# Direct dashboard URL with a focused widget
datadog unfurl "https://app.datadoghq.com/dashboard/5iv-bx7-9xp/multiplayer-v2?fullscreen_widget=3737456056966802"

# Full dashboard (no widget focus) — lists all widgets
datadog unfurl "https://app.datadoghq.com/dashboard/5iv-bx7-9xp/multiplayer-v2"

# Metric explorer URL (widget definition is decoded from the URL fragment)
datadog unfurl "https://app.datadoghq.com/metric/explorer?start=123&end=456#N4Ig..."

# Full JSON output
datadog unfurl "https://app.datadoghq.com/s/e16e18c08/yry-azg-bva" --json
```

## What it does

1. **Resolves `/s/` short links** — follows the redirect and extracts the real URL (dashboard, metric explorer, or notebook)
2. **Fetches dashboard data** via the Datadog API (for dashboard URLs)
3. **Decodes metric explorer fragments** — metric explorer URLs encode the widget definition as lz-string in the URL fragment; the tool decompresses and parses it
4. **Fetches notebook cells** — for notebook URLs, fetches the notebook via API and shows the specific cell (if `cell_id` is present) or all cells, using the same widget format as dashboards
5. **Shows widget details** — title, type, formulas, and metric queries in a readable format
6. **Downloads the snapshot image** to the current directory (for shared links only — this is the same `og:image` that Slack unfurls). Filenames include the ID when available (`dd-widget-<ID>.png`, `dd-notebook-<ID>.png`) and auto-increment (`-1`, `-2`, ...) to avoid overwriting existing files

## Supported URL formats

| Format | Example |
|--------|---------|
| Shared link | `https://app.datadoghq.com/s/TOKEN/ID` |
| Dashboard with widget | `https://app.datadoghq.com/dashboard/ID/title?fullscreen_widget=123` |
| Dashboard (all widgets) | `https://app.datadoghq.com/dashboard/ID/title` |
| Metric explorer | `https://app.datadoghq.com/metric/explorer?start=...&end=...#N4Ig...` |
| Notebook cell | `https://app.datadoghq.com/notebook/ID?cell_id=CELL_ID` (typically via short link) |

## Widget focus

If the URL contains `fullscreen_widget` or `tile_focus` query params, only that specific widget is shown. This is the default when using shared links created from a focused widget.

## Snapshot image

When unfurling a shared link, the tool downloads the `og:image` — the exact same image Slack shows when you paste the link. This image may include cursor annotations (timestamp and metric count) that aren't available in the text output.

## Example output

```
Resolving short link...
Dashboard: Multiplayer (13 widgets)
## File Download and Launch Failed  [id: 3737456056966802]
Type: timeseries

Request 1:
  Formula: default_zero(query1)
  Query (query1): sum:multiplayer.docs.load_failed{$env,$pod_name} by {error}.as_count()
Snapshot: dd-widget-3737456056966802.png
(Tip: the snapshot may include cursor annotations — timestamp and count — not shown above)
```
