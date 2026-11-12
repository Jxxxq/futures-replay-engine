use std::collections::{btree_map, hash_map, BTreeMap};

use crate::fast_hash::FastHashMap;
use crate::mbo::{EngineMboRecord, MboAction, MboSide};
use crate::types::{InstrumentId, OrderId, Px9, Qty, Sequence};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BookMutation {
    Added,
    Cancelled,
    Modified { priority_reset: bool },
    Reset,
    NoOp,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum BookError {
    WrongInstrument {
        expected: InstrumentId,
        actual: InstrumentId,
    },
    InvalidBookSide(MboSide),
    MissingOrder(OrderId),
    DuplicateOrder(OrderId),
    ZeroOrderId,
    ZeroSize,
    UndefinedPrice,
    CancelSideMismatch {
        order_id: OrderId,
        expected: MboSide,
        actual: MboSide,
    },
    CancelPriceMismatch {
        order_id: OrderId,
        expected: Px9,
        actual: Px9,
    },
    CancelExceedsResting {
        order_id: OrderId,
        resting: Qty,
        cancel: Qty,
    },
    ModifySideChange {
        order_id: OrderId,
        expected: MboSide,
        actual: MboSide,
    },
    LevelQuantityOverflow {
        side: MboSide,
        price: Px9,
    },
    LevelQuantityUnderflow {
        side: MboSide,
        price: Px9,
        resting: Qty,
        remove: Qty,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RestingOrder {
    pub order_id: OrderId,
    pub side: MboSide,
    pub price_px9: Px9,
    pub size: Qty,
    pub priority_sequence: Sequence,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BookSnapshot {
    pub instrument_id: InstrumentId,
    pub best_bid_px: Option<Px9>,
    pub best_bid_qty: Qty,
    pub best_ask_px: Option<Px9>,
    pub best_ask_qty: Qty,
    pub order_count: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BookLevel {
    pub price_px9: Px9,
    pub qty: Qty,
}

impl BookSnapshot {
    pub fn is_crossed(self) -> bool {
        matches!(
            (self.best_bid_px, self.best_ask_px),
            (Some(bid), Some(ask)) if bid >= ask
        )
    }
}

/// one price level: the total size resting there and how many orders that is.
/// kept together so a message only has to find its price once.
#[derive(Clone, Copy, Debug)]
struct Level {
    qty: Qty,
    order_count: usize,
}

type Levels = BTreeMap<Px9, Level>;

#[derive(Clone, Debug)]
pub struct DisplayedBook {
    instrument_id: InstrumentId,
    orders: FastHashMap<OrderId, RestingOrder>,
    bids: Levels,
    asks: Levels,
}

impl DisplayedBook {
    pub fn new(instrument_id: InstrumentId) -> Self {
        Self {
            instrument_id,
            orders: FastHashMap::default(),
            bids: BTreeMap::new(),
            asks: BTreeMap::new(),
        }
    }

    pub fn instrument_id(&self) -> InstrumentId {
        self.instrument_id
    }

    pub fn apply(&mut self, record: &EngineMboRecord) -> Result<BookMutation, BookError> {
        if record.instrument_id != self.instrument_id {
            return Err(BookError::WrongInstrument {
                expected: self.instrument_id,
                actual: record.instrument_id,
            });
        }

        match record.action {
            MboAction::Add => self.add_order(record),
            MboAction::Cancel => self.cancel_order(record),
            MboAction::Modify => self.modify_order(record),
            MboAction::Reset => {
                self.clear();
                Ok(BookMutation::Reset)
            }
            MboAction::Trade | MboAction::Fill | MboAction::None => Ok(BookMutation::NoOp),
        }
    }

    pub fn snapshot(&self) -> BookSnapshot {
        let (best_bid_px, best_bid_qty) = self
            .bids
            .last_key_value()
            .map(|(price, level)| (Some(*price), level.qty))
            .unwrap_or((None, Qty::new(0)));
        let (best_ask_px, best_ask_qty) = self
            .asks
            .first_key_value()
            .map(|(price, level)| (Some(*price), level.qty))
            .unwrap_or((None, Qty::new(0)));

        BookSnapshot {
            instrument_id: self.instrument_id,
            best_bid_px,
            best_bid_qty,
            best_ask_px,
            best_ask_qty,
            order_count: self.orders.len(),
        }
    }

    pub fn order(&self, order_id: OrderId) -> Option<&RestingOrder> {
        self.orders.get(&order_id)
    }

    /// how many separate orders are resting at this exact price. it only reads
    /// the book, so it has no effect on queue position, fills, or updates.
    pub fn order_count_at_price(&self, side: MboSide, price: Px9) -> usize {
        match side {
            MboSide::Bid => self.bids.get(&price),
            MboSide::Ask => self.asks.get(&price),
            MboSide::None => None,
        }
        .map_or(0, |level| level.order_count)
    }

    pub fn depth(&self, side: MboSide) -> Vec<BookLevel> {
        self.depth_limited(side, usize::MAX)
    }

    pub fn depth_limited(&self, side: MboSide, max_levels: usize) -> Vec<BookLevel> {
        let mut levels = Vec::with_capacity(max_levels.min(match side {
            MboSide::Bid => self.bids.len(),
            MboSide::Ask => self.asks.len(),
            MboSide::None => 0,
        }));
        match side {
            MboSide::Bid => {
                for (price, level) in self.bids.iter().rev().take(max_levels) {
                    levels.push(BookLevel {
                        price_px9: *price,
                        qty: level.qty,
                    });
                }
            }
            MboSide::Ask => {
                for (price, level) in self.asks.iter().take(max_levels) {
                    levels.push(BookLevel {
                        price_px9: *price,
                        qty: level.qty,
                    });
                }
            }
            MboSide::None => {}
        }
        levels
    }

    pub fn qty_at_price_limited(&self, side: MboSide, price: Px9, max_levels: usize) -> Qty {
        match side {
            MboSide::Bid => self
                .bids
                .iter()
                .rev()
                .take(max_levels)
                .find_map(|(level_price, level)| (*level_price == price).then_some(level.qty))
                .unwrap_or_else(|| Qty::new(0)),
            MboSide::Ask => self
                .asks
                .iter()
                .take(max_levels)
                .find_map(|(level_price, level)| (*level_price == price).then_some(level.qty))
                .unwrap_or_else(|| Qty::new(0)),
            MboSide::None => Qty::new(0),
        }
    }

    pub fn depth_limited_array<const N: usize>(&self, side: MboSide) -> ([BookLevel; N], usize) {
        let empty = BookLevel {
            price_px9: Px9::default(),
            qty: Qty::default(),
        };
        let mut levels = [empty; N];
        let mut len = 0;

        match side {
            MboSide::Bid => {
                for (price, level) in self.bids.iter().rev().take(N) {
                    levels[len] = BookLevel {
                        price_px9: *price,
                        qty: level.qty,
                    };
                    len += 1;
                }
            }
            MboSide::Ask => {
                for (price, level) in self.asks.iter().take(N) {
                    levels[len] = BookLevel {
                        price_px9: *price,
                        qty: level.qty,
                    };
                    len += 1;
                }
            }
            MboSide::None => {}
        }

        (levels, len)
    }

    pub fn clear(&mut self) {
        self.orders.clear();
        self.bids.clear();
        self.asks.clear();
    }

    fn add_order(&mut self, record: &EngineMboRecord) -> Result<BookMutation, BookError> {
        Self::validate_resting_record(record)?;
        let hash_map::Entry::Vacant(order_slot) = self.orders.entry(record.order_id) else {
            return Err(BookError::DuplicateOrder(record.order_id));
        };

        match side_levels(&mut self.bids, &mut self.asks, record.side).entry(record.price_px9) {
            btree_map::Entry::Occupied(mut level_slot) => {
                let level = level_slot.get_mut();
                level.qty = level_add(level.qty, record.size, record.side, record.price_px9)?;
                level.order_count = level
                    .order_count
                    .checked_add(1)
                    .expect("a price-level order count cannot exceed the total order map capacity");
            }
            btree_map::Entry::Vacant(level_slot) => {
                level_slot.insert(Level {
                    qty: record.size,
                    order_count: 1,
                });
            }
        }
        order_slot.insert(RestingOrder {
            order_id: record.order_id,
            side: record.side,
            price_px9: record.price_px9,
            size: record.size,
            priority_sequence: record.sequence,
        });
        Ok(BookMutation::Added)
    }

    fn cancel_order(&mut self, record: &EngineMboRecord) -> Result<BookMutation, BookError> {
        let hash_map::Entry::Occupied(mut order_slot) = self.orders.entry(record.order_id) else {
            return Err(BookError::MissingOrder(record.order_id));
        };
        let order = order_slot.get();
        let (side, price, resting) = (order.side, order.price_px9, order.size);

        let cancel_qty = record.size;
        if cancel_qty.is_zero() {
            return Err(BookError::ZeroSize);
        }
        if record.side != side {
            return Err(BookError::CancelSideMismatch {
                order_id: record.order_id,
                expected: side,
                actual: record.side,
            });
        }
        if record.price_px9 != price {
            return Err(BookError::CancelPriceMismatch {
                order_id: record.order_id,
                expected: price,
                actual: record.price_px9,
            });
        }
        if cancel_qty > resting {
            return Err(BookError::CancelExceedsResting {
                order_id: record.order_id,
                resting,
                cancel: cancel_qty,
            });
        }

        let btree_map::Entry::Occupied(mut level_slot) =
            side_levels(&mut self.bids, &mut self.asks, side).entry(price)
        else {
            return Err(missing_level(side, price, cancel_qty));
        };
        let level = level_slot.get_mut();
        level.qty = level_sub(level.qty, cancel_qty, side, price)?;

        let remaining = resting.get() - cancel_qty.get();
        if remaining > 0 {
            order_slot.get_mut().size = Qty::new(remaining);
        } else {
            order_slot.remove();
            level.order_count = level
                .order_count
                .checked_sub(1)
                .expect("resting price-level order count cannot underflow");
            if level.order_count == 0 {
                level_slot.remove();
            }
        }

        Ok(BookMutation::Cancelled)
    }

    fn modify_order(&mut self, record: &EngineMboRecord) -> Result<BookMutation, BookError> {
        Self::validate_resting_record(record)?;
        let order = self
            .orders
            .get_mut(&record.order_id)
            .ok_or(BookError::MissingOrder(record.order_id))?;
        if record.side != order.side {
            return Err(BookError::ModifySideChange {
                order_id: record.order_id,
                expected: order.side,
                actual: record.side,
            });
        }

        let side = order.side;
        let levels = side_levels(&mut self.bids, &mut self.asks, side);
        if record.price_px9 == order.price_px9 {
            let price = order.price_px9;
            let level = levels
                .get_mut(&price)
                .ok_or(missing_level(side, price, order.size))?;
            let after_remove = level_sub(level.qty, order.size, side, price)?;
            level.qty = level_add(after_remove, record.size, side, price)?;
        } else {
            // check both levels before touching either one, so a rejected
            // modify doesn't leave the book half changed.
            let new_level_qty = levels
                .get(&record.price_px9)
                .map_or(Qty::new(0), |level| level.qty);
            let btree_map::Entry::Occupied(mut old_slot) = levels.entry(order.price_px9) else {
                return Err(missing_level(side, order.price_px9, order.size));
            };
            let after_remove = level_sub(old_slot.get().qty, order.size, side, order.price_px9)?;
            let after_add = level_add(new_level_qty, record.size, side, record.price_px9)?;

            let old_level = old_slot.get_mut();
            old_level.qty = after_remove;
            old_level.order_count = old_level
                .order_count
                .checked_sub(1)
                .expect("resting price-level order count cannot underflow");
            if old_level.order_count == 0 {
                old_slot.remove();
            }

            match levels.entry(record.price_px9) {
                btree_map::Entry::Occupied(mut new_slot) => {
                    let new_level = new_slot.get_mut();
                    new_level.qty = after_add;
                    new_level.order_count = new_level.order_count.checked_add(1).expect(
                        "a price-level order count cannot exceed the total order map capacity",
                    );
                }
                btree_map::Entry::Vacant(new_slot) => {
                    new_slot.insert(Level {
                        qty: after_add,
                        order_count: 1,
                    });
                }
            }
        }

        let priority_reset =
            record.price_px9 != order.price_px9 || record.size.get() > order.size.get();
        order.price_px9 = record.price_px9;
        order.size = record.size;
        if priority_reset {
            order.priority_sequence = record.sequence;
        }
        Ok(BookMutation::Modified { priority_reset })
    }

    fn validate_resting_record(record: &EngineMboRecord) -> Result<(), BookError> {
        if record.order_id.is_zero() {
            return Err(BookError::ZeroOrderId);
        }
        if !record.side.is_book_side() {
            return Err(BookError::InvalidBookSide(record.side));
        }
        if record.size.is_zero() {
            return Err(BookError::ZeroSize);
        }
        if record.price_px9.is_undefined() {
            return Err(BookError::UndefinedPrice);
        }
        Ok(())
    }
}

fn side_levels<'a>(bids: &'a mut Levels, asks: &'a mut Levels, side: MboSide) -> &'a mut Levels {
    match side {
        MboSide::Bid => bids,
        MboSide::Ask => asks,
        MboSide::None => unreachable!("validated resting records must use book sides"),
    }
}

fn level_add(current: Qty, qty: Qty, side: MboSide, price: Px9) -> Result<Qty, BookError> {
    current
        .get()
        .checked_add(qty.get())
        .map(Qty::new)
        .ok_or(BookError::LevelQuantityOverflow { side, price })
}

fn level_sub(current: Qty, qty: Qty, side: MboSide, price: Px9) -> Result<Qty, BookError> {
    current
        .get()
        .checked_sub(qty.get())
        .map(Qty::new)
        .ok_or(BookError::LevelQuantityUnderflow {
            side,
            price,
            resting: current,
            remove: qty,
        })
}

/// nothing is resting at this price, so there's no size to take off it.
fn missing_level(side: MboSide, price: Px9, remove: Qty) -> BookError {
    BookError::LevelQuantityUnderflow {
        side,
        price,
        resting: Qty::new(0),
        remove,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mbo::{MboFlags, F_LAST};
    use crate::types::{
        ChannelId, PublisherId, RecordOrdinal, RecordType, TsEvent, TsInDelta, TsRecv,
    };

    fn record(
        action: MboAction,
        side: MboSide,
        order_id: u64,
        price: i64,
        size: u32,
    ) -> EngineMboRecord {
        EngineMboRecord {
            ordinal: RecordOrdinal::new(0),
            rtype: RecordType::new(0),
            publisher_id: PublisherId::new(0),
            instrument_id: InstrumentId::new(1),
            channel_id: ChannelId::new(0),
            action,
            side,
            price_px9: Px9::new(price),
            size: Qty::new(size),
            order_id: OrderId::new(order_id),
            flags: MboFlags::new(F_LAST),
            ts_event: TsEvent(1),
            ts_recv: TsRecv(1),
            ts_in_delta: TsInDelta(0),
            sequence: Sequence(1),
        }
    }

    #[test]
    fn actions_mutate_only_expected_state() {
        let mut book = DisplayedBook::new(InstrumentId::new(1));

        book.apply(&record(MboAction::Add, MboSide::Bid, 1, 100, 5))
            .unwrap();
        book.apply(&record(MboAction::Add, MboSide::Ask, 2, 101, 7))
            .unwrap();
        assert_eq!(book.snapshot().best_bid_qty, Qty::new(5));
        assert_eq!(book.snapshot().best_ask_qty, Qty::new(7));

        assert_eq!(
            book.apply(&record(MboAction::Trade, MboSide::Ask, 2, 101, 2))
                .unwrap(),
            BookMutation::NoOp
        );
        assert_eq!(
            book.apply(&record(MboAction::Fill, MboSide::Ask, 2, 101, 2))
                .unwrap(),
            BookMutation::NoOp
        );
        assert_eq!(book.snapshot().best_ask_qty, Qty::new(7));

        book.apply(&record(MboAction::Cancel, MboSide::Ask, 2, 101, 2))
            .unwrap();
        assert_eq!(book.snapshot().best_ask_qty, Qty::new(5));
    }

    #[test]
    fn reset_clears_book() {
        let mut book = DisplayedBook::new(InstrumentId::new(1));
        book.apply(&record(MboAction::Add, MboSide::Bid, 1, 100, 1))
            .unwrap();
        book.apply(&record(MboAction::Add, MboSide::Ask, 2, 101, 1))
            .unwrap();

        assert_eq!(
            book.apply(&record(MboAction::Reset, MboSide::None, 0, 0, 0))
                .unwrap(),
            BookMutation::Reset
        );
        assert_eq!(book.snapshot().order_count, 0);
        assert_eq!(book.snapshot().best_bid_px, None);
        assert_eq!(book.snapshot().best_ask_px, None);
    }

    #[test]
    fn exact_price_order_count_tracks_distinct_resting_orders_read_only() {
        let mut book = DisplayedBook::new(InstrumentId::new(1));
        book.apply(&record(MboAction::Add, MboSide::Ask, 1, 101, 2))
            .unwrap();
        book.apply(&record(MboAction::Add, MboSide::Ask, 2, 101, 3))
            .unwrap();
        book.apply(&record(MboAction::Add, MboSide::Ask, 3, 102, 4))
            .unwrap();
        let before = book.snapshot();

        assert_eq!(book.order_count_at_price(MboSide::Ask, Px9::new(101)), 2);
        assert_eq!(book.order_count_at_price(MboSide::Ask, Px9::new(102)), 1);
        assert_eq!(book.order_count_at_price(MboSide::Bid, Px9::new(101)), 0);
        assert_eq!(book.order_count_at_price(MboSide::None, Px9::new(101)), 0);
        assert_eq!(book.snapshot(), before);

        book.apply(&record(MboAction::Cancel, MboSide::Ask, 1, 101, 2))
            .unwrap();
        assert_eq!(book.order_count_at_price(MboSide::Ask, Px9::new(101)), 1);
        book.apply(&record(MboAction::Modify, MboSide::Ask, 2, 103, 3))
            .unwrap();
        assert_eq!(book.order_count_at_price(MboSide::Ask, Px9::new(101)), 0);
        assert_eq!(book.order_count_at_price(MboSide::Ask, Px9::new(103)), 1);
    }

    #[test]
    fn modify_tracks_priority_resets() {
        let mut book = DisplayedBook::new(InstrumentId::new(1));
        let mut add = record(MboAction::Add, MboSide::Bid, 1, 100, 5);
        add.sequence = Sequence(1);
        book.apply(&add).unwrap();

        let mut decrease = record(MboAction::Modify, MboSide::Bid, 1, 100, 3);
        decrease.sequence = Sequence(2);
        assert_eq!(
            book.apply(&decrease).unwrap(),
            BookMutation::Modified {
                priority_reset: false
            }
        );
        assert_eq!(
            book.order(OrderId::new(1)).unwrap().priority_sequence,
            Sequence(1)
        );

        let mut increase = record(MboAction::Modify, MboSide::Bid, 1, 100, 6);
        increase.sequence = Sequence(3);
        assert_eq!(
            book.apply(&increase).unwrap(),
            BookMutation::Modified {
                priority_reset: true
            }
        );
        assert_eq!(
            book.order(OrderId::new(1)).unwrap().priority_sequence,
            Sequence(3)
        );
    }

    #[test]
    fn modify_side_change_is_rejected_without_mutating_book() {
        let mut book = DisplayedBook::new(InstrumentId::new(1));
        let mut add = record(MboAction::Add, MboSide::Bid, 1, 100, 5);
        add.sequence = Sequence(1);
        book.apply(&add).unwrap();

        let mut side_change = record(MboAction::Modify, MboSide::Ask, 1, 100, 5);
        side_change.sequence = Sequence(2);
        let before = book.snapshot();
        assert_eq!(
            book.apply(&side_change),
            Err(BookError::ModifySideChange {
                order_id: OrderId::new(1),
                expected: MboSide::Bid,
                actual: MboSide::Ask,
            })
        );
        assert_eq!(book.snapshot(), before);
        let order = book.order(OrderId::new(1)).unwrap();
        assert_eq!(order.side, MboSide::Bid);
        assert_eq!(order.priority_sequence, Sequence(1));
    }

    #[test]
    fn invalid_cancels_are_transactional() {
        let mut book = DisplayedBook::new(InstrumentId::new(1));
        book.apply(&record(MboAction::Add, MboSide::Bid, 1, 100, 5))
            .unwrap();
        let before = book.snapshot();

        assert_eq!(
            book.apply(&record(MboAction::Cancel, MboSide::Bid, 1, 100, 0)),
            Err(BookError::ZeroSize)
        );
        assert_eq!(book.snapshot(), before);
        assert_eq!(book.order(OrderId::new(1)).unwrap().size, Qty::new(5));

        assert!(matches!(
            book.apply(&record(MboAction::Cancel, MboSide::Bid, 1, 100, 6)),
            Err(BookError::CancelExceedsResting { .. })
        ));
        assert_eq!(book.snapshot(), before);

        assert!(matches!(
            book.apply(&record(MboAction::Cancel, MboSide::Ask, 1, 100, 1)),
            Err(BookError::CancelSideMismatch { .. })
        ));
        assert!(matches!(
            book.apply(&record(MboAction::Cancel, MboSide::Bid, 1, 101, 1)),
            Err(BookError::CancelPriceMismatch { .. })
        ));
        assert_eq!(book.snapshot(), before);
    }

    #[test]
    fn unknown_orders_are_rejected_without_changing_the_book() {
        let mut book = DisplayedBook::new(InstrumentId::new(1));
        book.apply(&record(MboAction::Add, MboSide::Bid, 1, 100, 5))
            .unwrap();
        let before = book.snapshot();

        assert_eq!(
            book.apply(&record(MboAction::Cancel, MboSide::Bid, 2, 100, 1)),
            Err(BookError::MissingOrder(OrderId::new(2)))
        );
        assert_eq!(
            book.apply(&record(MboAction::Modify, MboSide::Bid, 2, 100, 1)),
            Err(BookError::MissingOrder(OrderId::new(2)))
        );
        assert_eq!(book.snapshot(), before);
    }
}
