# futures-replay-engine

Rebuilds a CME futures order book order by order from Databento's
market-by-order (level 3) data, fast enough to replay a typical ES trading day
in about a second and a half.

You give it a day of raw exchange messages and it gives you the exact state of
the book after every exchange event: best bid and ask, depth by price level,
and how many orders are resting at each price. It's meant as the foundation
for market microstructure research, where you need to know what the book
looked like at a precise moment, not just the last trade.

## Quick start

You need [Rust](https://rustup.rs). The toolchain version is pinned in
`rust-toolchain.toml`, so `rustup` installs the right one automatically.

```
git clone https://github.com/Jxxxq/futures-replay-engine.git
cd futures-replay-engine
cargo test --locked
cargo run --locked -p replay-smoke -- smoke
```

`smoke` runs a few synthetic orders through the book and checks the results:

```
status=ok
completed_events=3
add_mutates=true
trade_mutates=false
reset_clears=true
f_last_required=true
```

To replay a real day, point it at a Databento MBO file (`.dbn` or
`.dbn.zst`):

```
cargo run --release --locked -p replay-smoke -- mbo glbx-mdp3-20250916.mbo.dbn.zst
```

Output for one ES session, trimmed:

```
status=ok
mode=book_replay
dataset=GLBX.MDP3
symbols=ES.v.0
mbo_records=4995152
completed_events=4648879
action_add=2051780
action_cancel=2045196
action_modify=660183
action_trade=76872
action_fill=161120
maybe_bad_book_records=0
book_replay=ok
records_per_second=6939122
```

Add `--decode-only` to time reading the file without updating the book. No
market data is included in this repo. You can get MBO files from
[Databento](https://databento.com).

## How it works

1. **Reading.** The file is decoded one record at a time in the order the
   exchange sent it, so a 5 million record day never has to fit in memory.
2. **Book updates.** Adds, cancels, modifies and clears change the book.
   Trades and fills don't change it directly, because the filled size is
   removed by the cancel or modify message that comes right after.
   Modifies that move an order to a new price or increase its size send it to
   the back of the queue, the same as on the exchange.
3. **Events.** CME sends some changes as several messages that only make
   sense together. Records are grouped until the one marked `F_LAST`, and the
   book is only reported between complete events, never halfway through one.

A few design choices worth knowing about:

- **Fixed-point prices.** Prices are stored as whole numbers of billionths, so
  there's no floating-point rounding anywhere in the book.
- **Strict by default.** If a message doesn't make sense, like a cancel for an
  order that isn't in the book or a cancel bigger than the order, the replay
  stops with an error instead of guessing. Databento's own example book treats
  an unknown modify as a new order. I chose to stop, so a bad file can't
  quietly produce a wrong book.
- **One replay per file.** Databento's daily files start with a full snapshot
  of the book, so each file gets a fresh replay.

## Testing

`cargo test --locked` runs 35 tests on synthetic data. They cover book
resets, invalid cancels (wrong side, wrong price, zero size, too large),
cancels and modifies for orders that don't exist, modifies that change queue
priority, trades that shouldn't touch the book, and files that end partway
through an event.

I also checked it against real data:

- **Against a second, independent order book.** I wrote a separate order
  book in Python from Databento's documented rules and compared the two after
  every exchange event: best bid and ask, the top 5 price levels on each side,
  and the total number of resting orders. Across 5 trading days, including two
  with ES and NQ in the same file, they agreed on all 105,394,814 events.
- **Every file I had.** It replayed 225 ES and NQ files from 2025 and 2026
  (about 3.5 billion messages) without a single book error. The only two
  files it refused were corrupted downloads, which a separate zstd check
  confirmed. It stopped on them instead of building a bad book.

## Performance

Release build on my machine, with full book updates, averaged over repeated
runs:

| ES day | Messages | Time | Messages per second |
|---|---:|---:|---:|
| Sep 16, 2025 (light) | 5.0 million | 0.7 s | 6.7 to 6.9 million |
| Sep 26, 2025 (typical) | 11.3 million | 1.5 s | 7.7 million |
| Nov 20, 2025 (busiest I have) | 34.2 million | 4.7 s | 7.2 million |

Your numbers will depend on your hardware and the file.

## Limitations

- Built and tested on CME Globex data (`GLBX.MDP3`) from Databento. Other
  venues use different message rules and haven't been tested.
- It tracks the displayed book only. It doesn't model hidden orders or
  implied prices, and it doesn't simulate your own orders getting filled.

## Project layout

```
crates/engine-core     the order book, event grouping and the Databento reader
crates/engine-testkit  helpers for building synthetic test records
crates/engine-cli      the replay-smoke command-line tool
```

## License

MIT. See [LICENSE](LICENSE).
