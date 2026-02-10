# datadog_utils

A helper crate for tools that interop with datadog / time-ranges. Converts strings like "last 30 minutes" to chrono DateTimes.

Can also extract the time ranges from datadog urls like:
https://app.datadoghq.com/dashboard/ebf-xrc-9sz?from_ts=1635367665415&to_ts=1635369465415&live=true
