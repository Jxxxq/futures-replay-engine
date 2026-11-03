use std::collections::BTreeMap;

use crate::book::{BookError, BookLevel, BookMutation, BookSnapshot, DisplayedBook};
use crate::fast_hash::FastHashMap;
use crate::mbo::{EngineMboRecord, MboAction, MboSide};
use crate::types::{
    BookEpoch, ChannelId, InstrumentId, InstrumentKey, PublisherId, SessionId, StreamIdentity,
    StreamKey,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CompletedMboEvent {
    pub stream_key: StreamKey,
    pub instrument_id: InstrumentId,
    pub records: Vec<EngineMboRecord>,
    pub mutations: Vec<BookMutation>,
    pub snapshot: BookSnapshot,
    pub bid_depth: Vec<BookLevel>,
    pub ask_depth: Vec<BookLevel>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum EventAssemblerError {
    Book(BookError),
    MixedStreamScope {
        instrument_id: InstrumentId,
        previous_publisher_id: PublisherId,
        previous_channel_id: ChannelId,
        current_publisher_id: PublisherId,
        current_channel_id: ChannelId,
    },
    UnresolvedInstrumentKey {
        publisher_id: PublisherId,
        instrument_id: InstrumentId,
    },
    ResetInsideOpenEvent {
        stream_key: StreamKey,
        buffered_records: usize,
    },
    IncompleteEventAtEnd {
        stream_key: StreamKey,
        buffered_records: usize,
    },
}

impl From<BookError> for EventAssemblerError {
    fn from(value: BookError) -> Self {
        Self::Book(value)
    }
}

#[derive(Clone, Debug)]
pub struct ReplayScope {
    dataset: String,
    session_id: SessionId,
    symbol_epochs: BTreeMap<(PublisherId, InstrumentId), String>,
    publisher_independent_symbol_epochs: BTreeMap<InstrumentId, String>,
    require_resolved_instruments: bool,
}

impl ReplayScope {
    pub fn new(dataset: impl Into<String>, session_id: SessionId) -> Self {
        Self {
            dataset: dataset.into(),
            session_id,
            symbol_epochs: BTreeMap::new(),
            publisher_independent_symbol_epochs: BTreeMap::new(),
            require_resolved_instruments: true,
        }
    }

    pub fn with_instrument(
        mut self,
        publisher_id: PublisherId,
        instrument_id: InstrumentId,
        symbol_epoch: impl Into<String>,
    ) -> Self {
        self.symbol_epochs
            .insert((publisher_id, instrument_id), symbol_epoch.into());
        self
    }

    pub fn with_instrument_any_publisher(
        mut self,
        instrument_id: InstrumentId,
        symbol_epoch: impl Into<String>,
    ) -> Self {
        self.publisher_independent_symbol_epochs
            .insert(instrument_id, symbol_epoch.into());
        self
    }

    fn unscoped() -> Self {
        Self {
            dataset: "unscoped".to_string(),
            session_id: SessionId::new("unscoped"),
            symbol_epochs: BTreeMap::new(),
            publisher_independent_symbol_epochs: BTreeMap::new(),
            require_resolved_instruments: false,
        }
    }

    fn resolve(&self, record: &EngineMboRecord) -> Result<InstrumentKey, EventAssemblerError> {
        let symbol_epoch = self
            .symbol_epochs
            .get(&(record.publisher_id, record.instrument_id))
            .cloned()
            .or_else(|| {
                self.publisher_independent_symbol_epochs
                    .get(&record.instrument_id)
                    .cloned()
            })
            .or_else(|| {
                (!self.require_resolved_instruments)
                    .then(|| format!("unresolved-{}", record.instrument_id.get()))
            })
            .ok_or(EventAssemblerError::UnresolvedInstrumentKey {
                publisher_id: record.publisher_id,
                instrument_id: record.instrument_id,
            })?;
        Ok(InstrumentKey::new(
            self.dataset.clone(),
            record.publisher_id,
            record.instrument_id,
            symbol_epoch,
        ))
    }
}

struct InstrumentReplayState {
    identity: StreamIdentity,
    book_epoch: BookEpoch,
    book: DisplayedBook,
    buffer: Vec<EngineMboRecord>,
    mutations: Vec<BookMutation>,
}

impl InstrumentReplayState {
    fn stream_key(&self) -> StreamKey {
        self.identity.at_epoch(self.book_epoch)
    }
}

pub struct EventAssembler {
    replay_scope: ReplayScope,
    states: FastHashMap<InstrumentKey, InstrumentReplayState>,
    depth_limit: usize,
}

impl Default for EventAssembler {
    fn default() -> Self {
        Self {
            replay_scope: ReplayScope::unscoped(),
            states: FastHashMap::default(),
            depth_limit: 10,
        }
    }
}

impl EventAssembler {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn new_with_depth_limit(depth_limit: usize) -> Self {
        Self {
            depth_limit,
            ..Self::default()
        }
    }

    pub fn new_scoped(replay_scope: ReplayScope) -> Self {
        Self {
            replay_scope,
            ..Self::default()
        }
    }

    pub fn new_scoped_with_depth_limit(replay_scope: ReplayScope, depth_limit: usize) -> Self {
        Self {
            replay_scope,
            states: FastHashMap::default(),
            depth_limit,
        }
    }

    pub fn ingest(
        &mut self,
        record: EngineMboRecord,
    ) -> Result<Option<CompletedMboEvent>, EventAssemblerError> {
        let instrument_id = record.instrument_id;
        let instrument_key = self.replay_scope.resolve(&record)?;
        let session_id = self.replay_scope.session_id.clone();
        let state = self
            .states
            .entry(instrument_key.clone())
            .or_insert_with(|| InstrumentReplayState {
                identity: StreamIdentity::new(
                    instrument_key.clone(),
                    record.channel_id,
                    session_id,
                ),
                book_epoch: BookEpoch::default(),
                book: DisplayedBook::new(instrument_id),
                buffer: Vec::new(),
                mutations: Vec::new(),
            });
        if state.identity.channel_id != record.channel_id {
            return Err(EventAssemblerError::MixedStreamScope {
                instrument_id,
                previous_publisher_id: state.identity.instrument.publisher_id,
                previous_channel_id: state.identity.channel_id,
                current_publisher_id: record.publisher_id,
                current_channel_id: record.channel_id,
            });
        }

        if record.action == MboAction::Reset {
            if !state.buffer.is_empty() {
                return Err(EventAssemblerError::ResetInsideOpenEvent {
                    stream_key: state.stream_key(),
                    buffered_records: state.buffer.len(),
                });
            }
            state.book_epoch = state.book_epoch.next();
        }
        let mutation = state.book.apply(&record)?;

        let is_last = record.is_last();
        state.buffer.push(record);
        state.mutations.push(mutation);

        if !is_last {
            return Ok(None);
        }

        let records = std::mem::take(&mut state.buffer);
        let mutations = std::mem::take(&mut state.mutations);
        let snapshot = state.book.snapshot();
        let bid_depth = if self.depth_limit == 0 {
            Vec::new()
        } else {
            state
                .book
                .depth_limited(crate::mbo::MboSide::Bid, self.depth_limit)
        };
        let ask_depth = if self.depth_limit == 0 {
            Vec::new()
        } else {
            state
                .book
                .depth_limited(crate::mbo::MboSide::Ask, self.depth_limit)
        };

        Ok(Some(CompletedMboEvent {
            stream_key: state.stream_key(),
            instrument_id,
            records,
            mutations,
            snapshot,
            bid_depth,
            ask_depth,
        }))
    }

    pub fn incomplete_record_count(&self, instrument_id: InstrumentId) -> usize {
        self.states
            .values()
            .filter(|state| state.identity.instrument.instrument_id == instrument_id)
            .map(|state| state.buffer.len())
            .sum()
    }

    pub fn finish_file(&self) -> Result<(), EventAssemblerError> {
        if let Some(state) = self.states.values().find(|state| !state.buffer.is_empty()) {
            return Err(EventAssemblerError::IncompleteEventAtEnd {
                stream_key: state.stream_key(),
                buffered_records: state.buffer.len(),
            });
        }
        Ok(())
    }

    pub fn reset_for_next_file(&mut self) -> Result<(), EventAssemblerError> {
        self.finish_file()?;
        self.states.clear();
        Ok(())
    }

    pub fn book_snapshot_for_diagnostics(
        &self,
        instrument: &InstrumentKey,
    ) -> Option<BookSnapshot> {
        self.states
            .get(instrument)
            .map(|state| state.book.snapshot())
    }

    pub fn book_depth_limited(
        &self,
        instrument: &InstrumentKey,
        side: MboSide,
        max_levels: usize,
    ) -> Vec<BookLevel> {
        self.states
            .get(instrument)
            .map(|state| state.book.depth_limited(side, max_levels))
            .unwrap_or_default()
    }

    pub fn book_qty_at_price_limited(
        &self,
        instrument: &InstrumentKey,
        side: MboSide,
        price: crate::types::Px9,
        max_levels: usize,
    ) -> crate::types::Qty {
        self.states
            .get(instrument)
            .map(|state| state.book.qty_at_price_limited(side, price, max_levels))
            .unwrap_or_default()
    }

    /// how many separate orders are resting at `price`, as of the event you're
    /// asking about.
    ///
    /// returns nothing if that stream doesn't exist or the book has been reset
    /// since the event, so call it right after `ingest` hands the event back.
    pub fn book_order_count_at_price(
        &self,
        stream_key: &StreamKey,
        side: MboSide,
        price: crate::types::Px9,
    ) -> Option<usize> {
        let state = self.states.get(&stream_key.instrument)?;
        (state.stream_key() == *stream_key).then(|| state.book.order_count_at_price(side, price))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mbo::{MboAction, MboFlags, MboSide, F_LAST};
    use crate::types::{
        ChannelId, OrderId, PublisherId, Px9, Qty, RecordOrdinal, RecordType, Sequence, TsEvent,
        TsInDelta, TsRecv,
    };

    fn record(
        ordinal: u64,
        action: MboAction,
        side: MboSide,
        order_id: u64,
        price: i64,
        size: u32,
        flags: u8,
    ) -> EngineMboRecord {
        record_with_scope(
            ordinal,
            action,
            side,
            order_id,
            price,
            size,
            flags,
            PublisherId::new(0),
            ChannelId::new(0),
            InstrumentId::new(1),
        )
    }

    fn record_with_scope(
        ordinal: u64,
        action: MboAction,
        side: MboSide,
        order_id: u64,
        price: i64,
        size: u32,
        flags: u8,
        publisher_id: PublisherId,
        channel_id: ChannelId,
        instrument_id: InstrumentId,
    ) -> EngineMboRecord {
        EngineMboRecord {
            ordinal: RecordOrdinal::new(ordinal),
            rtype: RecordType::new(0),
            publisher_id,
            instrument_id,
            channel_id,
            action,
            side,
            price_px9: Px9::new(price),
            size: Qty::new(size),
            order_id: OrderId::new(order_id),
            flags: MboFlags::new(flags),
            ts_event: TsEvent(1),
            ts_recv: TsRecv(1),
            ts_in_delta: TsInDelta(0),
            sequence: Sequence(ordinal as u32),
        }
    }

    #[test]
    fn rejects_mixed_publisher_or_channel_for_same_instrument() {
        let mut assembler = EventAssembler::new();
        let first = record_with_scope(
            1,
            MboAction::Add,
            MboSide::Bid,
            1,
            100,
            5,
            F_LAST,
            PublisherId::new(1),
            ChannelId::new(7),
            InstrumentId::new(55),
        );
        assembler.ingest(first).unwrap();

        let second = record_with_scope(
            2,
            MboAction::Add,
            MboSide::Ask,
            2,
            101,
            5,
            F_LAST,
            PublisherId::new(1),
            ChannelId::new(8),
            InstrumentId::new(55),
        );
        let err = assembler.ingest(second).unwrap_err();
        assert_eq!(
            err,
            EventAssemblerError::MixedStreamScope {
                instrument_id: InstrumentId::new(55),
                previous_publisher_id: PublisherId::new(1),
                previous_channel_id: ChannelId::new(7),
                current_publisher_id: PublisherId::new(1),
                current_channel_id: ChannelId::new(8),
            }
        );
    }

    #[test]
    fn scoped_identity_separates_equal_numeric_ids_across_publishers() {
        let scope = ReplayScope::new("GLBX.MDP3", SessionId::new("2026-03-03"))
            .with_instrument(PublisherId::new(1), InstrumentId::new(55), "ESM6")
            .with_instrument(PublisherId::new(2), InstrumentId::new(55), "NQM6");
        let mut assembler = EventAssembler::new_scoped(scope);
        let es = assembler
            .ingest(record_with_scope(
                1,
                MboAction::Add,
                MboSide::Bid,
                1,
                100,
                5,
                F_LAST,
                PublisherId::new(1),
                ChannelId::new(7),
                InstrumentId::new(55),
            ))
            .unwrap()
            .unwrap();
        let nq = assembler
            .ingest(record_with_scope(
                2,
                MboAction::Add,
                MboSide::Ask,
                2,
                201,
                8,
                F_LAST,
                PublisherId::new(2),
                ChannelId::new(9),
                InstrumentId::new(55),
            ))
            .unwrap()
            .unwrap();

        assert_ne!(es.stream_key, nq.stream_key);
        assert_eq!(es.stream_key.instrument.symbol_epoch, "ESM6");
        assert_eq!(nq.stream_key.instrument.symbol_epoch, "NQM6");
        assert_eq!(es.snapshot.best_bid_qty, Qty::new(5));
        assert_eq!(nq.snapshot.best_ask_qty, Qty::new(8));
        assert_eq!(
            assembler
                .book_snapshot_for_diagnostics(&es.stream_key.instrument)
                .unwrap(),
            es.snapshot
        );
        assert_eq!(
            assembler
                .book_snapshot_for_diagnostics(&nq.stream_key.instrument)
                .unwrap(),
            nq.snapshot
        );
        assert_eq!(
            assembler.book_depth_limited(&es.stream_key.instrument, MboSide::Bid, 10),
            es.bid_depth
        );
        assert_eq!(
            assembler.book_depth_limited(&nq.stream_key.instrument, MboSide::Ask, 10),
            nq.ask_depth
        );
        assert_eq!(
            assembler.book_qty_at_price_limited(
                &es.stream_key.instrument,
                MboSide::Bid,
                Px9::new(100),
                10,
            ),
            Qty::new(5)
        );
        assert_eq!(
            assembler.book_qty_at_price_limited(
                &nq.stream_key.instrument,
                MboSide::Ask,
                Px9::new(201),
                10,
            ),
            Qty::new(8)
        );
    }

    #[test]
    fn scoped_identity_rejects_unresolved_instrument() {
        let mut assembler =
            EventAssembler::new_scoped(ReplayScope::new("GLBX.MDP3", SessionId::new("2026-03-03")));
        assert!(matches!(
            assembler.ingest(record(1, MboAction::Add, MboSide::Bid, 1, 100, 5, F_LAST)),
            Err(EventAssemblerError::UnresolvedInstrumentKey { .. })
        ));
    }

    #[test]
    fn finish_rejects_incomplete_event_and_file_reset_clears_state() {
        let mut assembler = EventAssembler::new();
        assembler
            .ingest(record(1, MboAction::Add, MboSide::Bid, 1, 100, 5, 0))
            .unwrap();
        assert!(matches!(
            assembler.finish_file(),
            Err(EventAssemblerError::IncompleteEventAtEnd {
                buffered_records: 1,
                ..
            })
        ));

        let mut complete = EventAssembler::new();
        let event = complete
            .ingest(record(1, MboAction::Add, MboSide::Bid, 1, 100, 5, F_LAST))
            .unwrap()
            .unwrap();
        complete.reset_for_next_file().unwrap();
        assert_eq!(
            complete.book_snapshot_for_diagnostics(&event.stream_key.instrument),
            None
        );
    }

    #[test]
    fn reset_advances_book_epoch() {
        let mut assembler = EventAssembler::new();
        let before = assembler
            .ingest(record(1, MboAction::Add, MboSide::Bid, 1, 100, 5, F_LAST))
            .unwrap()
            .unwrap();
        let after = assembler
            .ingest(record(2, MboAction::Reset, MboSide::None, 0, 0, 0, F_LAST))
            .unwrap()
            .unwrap();
        assert_eq!(before.stream_key.book_epoch, BookEpoch(0));
        assert_eq!(after.stream_key.book_epoch, BookEpoch(1));
        assert_eq!(after.snapshot.order_count, 0);
        assert_eq!(
            assembler.book_order_count_at_price(&before.stream_key, MboSide::Bid, Px9::new(100)),
            None,
            "a stale completed-event epoch must not read the replacement book"
        );
    }

    #[test]
    fn completed_event_epoch_exposes_exact_resting_order_count() {
        let mut assembler = EventAssembler::new();
        assert!(assembler
            .ingest(record(1, MboAction::Add, MboSide::Ask, 1, 101, 2, 0))
            .unwrap()
            .is_none());
        let event = assembler
            .ingest(record(2, MboAction::Add, MboSide::Ask, 2, 101, 3, F_LAST))
            .unwrap()
            .unwrap();
        assert_eq!(
            assembler.book_order_count_at_price(&event.stream_key, MboSide::Ask, Px9::new(101)),
            Some(2)
        );
        assert_eq!(
            assembler.book_order_count_at_price(&event.stream_key, MboSide::Bid, Px9::new(101)),
            Some(0)
        );
    }

    #[test]
    fn withholds_snapshot_until_f_last() {
        let mut assembler = EventAssembler::new();
        let first = record(1, MboAction::Add, MboSide::Bid, 1, 100, 5, 0);
        assert!(assembler.ingest(first).unwrap().is_none());
        assert_eq!(assembler.incomplete_record_count(InstrumentId::new(1)), 1);

        let second = record(2, MboAction::Add, MboSide::Ask, 2, 101, 7, F_LAST);
        let event = assembler
            .ingest(second)
            .unwrap()
            .expect("F_LAST emits event");
        assert_eq!(event.records.len(), 2);
        assert_eq!(event.snapshot.best_bid_px, Some(Px9::new(100)));
        assert_eq!(event.snapshot.best_ask_px, Some(Px9::new(101)));
        assert_eq!(event.bid_depth.len(), 1);
        assert_eq!(event.ask_depth.len(), 1);
    }

    #[test]
    fn depth_limit_zero_keeps_snapshot_but_skips_depth_vectors() {
        let mut assembler = EventAssembler::new_with_depth_limit(0);
        assembler
            .ingest(record(1, MboAction::Add, MboSide::Bid, 1, 100, 5, F_LAST))
            .unwrap();
        let event = assembler
            .ingest(record(2, MboAction::Add, MboSide::Ask, 2, 101, 7, F_LAST))
            .unwrap()
            .expect("F_LAST emits event");

        assert_eq!(event.snapshot.best_bid_px, Some(Px9::new(100)));
        assert_eq!(event.snapshot.best_bid_qty, Qty::new(5));
        assert_eq!(event.snapshot.best_ask_px, Some(Px9::new(101)));
        assert_eq!(event.snapshot.best_ask_qty, Qty::new(7));
        assert!(event.bid_depth.is_empty());
        assert!(event.ask_depth.is_empty());
    }

    #[test]
    fn f_last_gates_crossed_intermediate_book() {
        let mut assembler = EventAssembler::new();
        let first = assembler
            .ingest(record(1, MboAction::Add, MboSide::Bid, 1, 99, 3, F_LAST))
            .unwrap()
            .unwrap();
        assembler
            .ingest(record(2, MboAction::Add, MboSide::Ask, 2, 100, 5, F_LAST))
            .unwrap();

        let crossed_mid_event = record(3, MboAction::Add, MboSide::Bid, 3, 101, 1, 0);
        assert!(assembler.ingest(crossed_mid_event).unwrap().is_none());
        assert!(assembler
            .book_snapshot_for_diagnostics(&first.stream_key.instrument)
            .unwrap()
            .is_crossed());

        let completed = record(4, MboAction::Modify, MboSide::Ask, 2, 102, 5, F_LAST);
        let event = assembler
            .ingest(completed)
            .unwrap()
            .expect("F_LAST emits event");
        assert!(!event.snapshot.is_crossed());
        assert_eq!(event.snapshot.best_bid_px, Some(Px9::new(101)));
        assert_eq!(event.snapshot.best_ask_px, Some(Px9::new(102)));
    }

    #[test]
    fn reset_without_f_last_does_not_emit_snapshot() {
        let mut assembler = EventAssembler::new();
        let first = assembler
            .ingest(record(1, MboAction::Add, MboSide::Bid, 1, 100, 1, F_LAST))
            .unwrap()
            .unwrap();

        let reset = record(2, MboAction::Reset, MboSide::None, 0, 0, 0, 0);
        assert!(assembler.ingest(reset).unwrap().is_none());
        assert_eq!(
            assembler
                .book_snapshot_for_diagnostics(&first.stream_key.instrument)
                .unwrap()
                .order_count,
            0
        );

        let add_after_reset = record(3, MboAction::Add, MboSide::Bid, 3, 99, 2, F_LAST);
        let event = assembler
            .ingest(add_after_reset)
            .unwrap()
            .expect("next F_LAST emits reset batch");
        assert_eq!(event.records.len(), 2);
        assert_eq!(event.snapshot.order_count, 1);
        assert_eq!(event.snapshot.best_bid_px, Some(Px9::new(99)));
    }
}
