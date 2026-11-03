use std::collections::BTreeMap;

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

#[derive(Clone, Debug)]
pub struct DisplayedBook {
    instrument_id: InstrumentId,
    orders: FastHashMap<OrderId, RestingOrder>,
    bids: BTreeMap<Px9, Qty>,
    asks: BTreeMap<Px9, Qty>,
    bid_order_counts: BTreeMap<Px9, usize>,
    ask_order_counts: BTreeMap<Px9, usize>,
}

impl DisplayedBook {
    pub fn new(instrument_id: InstrumentId) -> Self {
        Self {
            instrument_id,
            orders: FastHashMap::default(),
            bids: BTreeMap::new(),
            asks: BTreeMap::new(),
            bid_order_counts: BTreeMap::new(),
            ask_order_counts: BTreeMap::new(),
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
            .map(|(price, qty)| (Some(*price), *qty))
            .unwrap_or((None, Qty::new(0)));
        let (best_ask_px, best_ask_qty) = self
            .asks
            .first_key_value()
            .map(|(price, qty)| (Some(*price), *qty))
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
            MboSide::Bid => self.bid_order_counts.get(&price).copied(),
            MboSide::Ask => self.ask_order_counts.get(&price).copied(),
            MboSide::None => None,
        }
        .unwrap_or(0)
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
                for (price, qty) in self.bids.iter().rev().take(max_levels) {
                    levels.push(BookLevel {
                        price_px9: *price,
                        qty: *qty,
                    });
                }
            }
            MboSide::Ask => {
                for (price, qty) in self.asks.iter().take(max_levels) {
                    levels.push(BookLevel {
                        price_px9: *price,
                        qty: *qty,
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
                .find_map(|(level_price, qty)| (*level_price == price).then_some(*qty))
                .unwrap_or_else(|| Qty::new(0)),
            MboSide::Ask => self
                .asks
                .iter()
                .take(max_levels)
                .find_map(|(level_price, qty)| (*level_price == price).then_some(*qty))
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
                for (price, qty) in self.bids.iter().rev().take(N) {
                    levels[len] = BookLevel {
                        price_px9: *price,
                        qty: *qty,
                    };
                    len += 1;
                }
            }
            MboSide::Ask => {
                for (price, qty) in self.asks.iter().take(N) {
                    levels[len] = BookLevel {
                        price_px9: *price,
                        qty: *qty,
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
        self.bid_order_counts.clear();
        self.ask_order_counts.clear();
    }

    fn add_order(&mut self, record: &EngineMboRecord) -> Result<BookMutation, BookError> {
        Self::validate_resting_record(record)?;
        if self.orders.contains_key(&record.order_id) {
            return Err(BookError::DuplicateOrder(record.order_id));
        }

        let next_level_qty = self.checked_level_add(record.side, record.price_px9, record.size)?;
        let order = RestingOrder {
            order_id: record.order_id,
            side: record.side,
            price_px9: record.price_px9,
            size: record.size,
            priority_sequence: record.sequence,
        };
        self.set_level_qty(order.side, order.price_px9, next_level_qty);
        self.increment_level_order_count(order.side, order.price_px9);
        self.orders.insert(order.order_id, order);
        Ok(BookMutation::Added)
    }

    fn cancel_order(&mut self, record: &EngineMboRecord) -> Result<BookMutation, BookError> {
        let order = self
            .orders
            .get(&record.order_id)
            .cloned()
            .ok_or(BookError::MissingOrder(record.order_id))?;

        let cancel_qty = record.size;
        if cancel_qty.is_zero() {
            return Err(BookError::ZeroSize);
        }
        if record.side != order.side {
            return Err(BookError::CancelSideMismatch {
                order_id: record.order_id,
                expected: order.side,
                actual: record.side,
            });
        }
        if record.price_px9 != order.price_px9 {
            return Err(BookError::CancelPriceMismatch {
                order_id: record.order_id,
                expected: order.price_px9,
                actual: record.price_px9,
            });
        }
        if cancel_qty > order.size {
            return Err(BookError::CancelExceedsResting {
                order_id: record.order_id,
                resting: order.size,
                cancel: cancel_qty,
            });
        }

        let next_level_qty = self.checked_level_sub(order.side, order.price_px9, cancel_qty)?;
        let remaining = order.size.get() - cancel_qty.get();
        self.set_level_qty(order.side, order.price_px9, next_level_qty);
        if remaining > 0 {
            self.orders
                .get_mut(&record.order_id)
                .expect("validated order remains present")
                .size = Qty::new(remaining);
        } else {
            self.orders.remove(&record.order_id);
            self.decrement_level_order_count(order.side, order.price_px9);
        }

        Ok(BookMutation::Cancelled)
    }

    fn modify_order(&mut self, record: &EngineMboRecord) -> Result<BookMutation, BookError> {
        Self::validate_resting_record(record)?;
        let order = self
            .orders
            .get(&record.order_id)
            .cloned()
            .ok_or(BookError::MissingOrder(record.order_id))?;
        if record.side != order.side {
            return Err(BookError::ModifySideChange {
                order_id: record.order_id,
                expected: order.side,
                actual: record.side,
            });
        }

        let (old_level_qty, new_level_qty) = if record.price_px9 == order.price_px9 {
            let after_remove = self.checked_level_sub(order.side, order.price_px9, order.size)?;
            let after_add = after_remove.get().checked_add(record.size.get()).ok_or(
                BookError::LevelQuantityOverflow {
                    side: order.side,
                    price: order.price_px9,
                },
            )?;
            (None, Qty::new(after_add))
        } else {
            (
                Some(self.checked_level_sub(order.side, order.price_px9, order.size)?),
                self.checked_level_add(record.side, record.price_px9, record.size)?,
            )
        };

        if let Some(qty) = old_level_qty {
            self.set_level_qty(order.side, order.price_px9, qty);
            self.decrement_level_order_count(order.side, order.price_px9);
            self.increment_level_order_count(record.side, record.price_px9);
        }
        self.set_level_qty(record.side, record.price_px9, new_level_qty);

        let priority_reset =
            record.price_px9 != order.price_px9 || record.size.get() > order.size.get();
        let order = self
            .orders
            .get_mut(&record.order_id)
            .expect("validated order remains present");
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

    fn checked_level_add(&self, side: MboSide, price: Px9, qty: Qty) -> Result<Qty, BookError> {
        let current = self.level_qty(side, price);
        current
            .get()
            .checked_add(qty.get())
            .map(Qty::new)
            .ok_or(BookError::LevelQuantityOverflow { side, price })
    }

    fn checked_level_sub(&self, side: MboSide, price: Px9, qty: Qty) -> Result<Qty, BookError> {
        let current = self.level_qty(side, price);
        current.get().checked_sub(qty.get()).map(Qty::new).ok_or(
            BookError::LevelQuantityUnderflow {
                side,
                price,
                resting: current,
                remove: qty,
            },
        )
    }

    fn level_qty(&self, side: MboSide, price: Px9) -> Qty {
        match side {
            MboSide::Bid => self.bids.get(&price).copied(),
            MboSide::Ask => self.asks.get(&price).copied(),
            MboSide::None => None,
        }
        .unwrap_or_default()
    }

    fn set_level_qty(&mut self, side: MboSide, price: Px9, qty: Qty) {
        if qty.is_zero() {
            self.levels_mut(side).remove(&price);
        } else {
            self.levels_mut(side).insert(price, qty);
        }
    }

    fn levels_mut(&mut self, side: MboSide) -> &mut BTreeMap<Px9, Qty> {
        match side {
            MboSide::Bid => &mut self.bids,
            MboSide::Ask => &mut self.asks,
            MboSide::None => unreachable!("validated resting records must use book sides"),
        }
    }

    fn increment_level_order_count(&mut self, side: MboSide, price: Px9) {
        let count = self.order_counts_mut(side).entry(price).or_default();
        *count = count
            .checked_add(1)
            .expect("a price-level order count cannot exceed the total order map capacity");
    }

    fn decrement_level_order_count(&mut self, side: MboSide, price: Px9) {
        let counts = self.order_counts_mut(side);
        let count = counts
            .get_mut(&price)
            .expect("quantity and order-count indexes must contain the same resting level");
        *count = count
            .checked_sub(1)
            .expect("resting price-level order count cannot underflow");
        if *count == 0 {
            counts.remove(&price);
        }
    }

    fn order_counts_mut(&mut self, side: MboSide) -> &mut BTreeMap<Px9, usize> {
        match side {
            MboSide::Bid => &mut self.bid_order_counts,
            MboSide::Ask => &mut self.ask_order_counts,
            MboSide::None => unreachable!("validated resting records must use book sides"),
        }
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
