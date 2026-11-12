use std::collections::BTreeSet;
use std::fmt;
use std::fs::File;
use std::io::{self, BufReader, Read};
use std::path::Path;
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::thread;

use dbn::decode::{DbnDecoder, DbnMetadata, DecodeRecordRef, DynReader};
use dbn::{Compression, MboMsg, Metadata, Record, Schema};

use super::mbo_convert::{convert_mbo_msg, DbnConversionError};
use crate::mbo::{EngineMboRecord, MboAction};
use crate::types::{InstrumentId, PublisherId, RecordOrdinal};

const DBN_FILE_READ_BUFFER_BYTES: usize = 4 * 1024 * 1024;
/// the zstd thread hands bytes to the decoder in chunks this big, and only
/// this many can be waiting at once, so memory stays flat on big files.
const DECOMPRESSED_CHUNK_BYTES: usize = 256 * 1024;
const DECOMPRESSED_CHUNKS_QUEUED: usize = 16;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DbnMboMetadata {
    pub version: u8,
    pub dataset: String,
    pub schema: String,
    pub start: u64,
    pub end: Option<u64>,
    pub stype_in: Option<String>,
    pub stype_out: String,
    pub ts_out: bool,
    pub symbols: Vec<String>,
    pub mapping_count: usize,
}

impl From<&Metadata> for DbnMboMetadata {
    fn from(metadata: &Metadata) -> Self {
        Self {
            version: metadata.version,
            dataset: metadata.dataset.clone(),
            schema: metadata
                .schema
                .map(|schema| schema.to_string())
                .unwrap_or_else(|| "unknown".to_string()),
            start: metadata.start,
            end: metadata.end.map(|end| end.get()),
            stype_in: metadata.stype_in.map(|stype| stype.to_string()),
            stype_out: metadata.stype_out.to_string(),
            ts_out: metadata.ts_out,
            symbols: metadata.symbols.clone(),
            mapping_count: metadata.mappings.len(),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct DbnMboActionCounts {
    pub add: u64,
    pub cancel: u64,
    pub modify: u64,
    pub reset: u64,
    pub trade: u64,
    pub fill: u64,
    pub none: u64,
}

impl DbnMboActionCounts {
    fn observe(&mut self, action: MboAction) {
        match action {
            MboAction::Add => self.add += 1,
            MboAction::Cancel => self.cancel += 1,
            MboAction::Modify => self.modify += 1,
            MboAction::Reset => self.reset += 1,
            MboAction::Trade => self.trade += 1,
            MboAction::Fill => self.fill += 1,
            MboAction::None => self.none += 1,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct DbnMboFlagCounts {
    pub last: u64,
    pub snapshot: u64,
    pub bad_ts_recv: u64,
    pub maybe_bad_book: u64,
}

impl DbnMboFlagCounts {
    fn observe(&mut self, record: &EngineMboRecord) {
        self.last += u64::from(record.flags.is_last());
        self.snapshot += u64::from(record.flags.is_snapshot());
        self.bad_ts_recv += u64::from(record.flags.bad_ts_recv());
        self.maybe_bad_book += u64::from(record.flags.maybe_bad_book());
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DbnMboStreamSummary {
    pub input_bytes: u64,
    pub metadata: DbnMboMetadata,
    pub decoded_records: u64,
    pub mbo_records: u64,
    pub completed_events: u64,
    pub instrument_pairs: Vec<(PublisherId, InstrumentId)>,
    pub first_ts_recv: Option<u64>,
    pub last_ts_recv: Option<u64>,
    pub actions: DbnMboActionCounts,
    pub flags: DbnMboFlagCounts,
}

#[derive(Debug)]
pub enum DbnMboStreamError<E> {
    Io(io::Error),
    Dbn(dbn::Error),
    Metadata(String),
    Conversion {
        ordinal: u64,
        source: DbnConversionError,
    },
    UnexpectedRecordType {
        ordinal: u64,
        rtype: u8,
    },
    UnsupportedOrderBookGranularity {
        ordinal: u64,
        instrument_id: InstrumentId,
        flags: u8,
    },
    Sink {
        ordinal: u64,
        source: E,
    },
}

impl<E: fmt::Display> fmt::Display for DbnMboStreamError<E> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "I/O error: {error}"),
            Self::Dbn(error) => write!(formatter, "DBN decode error: {error}"),
            Self::Metadata(message) => write!(formatter, "invalid DBN metadata: {message}"),
            Self::Conversion { ordinal, source } => {
                write!(formatter, "MBO conversion error at ordinal {ordinal}: {source:?}")
            }
            Self::UnexpectedRecordType { ordinal, rtype } => write!(
                formatter,
                "non-MBO record type {rtype:#04x} in MBO stream at ordinal {ordinal}"
            ),
            Self::UnsupportedOrderBookGranularity {
                ordinal,
                instrument_id,
                flags,
            } => write!(
                formatter,
                "aggregated/top-of-book flags {flags:#04x} at ordinal {ordinal} for instrument {}; order-level replay stopped",
                instrument_id.get()
            ),
            Self::Sink { ordinal, source } => {
                write!(formatter, "replay sink failed at ordinal {ordinal}: {source}")
            }
        }
    }
}

/// reads a dbn file one record at a time, in the same order the exchange sent
/// them, without holding the whole file in memory.
///
/// works with plain `.dbn` and zstd-compressed `.dbn.zst` files. the callback
/// gets each record once and keeps whatever state it needs. use a fresh replay
/// for every file, since daily files usually open with a full book snapshot.
pub fn stream_mbo_records_from_dbn<F, E>(
    path: impl AsRef<Path>,
    on_record: F,
) -> Result<DbnMboStreamSummary, DbnMboStreamError<E>>
where
    F: FnMut(EngineMboRecord) -> Result<(), E>,
{
    let path = path.as_ref();
    let file = File::open(path).map_err(DbnMboStreamError::Io)?;
    let input_bytes = file.metadata().map_err(DbnMboStreamError::Io)?.len();
    let reader = BufReader::with_capacity(DBN_FILE_READ_BUFFER_BYTES, file);

    if path
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("zst"))
    {
        // unpacking zstd takes about as long as the book work, so it runs on
        // its own thread. records still come out in file order.
        thread::scope(|scope| {
            let (sender, receiver) = mpsc::sync_channel(DECOMPRESSED_CHUNKS_QUEUED);
            scope.spawn(move || decompress_into(reader, &sender));
            let decoder =
                DbnDecoder::new(ChunkReader::new(receiver)).map_err(DbnMboStreamError::Dbn)?;
            stream_from_decoder(decoder, input_bytes, on_record)
        })
    } else {
        let decoder = DbnDecoder::new(reader).map_err(DbnMboStreamError::Dbn)?;
        stream_from_decoder(decoder, input_bytes, on_record)
    }
}

/// runs on the zstd thread. unpacks the file and sends the bytes over a chunk
/// at a time. if a read fails it sends what it already has and then the
/// error. if the decoder has stopped listening it just quits.
fn decompress_into(reader: BufReader<File>, chunks: &SyncSender<io::Result<Vec<u8>>>) {
    let mut decompressed = match DynReader::with_buffer(reader, Compression::Zstd) {
        Ok(decompressed) => decompressed,
        Err(error) => {
            let _ = chunks.send(Err(io::Error::other(error)));
            return;
        }
    };
    loop {
        let mut chunk = Vec::with_capacity(DECOMPRESSED_CHUNK_BYTES);
        let result = (&mut decompressed)
            .take(DECOMPRESSED_CHUNK_BYTES as u64)
            .read_to_end(&mut chunk);
        if !chunk.is_empty() && chunks.send(Ok(chunk)).is_err() {
            return;
        }
        match result {
            Ok(0) => return,
            Ok(_) => {}
            Err(error) => {
                let _ = chunks.send(Err(error));
                return;
            }
        }
    }
}

/// reads the chunks back in order on the decoder's side.
///
/// the channel closing means the file is done. a read error comes through as
/// an `Err` chunk before that, and if the zstd thread panics the scope panics
/// too, so a broken file can't look like a normal end.
struct ChunkReader {
    chunks: Receiver<io::Result<Vec<u8>>>,
    current: Vec<u8>,
    position: usize,
}

impl ChunkReader {
    fn new(chunks: Receiver<io::Result<Vec<u8>>>) -> Self {
        Self {
            chunks,
            current: Vec::new(),
            position: 0,
        }
    }
}

impl Read for ChunkReader {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        if self.position == self.current.len() {
            let Ok(chunk) = self.chunks.recv() else {
                return Ok(0);
            };
            self.current = chunk?;
            self.position = 0;
        }
        let available = &self.current[self.position..];
        let count = available.len().min(out.len());
        out[..count].copy_from_slice(&available[..count]);
        self.position += count;
        Ok(count)
    }
}

fn stream_from_decoder<R, F, E>(
    mut decoder: DbnDecoder<R>,
    input_bytes: u64,
    mut on_record: F,
) -> Result<DbnMboStreamSummary, DbnMboStreamError<E>>
where
    R: Read,
    F: FnMut(EngineMboRecord) -> Result<(), E>,
{
    validate_metadata(decoder.metadata())?;
    let metadata = DbnMboMetadata::from(decoder.metadata());
    let mut decoded_records = 0_u64;
    let mut mbo_records = 0_u64;
    let mut instruments = BTreeSet::new();
    let mut first_ts_recv = None;
    let mut last_ts_recv = None;
    let mut actions = DbnMboActionCounts::default();
    let mut flags = DbnMboFlagCounts::default();

    while let Some(record_ref) = decoder
        .decode_record_ref()
        .map_err(DbnMboStreamError::Dbn)?
    {
        let ordinal = decoded_records;
        decoded_records += 1;
        if !record_ref.has::<MboMsg>() {
            return Err(DbnMboStreamError::UnexpectedRecordType {
                ordinal,
                rtype: record_ref.header().rtype,
            });
        }
        let mbo = record_ref
            .try_get::<MboMsg>()
            .map_err(DbnMboStreamError::Dbn)?;
        let record = convert_mbo_msg(mbo, RecordOrdinal::new(ordinal))
            .map_err(|source| DbnMboStreamError::Conversion { ordinal, source })?;
        if record.flags.is_tob() || record.flags.is_mbp() {
            return Err(DbnMboStreamError::UnsupportedOrderBookGranularity {
                ordinal,
                instrument_id: record.instrument_id,
                flags: record.flags.raw(),
            });
        }

        mbo_records += 1;
        instruments.insert((record.publisher_id, record.instrument_id));
        first_ts_recv.get_or_insert(record.ts_recv.0);
        last_ts_recv = Some(record.ts_recv.0);
        actions.observe(record.action);
        flags.observe(&record);
        on_record(record).map_err(|source| DbnMboStreamError::Sink { ordinal, source })?;
    }

    Ok(DbnMboStreamSummary {
        input_bytes,
        metadata,
        decoded_records,
        mbo_records,
        completed_events: flags.last,
        instrument_pairs: instruments.into_iter().collect(),
        first_ts_recv,
        last_ts_recv,
        actions,
        flags,
    })
}

fn validate_metadata<E>(metadata: &Metadata) -> Result<(), DbnMboStreamError<E>> {
    if metadata.dataset.trim().is_empty() {
        return Err(DbnMboStreamError::Metadata(
            "dataset must be nonempty".to_string(),
        ));
    }
    if metadata.schema != Some(Schema::Mbo) {
        let schema = metadata
            .schema
            .map(|schema| schema.to_string())
            .unwrap_or_else(|| "unknown".to_string());
        return Err(DbnMboStreamError::Metadata(format!(
            "MBO reader requires schema=mbo, got {schema}"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::ffi::c_char;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};

    use dbn::encode::{DbnEncoder, EncodeRecord};
    use dbn::{rtype, FlagSet, Metadata, RecordHeader, SType};

    use super::*;
    use crate::mbo::{F_BAD_TS_RECV, F_LAST, F_MBP, F_SNAPSHOT};
    use crate::replay::BookReplayCounter;

    static NEXT_TEMP_ID: AtomicU64 = AtomicU64::new(0);

    struct TempDbn(PathBuf);

    impl TempDbn {
        fn new(suffix: &str) -> Self {
            let id = NEXT_TEMP_ID.fetch_add(1, Ordering::Relaxed);
            Self(std::env::temp_dir().join(format!(
                "replay-engine-test-{}-{id}.{suffix}",
                std::process::id()
            )))
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempDbn {
        fn drop(&mut self) {
            let _ = fs::remove_file(&self.0);
        }
    }

    fn metadata(schema: Schema) -> Metadata {
        Metadata::builder()
            .dataset("GLBX.MDP3")
            .schema(Some(schema))
            .start(1)
            .stype_in(Some(SType::InstrumentId))
            .stype_out(SType::InstrumentId)
            .symbols(vec!["TEST".to_string()])
            .build()
    }

    #[allow(clippy::too_many_arguments)]
    fn mbo(
        sequence: u32,
        action: u8,
        side: u8,
        order_id: u64,
        price: i64,
        size: u32,
        flags: u8,
    ) -> MboMsg {
        MboMsg {
            hd: RecordHeader::new::<MboMsg>(rtype::MBO, 1, 7, u64::from(sequence)),
            order_id,
            price,
            size,
            flags: FlagSet::new(flags),
            channel_id: 2,
            action: action as c_char,
            side: side as c_char,
            ts_recv: 100 + u64::from(sequence),
            ts_in_delta: sequence as i32,
            sequence,
        }
    }

    fn records() -> Vec<MboMsg> {
        vec![
            mbo(1, b'R', b'N', 0, i64::MAX, 0, F_SNAPSHOT | F_BAD_TS_RECV),
            mbo(
                2,
                b'A',
                b'B',
                10,
                5_000_000_000_000,
                3,
                F_SNAPSHOT | F_BAD_TS_RECV,
            ),
            mbo(
                3,
                b'A',
                b'A',
                11,
                5_000_250_000_000,
                2,
                F_SNAPSHOT | F_BAD_TS_RECV | F_LAST,
            ),
            mbo(4, b'T', b'A', 11, 5_000_250_000_000, 1, F_LAST),
        ]
    }

    fn write_fixture(path: &Path, schema: Schema, records: &[MboMsg], compressed: bool) {
        let file = File::create(path).unwrap();
        if compressed {
            let mut encoder = DbnEncoder::with_zstd(file, &metadata(schema)).unwrap();
            encoder.encode_records(records).unwrap();
            encoder.flush().unwrap();
        } else {
            let mut encoder = DbnEncoder::new(file, &metadata(schema)).unwrap();
            encoder.encode_records(records).unwrap();
            encoder.flush().unwrap();
        }
    }

    fn read_fixture(path: &Path) -> (DbnMboStreamSummary, Vec<EngineMboRecord>) {
        let mut decoded = Vec::new();
        let summary = stream_mbo_records_from_dbn(path, |record| {
            decoded.push(record);
            Ok::<(), &'static str>(())
        })
        .unwrap();
        (summary, decoded)
    }

    #[test]
    fn streams_plain_and_zstd_identically_in_source_order() {
        let plain = TempDbn::new("dbn");
        let compressed = TempDbn::new("dbn.zst");
        let records = records();
        write_fixture(plain.path(), Schema::Mbo, &records, false);
        write_fixture(compressed.path(), Schema::Mbo, &records, true);

        let (plain_summary, plain_records) = read_fixture(plain.path());
        let (compressed_summary, compressed_records) = read_fixture(compressed.path());

        assert_eq!(plain_records, compressed_records);
        assert_eq!(
            plain_records
                .iter()
                .map(|record| record.ordinal.get())
                .collect::<Vec<_>>(),
            vec![0, 1, 2, 3]
        );
        assert_eq!(plain_summary.decoded_records, 4);
        assert_eq!(plain_summary.mbo_records, 4);
        assert_eq!(plain_summary.completed_events, 2);
        assert_eq!(plain_summary.actions.add, 2);
        assert_eq!(plain_summary.actions.reset, 1);
        assert_eq!(plain_summary.actions.trade, 1);
        assert_eq!(plain_summary.flags.snapshot, 3);
        assert_eq!(plain_summary.flags.bad_ts_recv, 3);
        assert_eq!(plain_summary.instrument_pairs.len(), 1);
        assert_eq!(plain_summary.metadata.schema, "mbo");
        assert_eq!(plain_summary.metadata.dataset, "GLBX.MDP3");
        assert_eq!(plain_summary.metadata, compressed_summary.metadata);
        assert_eq!(plain_summary.actions, compressed_summary.actions);
        assert_eq!(plain_summary.flags, compressed_summary.flags);
    }

    #[test]
    fn streams_directly_into_the_book_counter() {
        let path = TempDbn::new("dbn.zst");
        write_fixture(path.path(), Schema::Mbo, &records(), true);
        let mut replay = BookReplayCounter::new();
        let mut replay_events = 0_u64;

        let summary = stream_mbo_records_from_dbn(path.path(), |record| {
            if replay.ingest(record)? {
                replay_events += 1;
            }
            Ok::<(), crate::replay::ReplayCounterError>(())
        })
        .unwrap();

        replay.finish_file().unwrap();
        assert_eq!(replay_events, summary.completed_events);
        assert_eq!(replay_events, 2);
    }

    #[test]
    fn streamed_book_replay_rejects_an_incomplete_final_event() {
        let path = TempDbn::new("dbn");
        let record = mbo(1, b'A', b'B', 10, 100, 1, 0);
        write_fixture(path.path(), Schema::Mbo, &[record], false);
        let mut replay = BookReplayCounter::new();

        let summary =
            stream_mbo_records_from_dbn(path.path(), |record| replay.ingest(record).map(|_| ()))
                .unwrap();

        assert_eq!(summary.completed_events, 0);
        assert!(matches!(
            replay.finish_file(),
            Err(crate::replay::ReplayCounterError::IncompleteEventAtEnd {
                buffered_records: 1,
                ..
            })
        ));
    }

    #[test]
    fn rejects_non_mbo_metadata_before_streaming() {
        let path = TempDbn::new("dbn");
        write_fixture(path.path(), Schema::Trades, &[], false);
        let result = stream_mbo_records_from_dbn(path.path(), |_| Ok::<(), ()>(()));

        assert!(matches!(result, Err(DbnMboStreamError::Metadata(_))));
    }

    #[test]
    fn rejects_aggregated_book_flags() {
        let path = TempDbn::new("dbn");
        let record = mbo(1, b'A', b'B', 10, 100, 1, F_MBP | F_LAST);
        write_fixture(path.path(), Schema::Mbo, &[record], false);
        let result = stream_mbo_records_from_dbn(path.path(), |_| Ok::<(), ()>(()));

        assert!(matches!(
            result,
            Err(DbnMboStreamError::UnsupportedOrderBookGranularity {
                ordinal: 0,
                flags,
                ..
            }) if flags == F_MBP | F_LAST
        ));
    }

    #[test]
    fn reports_sink_failure_with_source_ordinal() {
        let path = TempDbn::new("dbn");
        write_fixture(path.path(), Schema::Mbo, &records(), false);
        let result = stream_mbo_records_from_dbn(path.path(), |record| {
            if record.ordinal.get() == 1 {
                Err("intentional stop")
            } else {
                Ok(())
            }
        });

        assert!(matches!(
            result,
            Err(DbnMboStreamError::Sink {
                ordinal: 1,
                source: "intentional stop"
            })
        ));
    }

    /// enough records to need several chunks.
    fn many_records() -> Vec<MboMsg> {
        let count = (3 * DECOMPRESSED_CHUNK_BYTES / std::mem::size_of::<MboMsg>()) as u32;
        (1..=count)
            .map(|sequence| {
                mbo(
                    sequence,
                    b'A',
                    if sequence % 2 == 0 { b'B' } else { b'A' },
                    u64::from(sequence).wrapping_mul(0x9e37_79b9_7f4a_7c15),
                    5_000_000_000_000 + i64::from(sequence % 97) * 250_000_000,
                    1 + sequence % 13,
                    F_LAST,
                )
            })
            .collect()
    }

    #[test]
    fn streams_zstd_files_bigger_than_one_chunk_in_order() {
        let plain = TempDbn::new("dbn");
        let compressed = TempDbn::new("dbn.zst");
        let records = many_records();
        write_fixture(plain.path(), Schema::Mbo, &records, false);
        write_fixture(compressed.path(), Schema::Mbo, &records, true);

        let (_, plain_records) = read_fixture(plain.path());
        let (summary, compressed_records) = read_fixture(compressed.path());

        assert_eq!(summary.mbo_records, records.len() as u64);
        assert_eq!(compressed_records, plain_records);
        assert!(compressed_records
            .iter()
            .enumerate()
            .all(|(index, record)| record.ordinal.get() == index as u64));
    }

    #[test]
    fn rejects_a_damaged_zstd_file() {
        let path = TempDbn::new("dbn.zst");
        write_fixture(path.path(), Schema::Mbo, &many_records(), true);
        let mut bytes = fs::read(path.path()).unwrap();
        let middle = bytes.len() / 2;
        for byte in &mut bytes[middle..middle + 8] {
            *byte = !*byte;
        }
        fs::write(path.path(), bytes).unwrap();

        let result = stream_mbo_records_from_dbn(path.path(), |_| Ok::<(), ()>(()));

        assert!(result.is_err());
    }
}
