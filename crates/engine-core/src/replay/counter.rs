use crate::book::{BookError, DisplayedBook};
use crate::fast_hash::FastHashMap;
use crate::mbo::EngineMboRecord;
use crate::types::{ChannelId, InstrumentId, PublisherId};

type CounterStreamKey = (PublisherId, ChannelId, InstrumentId);

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ReplayCounterError {
    Book(BookError),
    IncompleteEventAtEnd {
        publisher_id: PublisherId,
        channel_id: ChannelId,
        instrument_id: InstrumentId,
        buffered_records: usize,
    },
}

impl From<BookError> for ReplayCounterError {
    fn from(value: BookError) -> Self {
        Self::Book(value)
    }
}

#[derive(Default)]
pub struct BookReplayCounter {
    books: FastHashMap<CounterStreamKey, DisplayedBook>,
    incomplete_counts: FastHashMap<CounterStreamKey, usize>,
}

impl BookReplayCounter {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn ingest(&mut self, record: EngineMboRecord) -> Result<bool, ReplayCounterError> {
        let instrument_id = record.instrument_id;
        let stream_key = (record.publisher_id, record.channel_id, instrument_id);
        if record.action.mutates_displayed_book() {
            self.books
                .entry(stream_key)
                .or_insert_with(|| DisplayedBook::new(instrument_id))
                .apply(&record)?;
        }

        let count = self.incomplete_counts.entry(stream_key).or_default();
        *count += 1;
        if record.is_last() {
            *count = 0;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    pub fn incomplete_record_count(&self, instrument_id: InstrumentId) -> usize {
        self.incomplete_counts
            .iter()
            .filter(|((_, _, current), _)| *current == instrument_id)
            .map(|(_, count)| *count)
            .sum()
    }

    pub fn finish_file(&self) -> Result<(), ReplayCounterError> {
        if let Some(((publisher_id, channel_id, instrument_id), buffered_records)) =
            self.incomplete_counts.iter().find(|(_, count)| **count > 0)
        {
            return Err(ReplayCounterError::IncompleteEventAtEnd {
                publisher_id: *publisher_id,
                channel_id: *channel_id,
                instrument_id: *instrument_id,
                buffered_records: *buffered_records,
            });
        }
        Ok(())
    }

    pub fn reset_for_next_file(&mut self) -> Result<(), ReplayCounterError> {
        self.finish_file()?;
        self.books.clear();
        self.incomplete_counts.clear();
        Ok(())
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
        EngineMboRecord {
            ordinal: RecordOrdinal::new(ordinal),
            rtype: RecordType::new(0),
            publisher_id: PublisherId::new(0),
            instrument_id: InstrumentId::new(1),
            channel_id: ChannelId::new(0),
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
    fn counts_completed_events_without_retaining_buffers() {
        let mut counter = BookReplayCounter::new();

        assert!(!counter
            .ingest(record(1, MboAction::Add, MboSide::Bid, 1, 100, 1, 0))
            .unwrap());
        assert_eq!(counter.incomplete_record_count(InstrumentId::new(1)), 1);
        assert!(counter
            .ingest(record(2, MboAction::Add, MboSide::Ask, 2, 101, 1, F_LAST))
            .unwrap());
        assert_eq!(counter.incomplete_record_count(InstrumentId::new(1)), 0);
    }

    #[test]
    fn skips_book_work_for_non_mutating_records() {
        let mut counter = BookReplayCounter::new();

        assert!(counter
            .ingest(record(1, MboAction::Trade, MboSide::None, 0, 0, 0, F_LAST))
            .unwrap());
        assert_eq!(counter.incomplete_record_count(InstrumentId::new(1)), 0);
    }

    #[test]
    fn finish_rejects_incomplete_events_and_file_reset_clears_books() {
        let mut counter = BookReplayCounter::new();
        counter
            .ingest(record(1, MboAction::Add, MboSide::Bid, 1, 100, 1, 0))
            .unwrap();
        assert!(matches!(
            counter.finish_file(),
            Err(ReplayCounterError::IncompleteEventAtEnd {
                buffered_records: 1,
                ..
            })
        ));

        let mut complete = BookReplayCounter::new();
        complete
            .ingest(record(1, MboAction::Add, MboSide::Bid, 1, 100, 1, F_LAST))
            .unwrap();
        complete.reset_for_next_file().unwrap();
        assert_eq!(complete.incomplete_record_count(InstrumentId::new(1)), 0);
    }
}
