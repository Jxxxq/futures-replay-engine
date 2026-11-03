use crate::types::{
    ChannelId, InstrumentId, OrderId, PublisherId, Px9, Qty, RecordOrdinal, RecordType, Sequence,
    TsEvent, TsInDelta, TsRecv,
};

pub const F_LAST: u8 = 0x80;
pub const F_TOB: u8 = 0x40;
pub const F_SNAPSHOT: u8 = 0x20;
pub const F_MBP: u8 = 0x10;
pub const F_BAD_TS_RECV: u8 = 0x08;
pub const F_MAYBE_BAD_BOOK: u8 = 0x04;
pub const F_PUBLISHER_SPECIFIC: u8 = 0x02;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MboAction {
    Add,
    Cancel,
    Modify,
    Reset,
    Trade,
    Fill,
    None,
}

impl MboAction {
    pub fn code(self) -> char {
        match self {
            Self::Add => 'A',
            Self::Cancel => 'C',
            Self::Modify => 'M',
            Self::Reset => 'R',
            Self::Trade => 'T',
            Self::Fill => 'F',
            Self::None => 'N',
        }
    }

    pub fn mutates_displayed_book(self) -> bool {
        matches!(self, Self::Add | Self::Cancel | Self::Modify | Self::Reset)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MboSide {
    Bid,
    Ask,
    None,
}

impl MboSide {
    pub fn code(self) -> char {
        match self {
            Self::Bid => 'B',
            Self::Ask => 'A',
            Self::None => 'N',
        }
    }

    pub fn is_book_side(self) -> bool {
        matches!(self, Self::Bid | Self::Ask)
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct MboFlags(pub u8);

impl MboFlags {
    pub fn new(raw: u8) -> Self {
        Self(raw)
    }

    pub fn raw(self) -> u8 {
        self.0
    }

    pub fn is_last(self) -> bool {
        self.0 & F_LAST != 0
    }

    pub fn maybe_bad_book(self) -> bool {
        self.0 & F_MAYBE_BAD_BOOK != 0
    }

    pub fn bad_ts_recv(self) -> bool {
        self.0 & F_BAD_TS_RECV != 0
    }

    pub fn is_snapshot(self) -> bool {
        self.0 & F_SNAPSHOT != 0
    }

    pub fn is_publisher_specific(self) -> bool {
        self.0 & F_PUBLISHER_SPECIFIC != 0
    }

    pub fn is_tob(self) -> bool {
        self.0 & F_TOB != 0
    }

    pub fn is_mbp(self) -> bool {
        self.0 & F_MBP != 0
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EngineMboRecord {
    pub ordinal: RecordOrdinal,
    pub rtype: RecordType,
    pub publisher_id: PublisherId,
    pub instrument_id: InstrumentId,
    pub channel_id: ChannelId,
    pub action: MboAction,
    pub side: MboSide,
    pub price_px9: Px9,
    pub size: Qty,
    pub order_id: OrderId,
    pub flags: MboFlags,
    pub ts_event: TsEvent,
    pub ts_recv: TsRecv,
    pub ts_in_delta: TsInDelta,
    pub sequence: Sequence,
}

impl EngineMboRecord {
    pub fn is_last(&self) -> bool {
        self.flags.is_last()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_acmr_mutate_displayed_book() {
        assert!(MboAction::Add.mutates_displayed_book());
        assert!(MboAction::Cancel.mutates_displayed_book());
        assert!(MboAction::Modify.mutates_displayed_book());
        assert!(MboAction::Reset.mutates_displayed_book());
        assert!(!MboAction::Trade.mutates_displayed_book());
        assert!(!MboAction::Fill.mutates_displayed_book());
        assert!(!MboAction::None.mutates_displayed_book());
    }

    #[test]
    fn flags_expose_last_gate() {
        assert!(MboFlags::new(F_LAST).is_last());
        assert!(!MboFlags::new(0).is_last());
    }

    #[test]
    fn flags_expose_quality_and_context_bits() {
        assert!(MboFlags::new(F_SNAPSHOT).is_snapshot());
        assert!(MboFlags::new(F_BAD_TS_RECV).bad_ts_recv());
        assert!(MboFlags::new(F_MAYBE_BAD_BOOK).maybe_bad_book());
        assert!(MboFlags::new(F_PUBLISHER_SPECIFIC).is_publisher_specific());
        assert!(MboFlags::new(F_TOB).is_tob());
        assert!(MboFlags::new(F_MBP).is_mbp());
    }
}
