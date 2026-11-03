# futures-replay-engine

A Rust engine that rebuilds a futures order book one order at a time from CME
market-by-order (level 3) data in Databento's DBN format.

It reads a day of data in the order the exchange sent it, applies each add,
cancel, modify and clear to the book (trades and fills are recorded but don't
change it, since the cancel that follows does), and groups records into
complete exchange events using the `F_LAST` flag. Prices are fixed-point integers, so
there's no floating-point drift in the book.

## What's in here

- `crates/engine-core`: the book, the event assembler and the DBN reader
- `crates/engine-testkit`: small builders for writing synthetic test records
- `crates/engine-cli`: a command-line tool for replaying a file and timing it

The tests cover book resets, cancels for orders that don't exist, modifies that
change queue priority, trades that shouldn't touch the book, and files that end
partway through an event.

## Running it

```
cargo test --locked
cargo run --locked -p replay-smoke -- smoke
cargo run --release --locked -p replay-smoke -- mbo path/to/file.mbo.dbn.zst
```

`smoke` replays a few synthetic records and checks the results. `mbo` replays a
real file and prints record counts and throughput. Add `--decode-only` to time
reading the file without updating the book.

No market data is included. You'll need your own MBO files from Databento.

## Speed

On my machine, a release build replayed a 79 MiB ES day (about 5.0 million
records) with full book updates at roughly 6.8 to 6.9 million records per
second. Your numbers will depend on your hardware and the file.
