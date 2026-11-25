# TDRUST

An experimental native financial charting workspace built with Rust and egui.

## Run

Install a recent Rust toolchain, then run:

```sh
cargo run --locked
```

The application starts with generated OHLCV data. Load a CSV with:

```sh
cargo run --locked -- path/to/bars.csv
```

CSV input uses the first six columns:

```text
time,open,high,low,close,volume
```

A header is optional. Timestamps can be Unix seconds or nanoseconds.

## Features

- Multiple chart widgets with independent viewports.
- Technical studies and drawing tools.
- Workspace layout and session persistence.
- Market-data adapters and historical order-flow tooling.

Live data availability depends on the selected provider. Provider credentials
must be supplied through environment variables; do not commit credentials or
local market-data files.

## Development

```sh
cargo fmt --check
cargo test --locked
```
