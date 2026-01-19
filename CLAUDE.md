# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Build and Test Commands

```bash
cargo build          # Build the library
cargo test           # Run all tests
cargo test <name>    # Run a specific test by name
cargo clippy         # Run linter
cargo fmt            # Format code
cargo doc --no-deps  # Generate documentation
```

## Project Overview

`throttled-tracing` is a Rust library providing periodic and throttled logging macros for the `tracing` ecosystem. The crate is designed to be published on crates.io.

## Architecture

The library provides eight categories of macros, each backed by a state struct:

- **`OnceState`** / `*_once!`: Log only the first time using `AtomicBool`
- **`ThrottleState`** / `*_every!`: Duration-based throttling using `Mutex<Option<Instant>>`
- **`CountState`** / `*_every_n!`: Log every N occurrences using `AtomicUsize`
- **`FirstNState`** / `*_first_n!`: Log only first N occurrences using `AtomicUsize`
- **`BackoffState`** / `*_backoff!`: Exponential backoff using `Mutex` with interval doubling
- **`OnChangeState`** / `*_on_change!`: Log on value change using hash comparison
- **`OncePerValueState`** / `*_once_per_value!`: Log once per unique value using `HashSet<u64>`
- **`SampleState`** / `*_sample!`: Probabilistic logging using `fastrand`

Each macro invocation creates its own static state variable, ensuring independent throttling per call site. All state types are thread-safe.

Macros are available for all five tracing levels: `trace`, `debug`, `info`, `warn`, `error` (40 macros total).
