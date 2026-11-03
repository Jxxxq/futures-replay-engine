use std::env;
use std::path::Path;
use std::process::ExitCode;
use std::time::Instant;

use replay_core::databento::stream_mbo_records_from_dbn;
use replay_core::mbo::{MboAction, MboSide};
use replay_core::replay::{BookReplayCounter, EventAssembler, ReplayScope};
use replay_core::types::{InstrumentId, PublisherId, SessionId};
use replay_testkit::builders::MboBuilder;

fn main() -> ExitCode {
    let mut args = env::args().skip(1);
    let command = args.next().unwrap_or_else(|| "help".to_string());
    match command.as_str() {
        "--version" | "version" => {
            println!("{} 0.1.0", replay_core::ENGINE_NAME);
            ExitCode::SUCCESS
        }
        "smoke" => match smoke() {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("smoke failed: {error}");
                ExitCode::from(2)
            }
        },
        "mbo" => {
            let args = args.collect::<Vec<_>>();
            match parse_mbo_args(&args).and_then(|(path, decode_only)| mbo(&path, decode_only)) {
                Ok(()) => ExitCode::SUCCESS,
                Err(error) => {
                    eprintln!("mbo failed: {error}");
                    ExitCode::from(2)
                }
            }
        }
        "help" | "--help" | "-h" => {
            print_help();
            ExitCode::SUCCESS
        }
        unknown => {
            eprintln!("unknown command: {unknown}");
            print_help();
            ExitCode::from(2)
        }
    }
}

fn print_help() {
    println!("replay-smoke --version");
    println!("replay-smoke smoke");
    println!("replay-smoke mbo <file.dbn[.zst]> [--decode-only]");
}

fn parse_mbo_args(args: &[String]) -> Result<(String, bool), String> {
    let Some(path) = args.first() else {
        return Err("usage: replay-smoke mbo <file.dbn[.zst]> [--decode-only]".to_string());
    };
    let mut decode_only = false;
    for argument in &args[1..] {
        match argument.as_str() {
            "--decode-only" if !decode_only => decode_only = true,
            "--decode-only" => return Err("--decode-only was supplied twice".to_string()),
            _ => return Err(format!("unknown mbo argument: {argument}")),
        }
    }
    Ok((path.clone(), decode_only))
}

fn mbo(path: impl AsRef<Path>, decode_only: bool) -> Result<(), String> {
    let path = path.as_ref();
    let started = Instant::now();
    let mut replay = (!decode_only).then(BookReplayCounter::new);
    let mut replay_completed_events = 0_u64;

    let summary = stream_mbo_records_from_dbn(path, |record| {
        if let Some(replay) = replay.as_mut() {
            if replay
                .ingest(record)
                .map_err(|error| format!("{error:?}"))?
            {
                replay_completed_events += 1;
            }
        }
        Ok::<(), String>(())
    })
    .map_err(|error| format!("{}: {error}", path.display()))?;

    if let Some(replay) = replay.as_ref() {
        replay
            .finish_file()
            .map_err(|error| format!("{}: finish_file: {error:?}", path.display()))?;
        if replay_completed_events != summary.completed_events {
            return Err(format!(
                "{}: reader counted {} completed events but replay counted {}",
                path.display(),
                summary.completed_events,
                replay_completed_events
            ));
        }
    }

    let elapsed = started.elapsed().as_secs_f64();
    let records_per_second = if elapsed > 0.0 {
        summary.mbo_records as f64 / elapsed
    } else {
        0.0
    };
    let input_mib_per_second = if elapsed > 0.0 {
        summary.input_bytes as f64 / (1024.0 * 1024.0) / elapsed
    } else {
        0.0
    };
    let instruments = summary
        .instrument_pairs
        .iter()
        .map(|(publisher, instrument)| format!("{}:{}", publisher.get(), instrument.get()))
        .collect::<Vec<_>>()
        .join(",");

    println!("status=ok");
    println!(
        "mode={}",
        if decode_only {
            "decode_only"
        } else {
            "book_replay"
        }
    );
    println!("path={}", path.display());
    println!("dbn_version={}", summary.metadata.version);
    println!("dataset={}", summary.metadata.dataset);
    println!("schema={}", summary.metadata.schema);
    println!("start_ns={}", summary.metadata.start);
    println!(
        "end_ns={}",
        summary
            .metadata
            .end
            .map(|value| value.to_string())
            .unwrap_or_else(|| "none".to_string())
    );
    println!(
        "stype_in={}",
        summary.metadata.stype_in.as_deref().unwrap_or("mixed")
    );
    println!("stype_out={}", summary.metadata.stype_out);
    println!("symbols={}", summary.metadata.symbols.join(","));
    println!("mapping_count={}", summary.metadata.mapping_count);
    println!("input_bytes={}", summary.input_bytes);
    println!("decoded_records={}", summary.decoded_records);
    println!("mbo_records={}", summary.mbo_records);
    println!("completed_events={}", summary.completed_events);
    println!("instruments={instruments}");
    println!("action_add={}", summary.actions.add);
    println!("action_cancel={}", summary.actions.cancel);
    println!("action_modify={}", summary.actions.modify);
    println!("action_reset={}", summary.actions.reset);
    println!("action_trade={}", summary.actions.trade);
    println!("action_fill={}", summary.actions.fill);
    println!("action_none={}", summary.actions.none);
    println!("snapshot_records={}", summary.flags.snapshot);
    println!("bad_ts_recv_records={}", summary.flags.bad_ts_recv);
    println!("maybe_bad_book_records={}", summary.flags.maybe_bad_book);
    println!("book_replay={}", if decode_only { "skipped" } else { "ok" });
    println!("elapsed_seconds={elapsed:.6}");
    println!("records_per_second={records_per_second:.0}");
    println!("input_mib_per_second={input_mib_per_second:.2}");
    Ok(())
}

fn smoke() -> Result<(), String> {
    let scope = ReplayScope::new("SYNTHETIC", SessionId::new("2026-01-05")).with_instrument(
        PublisherId::new(0),
        InstrumentId::new(1),
        "TEST",
    );
    let mut replay = EventAssembler::new_scoped(scope);

    let bid = MboBuilder::new()
        .bid(10, 5_000_000_000_000, 3)
        .into_records()
        .pop()
        .expect("synthetic bid");
    let bid_event = replay
        .ingest(bid)
        .map_err(|error| format!("bid ingest: {error:?}"))?
        .ok_or_else(|| "F_LAST bid did not complete".to_string())?;
    if bid_event.snapshot.best_bid_px.is_none() || bid_event.snapshot.order_count != 1 {
        return Err("add did not mutate the displayed book".to_string());
    }

    let trade = MboBuilder::new()
        .action(MboAction::Trade, MboSide::Ask, 99, 5_000_000_000_000, 1)
        .into_records()
        .pop()
        .expect("synthetic trade");
    let trade_event = replay
        .ingest(trade)
        .map_err(|error| format!("trade ingest: {error:?}"))?
        .ok_or_else(|| "F_LAST trade did not complete".to_string())?;
    if trade_event.snapshot != bid_event.snapshot {
        return Err("trade incorrectly mutated the displayed book".to_string());
    }

    let reset = MboBuilder::new()
        .action(MboAction::Reset, MboSide::None, 0, 0, 0)
        .into_records()
        .pop()
        .expect("synthetic reset");
    let reset_event = replay
        .ingest(reset)
        .map_err(|error| format!("reset ingest: {error:?}"))?
        .ok_or_else(|| "F_LAST reset did not complete".to_string())?;
    if reset_event.snapshot.order_count != 0 || reset_event.snapshot.best_bid_px.is_some() {
        return Err("reset did not clear the displayed book".to_string());
    }
    replay
        .finish_file()
        .map_err(|error| format!("finish_file: {error:?}"))?;

    println!("status=ok");
    println!("completed_events=3");
    println!("add_mutates=true");
    println!("trade_mutates=false");
    println!("reset_clears=true");
    println!("f_last_required=true");
    Ok(())
}
