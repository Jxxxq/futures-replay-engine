#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct Px9(pub i64);

impl Px9 {
    pub const UNDEF: Self = Self(i64::MAX);

    pub fn new(value: i64) -> Self {
        Self(value)
    }

    pub fn get(self) -> i64 {
        self.0
    }

    pub fn is_undefined(self) -> bool {
        self == Self::UNDEF
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct Qty(pub u32);

impl Qty {
    pub fn new(value: u32) -> Self {
        Self(value)
    }

    pub fn get(self) -> u32 {
        self.0
    }

    pub fn is_zero(self) -> bool {
        self.0 == 0
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct InstrumentId(pub u32);

impl InstrumentId {
    pub fn new(value: u32) -> Self {
        Self(value)
    }

    pub fn get(self) -> u32 {
        self.0
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct InstrumentKey {
    pub dataset: String,
    pub publisher_id: PublisherId,
    pub instrument_id: InstrumentId,
    pub symbol_epoch: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct SessionId(pub String);

impl SessionId {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct BookEpoch(pub u64);

impl BookEpoch {
    pub fn next(self) -> Self {
        Self(self.0.checked_add(1).expect("book epoch overflow"))
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct StreamKey {
    pub instrument: InstrumentKey,
    pub channel_id: ChannelId,
    pub session_id: SessionId,
    pub book_epoch: BookEpoch,
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct StreamIdentity {
    pub instrument: InstrumentKey,
    pub channel_id: ChannelId,
    pub session_id: SessionId,
}

impl StreamIdentity {
    pub fn new(instrument: InstrumentKey, channel_id: ChannelId, session_id: SessionId) -> Self {
        Self {
            instrument,
            channel_id,
            session_id,
        }
    }

    pub fn at_epoch(&self, book_epoch: BookEpoch) -> StreamKey {
        StreamKey::new(
            self.instrument.clone(),
            self.channel_id,
            self.session_id.clone(),
            book_epoch,
        )
    }
}

impl StreamKey {
    pub fn new(
        instrument: InstrumentKey,
        channel_id: ChannelId,
        session_id: SessionId,
        book_epoch: BookEpoch,
    ) -> Self {
        Self {
            instrument,
            channel_id,
            session_id,
            book_epoch,
        }
    }

    pub fn unscoped(
        publisher_id: PublisherId,
        instrument_id: InstrumentId,
        channel_id: ChannelId,
    ) -> Self {
        Self::new(
            InstrumentKey::new(
                "unscoped",
                publisher_id,
                instrument_id,
                format!("unresolved-{}", instrument_id.get()),
            ),
            channel_id,
            SessionId::new("unscoped"),
            BookEpoch::default(),
        )
    }
}

impl InstrumentKey {
    pub fn new(
        dataset: impl Into<String>,
        publisher_id: PublisherId,
        instrument_id: InstrumentId,
        symbol_epoch: impl Into<String>,
    ) -> Self {
        Self {
            dataset: dataset.into(),
            publisher_id,
            instrument_id,
            symbol_epoch: symbol_epoch.into(),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct RecordType(pub u8);

impl RecordType {
    pub fn new(value: u8) -> Self {
        Self(value)
    }

    pub fn get(self) -> u8 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct PublisherId(pub u16);

impl PublisherId {
    pub fn new(value: u16) -> Self {
        Self(value)
    }

    pub fn get(self) -> u16 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct ChannelId(pub u8);

impl ChannelId {
    pub fn new(value: u8) -> Self {
        Self(value)
    }

    pub fn get(self) -> u8 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct OrderId(pub u64);

impl OrderId {
    pub fn new(value: u64) -> Self {
        Self(value)
    }

    pub fn get(self) -> u64 {
        self.0
    }

    pub fn is_zero(self) -> bool {
        self.0 == 0
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct TsEvent(pub u64);

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct TsRecv(pub u64);

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct TsInDelta(pub i32);

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct RecordOrdinal(pub u64);

impl RecordOrdinal {
    pub fn new(value: u64) -> Self {
        Self(value)
    }

    pub fn get(self) -> u64 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct Sequence(pub u32);
