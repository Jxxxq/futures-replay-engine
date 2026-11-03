use std::ffi::c_char;

use dbn::MboMsg;

use crate::mbo::{EngineMboRecord, MboAction, MboFlags, MboSide};
use crate::types::{
    ChannelId, InstrumentId, OrderId, PublisherId, Px9, Qty, RecordOrdinal, RecordType, Sequence,
    TsEvent, TsInDelta, TsRecv,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DbnConversionError {
    UnknownAction(c_char),
    UnknownSide(c_char),
}

pub fn convert_mbo_msg(
    msg: &MboMsg,
    ordinal: RecordOrdinal,
) -> Result<EngineMboRecord, DbnConversionError> {
    Ok(EngineMboRecord {
        ordinal,
        rtype: RecordType::new(msg.hd.rtype),
        publisher_id: PublisherId::new(msg.hd.publisher_id),
        instrument_id: InstrumentId::new(msg.hd.instrument_id),
        channel_id: ChannelId::new(msg.channel_id),
        action: convert_action(msg.action)?,
        side: convert_side(msg.side)?,
        price_px9: Px9::new(msg.price),
        size: Qty::new(msg.size),
        order_id: OrderId::new(msg.order_id),
        flags: MboFlags::new(msg.flags.raw()),
        ts_event: TsEvent(msg.hd.ts_event),
        ts_recv: TsRecv(msg.ts_recv),
        ts_in_delta: TsInDelta(msg.ts_in_delta),
        sequence: Sequence(msg.sequence),
    })
}

pub fn convert_action(action: c_char) -> Result<MboAction, DbnConversionError> {
    match action as u8 {
        b'A' => Ok(MboAction::Add),
        b'C' => Ok(MboAction::Cancel),
        b'M' => Ok(MboAction::Modify),
        b'R' => Ok(MboAction::Reset),
        b'T' => Ok(MboAction::Trade),
        b'F' => Ok(MboAction::Fill),
        b'N' => Ok(MboAction::None),
        _ => Err(DbnConversionError::UnknownAction(action)),
    }
}

pub fn convert_side(side: c_char) -> Result<MboSide, DbnConversionError> {
    match side as u8 {
        b'B' => Ok(MboSide::Bid),
        b'A' => Ok(MboSide::Ask),
        b'N' => Ok(MboSide::None),
        _ => Err(DbnConversionError::UnknownSide(side)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dbn::{rtype, FlagSet, MboMsg, RecordHeader};

    #[test]
    fn converts_mbo_msg_without_losing_raw_context() {
        let msg = MboMsg {
            hd: RecordHeader::new::<MboMsg>(rtype::MBO, 42, 7, 100),
            order_id: 99,
            price: 5_000_250_000_000,
            size: 3,
            flags: FlagSet::new(crate::mbo::F_LAST),
            channel_id: 4,
            action: b'A' as c_char,
            side: b'B' as c_char,
            ts_recv: 105,
            ts_in_delta: 2,
            sequence: 88,
        };

        let record = convert_mbo_msg(&msg, RecordOrdinal::new(12)).unwrap();

        assert_eq!(record.ordinal.get(), 12);
        assert_eq!(record.rtype.get(), rtype::MBO);
        assert_eq!(record.publisher_id.get(), 42);
        assert_eq!(record.instrument_id.get(), 7);
        assert_eq!(record.channel_id.get(), 4);
        assert_eq!(record.action, MboAction::Add);
        assert_eq!(record.side, MboSide::Bid);
        assert_eq!(record.price_px9.get(), 5_000_250_000_000);
        assert_eq!(record.ts_event.0, 100);
        assert_eq!(record.ts_recv.0, 105);
        assert_eq!(record.ts_in_delta.0, 2);
        assert_eq!(record.sequence.0, 88);
        assert!(record.flags.is_last());
    }

    #[test]
    fn rejects_unknown_action_and_side() {
        assert_eq!(
            convert_action(b'X' as c_char),
            Err(DbnConversionError::UnknownAction(b'X' as c_char))
        );
        assert_eq!(
            convert_side(b'X' as c_char),
            Err(DbnConversionError::UnknownSide(b'X' as c_char))
        );
    }
}
