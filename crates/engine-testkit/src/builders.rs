pub use replay_core::mbo::{EngineMboRecord, MboAction, MboFlags, MboSide, F_LAST};
use replay_core::types::{
    ChannelId, InstrumentId, OrderId, PublisherId, Px9, Qty, RecordOrdinal, RecordType, Sequence,
    TsEvent, TsInDelta, TsRecv,
};

pub type SyntheticMboRecord = EngineMboRecord;

#[derive(Clone, Debug)]
pub struct MboBuilder {
    records: Vec<EngineMboRecord>,
    instrument_id: u32,
    flags: u8,
    ts_event: u64,
    ts_recv: u64,
    sequence: u32,
}

impl Default for MboBuilder {
    fn default() -> Self {
        Self {
            records: Vec::new(),
            instrument_id: 1,
            flags: F_LAST,
            ts_event: 1,
            ts_recv: 1,
            sequence: 1,
        }
    }
}

impl MboBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn instrument(mut self, instrument_id: u32) -> Self {
        self.instrument_id = instrument_id;
        self
    }

    pub fn flags(mut self, flags: u8) -> Self {
        self.flags = flags;
        self
    }

    pub fn timestamps(mut self, ts_event: u64, ts_recv: u64) -> Self {
        self.ts_event = ts_event;
        self.ts_recv = ts_recv;
        self
    }

    pub fn sequence(mut self, sequence: u32) -> Self {
        self.sequence = sequence;
        self
    }

    pub fn bid(mut self, order_id: u64, price_px9: i64, size: u32) -> Self {
        self.push(MboAction::Add, MboSide::Bid, order_id, price_px9, size);
        self
    }

    pub fn ask(mut self, order_id: u64, price_px9: i64, size: u32) -> Self {
        self.push(MboAction::Add, MboSide::Ask, order_id, price_px9, size);
        self
    }

    pub fn action(
        mut self,
        action: MboAction,
        side: MboSide,
        order_id: u64,
        price_px9: i64,
        size: u32,
    ) -> Self {
        self.push(action, side, order_id, price_px9, size);
        self
    }

    pub fn into_records(self) -> Vec<EngineMboRecord> {
        self.records
    }

    fn push(&mut self, action: MboAction, side: MboSide, order_id: u64, price_px9: i64, size: u32) {
        let ordinal = self.records.len() as u64;
        self.records.push(EngineMboRecord {
            ordinal: RecordOrdinal::new(ordinal),
            rtype: RecordType::new(0),
            publisher_id: PublisherId::new(0),
            instrument_id: InstrumentId::new(self.instrument_id),
            channel_id: ChannelId::new(0),
            action,
            side,
            price_px9: Px9::new(price_px9),
            size: Qty::new(size),
            order_id: OrderId::new(order_id),
            flags: MboFlags::new(self.flags),
            ts_event: TsEvent(self.ts_event),
            ts_recv: TsRecv(self.ts_recv),
            ts_in_delta: TsInDelta(0),
            sequence: Sequence(self.sequence),
        });
        self.sequence = self.sequence.saturating_add(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builder_assigns_ordinals_in_input_order() {
        let records = MboBuilder::new()
            .bid(10, 5_000_000_000_000, 3)
            .ask(11, 5_000_250_000_000, 2)
            .into_records();

        assert_eq!(records[0].ordinal.get(), 0);
        assert_eq!(records[1].ordinal.get(), 1);
        assert_eq!(records[0].action.code(), 'A');
        assert_eq!(records[0].side.code(), 'B');
    }

    #[test]
    fn builder_supports_non_mutating_trade_rows() {
        let records = MboBuilder::new()
            .action(MboAction::Trade, MboSide::Ask, 99, 101, 1)
            .into_records();

        assert_eq!(records[0].action, MboAction::Trade);
        assert_eq!(records[0].flags.raw(), F_LAST);
    }
}
