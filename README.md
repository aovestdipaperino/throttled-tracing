# throttled-tracing

[![GitHub stars](https://img.shields.io/github/stars/aovestdipaperino/throttled-tracing?logo=github&color=brightgreen)](https://github.com/aovestdipaperino/throttled-tracing/stargazers)

Periodic and throttled logging macros for the [tracing](https://crates.io/crates/tracing) ecosystem.

## Features

This crate provides macros that extend `tracing` with rate-limited logging capabilities:

- **`*_once!`** - Log only the first time the macro is reached
- **`*_every!(duration, ...)`** - Log at most once per specified duration
- **`*_every_n!(n, ...)`** - Log every N occurrences
- **`*_first_n!(n, ...)`** - Log only the first N occurrences
- **`*_backoff!(initial, max, ...)`** - Log with exponential backoff
- **`*_on_change!(value, ...)`** - Log only when the tracked value changes
- **`*_once_per_value!(key, ...)`** - Log once per unique key value
- **`*_sample!(probability, ...)`** - Log with probability sampling

All macros are available for each tracing level: `trace`, `debug`, `info`, `warn`, `error`.

## Usage

```rust
use throttled_tracing::{info_once, debug_every, warn_every_n};
use std::time::Duration;

fn process_item(item: u32) {
    // Only logs the first time this line is reached
    info_once!("Processing started");

    // Logs at most once per second
    debug_every!(Duration::from_secs(1), "Processing item {}", item);

    // Logs every 100th call
    warn_every_n!(100, "Processed {} items so far", item);
}
```

```rust
use throttled_tracing::{error_backoff, info_on_change, debug_once_per_value, trace_sample};
use std::time::Duration;

fn handle_error(err: &str) {
    // Exponential backoff: logs at 1s, 2s, 4s, 8s... up to 60s
    error_backoff!(Duration::from_secs(1), Duration::from_secs(60), "Error: {}", err);
}

fn monitor_status(status: u32) {
    // Only logs when status changes
    info_on_change!(status, "Status changed to {}", status);
}

fn process_user(user_id: u64) {
    // Logs once per unique user_id
    debug_once_per_value!(user_id, "First time seeing user {}", user_id);
}

fn high_volume_operation() {
    // Logs ~1% of calls
    trace_sample!(0.01, "Sampled operation");
}
```

## Available Macros

| Level | Once | Duration | Every N | First N | Backoff | On Change | Per Value | Sample |
|-------|------|----------|---------|---------|---------|-----------|-----------|--------|
| TRACE | `trace_once!` | `trace_every!` | `trace_every_n!` | `trace_first_n!` | `trace_backoff!` | `trace_on_change!` | `trace_once_per_value!` | `trace_sample!` |
| DEBUG | `debug_once!` | `debug_every!` | `debug_every_n!` | `debug_first_n!` | `debug_backoff!` | `debug_on_change!` | `debug_once_per_value!` | `debug_sample!` |
| INFO | `info_once!` | `info_every!` | `info_every_n!` | `info_first_n!` | `info_backoff!` | `info_on_change!` | `info_once_per_value!` | `info_sample!` |
| WARN | `warn_once!` | `warn_every!` | `warn_every_n!` | `warn_first_n!` | `warn_backoff!` | `warn_on_change!` | `warn_once_per_value!` | `warn_sample!` |
| ERROR | `error_once!` | `error_every!` | `error_every_n!` | `error_first_n!` | `error_backoff!` | `error_on_change!` | `error_once_per_value!` | `error_sample!` |

## How It Works

Each macro invocation maintains its own static state to track logging frequency. This means:

- Different call sites have independent throttling
- State persists for the lifetime of the program
- Thread-safe by default

**Note:** `*_on_change!` and `*_once_per_value!` use hash-based comparison, so values must implement `Hash`. The `*_once_per_value!` macro's memory grows with each unique value seen.

## License

Licensed under either of Apache License, Version 2.0 or MIT license at your option.
