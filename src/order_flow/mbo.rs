pub const FIXED_PRICE_SCALE: i64 = 1_000_000_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct MboPrice(i64);

impl MboPrice {
    pub const UNDEFINED: Self = Self(i64::MAX);

    pub const fn from_fixed(value: i64) -> Self {
        Self(value)
    }

    pub const fn fixed(self) -> i64 {
        self.0
    }

    pub const fn is_defined(self) -> bool {
        self.0 != Self::UNDEFINED.0
    }

    pub fn as_f64(self) -> Option<f64> {
        self.is_defined()
            .then_some(self.0 as f64 / FIXED_PRICE_SCALE as f64)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MboAction {
    Add,
    Cancel,
    Modify,
    Clear,
    Trade,
    Fill,
    None,
    Unknown(u8),
}

impl MboAction {
    pub const fn from_code(value: u8) -> Self {
        match value {
            b'A' => Self::Add,
            b'C' => Self::Cancel,
            b'M' => Self::Modify,
            b'R' => Self::Clear,
            b'T' => Self::Trade,
            b'F' => Self::Fill,
            b'N' => Self::None,
            other => Self::Unknown(other),
        }
    }

    pub const fn code(self) -> u8 {
        match self {
            Self::Add => b'A',
            Self::Cancel => b'C',
            Self::Modify => b'M',
            Self::Clear => b'R',
            Self::Trade => b'T',
            Self::Fill => b'F',
            Self::None => b'N',
            Self::Unknown(value) => value,
        }
    }

    pub const fn mutates_book(self) -> bool {
        matches!(self, Self::Add | Self::Cancel | Self::Modify | Self::Clear)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum MboSide {
    Ask,
    Bid,
    None,
    Unknown(u8),
}

impl MboSide {
    pub const fn from_code(value: u8) -> Self {
        match value {
            b'A' => Self::Ask,
            b'B' => Self::Bid,
            b'N' => Self::None,
            other => Self::Unknown(other),
        }
    }

    pub const fn code(self) -> u8 {
        match self {
            Self::Ask => b'A',
            Self::Bid => b'B',
            Self::None => b'N',
            Self::Unknown(value) => value,
        }
    }

    pub const fn is_book_side(self) -> bool {
        matches!(self, Self::Ask | Self::Bid)
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct MboFlags {
    raw: u8,
}

impl MboFlags {
    pub const LAST: u8 = 1 << 7;
    pub const TOB: u8 = 1 << 6;
    pub const SNAPSHOT: u8 = 1 << 5;
    pub const MBP: u8 = 1 << 4;
    pub const BAD_TS_RECV: u8 = 1 << 3;
    pub const MAYBE_BAD_BOOK: u8 = 1 << 2;
    pub const PUBLISHER_SPECIFIC: u8 = 1 << 1;

    pub const fn new(raw: u8) -> Self {
        Self { raw }
    }

    pub const fn empty() -> Self {
        Self { raw: 0 }
    }

    pub const fn raw(self) -> u8 {
        self.raw
    }

    pub const fn is_last(self) -> bool {
        self.has(Self::LAST)
    }

    pub const fn is_top_of_book(self) -> bool {
        self.has(Self::TOB)
    }

    pub const fn is_snapshot(self) -> bool {
        self.has(Self::SNAPSHOT)
    }

    pub const fn is_mbp(self) -> bool {
        self.has(Self::MBP)
    }

    pub const fn is_bad_ts_recv(self) -> bool {
        self.has(Self::BAD_TS_RECV)
    }

    pub const fn is_maybe_bad_book(self) -> bool {
        self.has(Self::MAYBE_BAD_BOOK)
    }

    pub const fn is_publisher_specific(self) -> bool {
        self.has(Self::PUBLISHER_SPECIFIC)
    }

    const fn has(self, flag: u8) -> bool {
        self.raw & flag != 0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MboSourceInfo {
    pub dataset: String,
    pub schema: String,
    pub symbol: String,
    pub stype_in: String,
    pub stype_out: String,
}

impl MboSourceInfo {
    pub fn new(
        dataset: impl Into<String>,
        symbol: impl Into<String>,
        stype_in: impl Into<String>,
        stype_out: impl Into<String>,
    ) -> Self {
        Self {
            dataset: dataset.into(),
            schema: "mbo".to_string(),
            symbol: symbol.into(),
            stype_in: stype_in.into(),
            stype_out: stype_out.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MboEvent {
    pub source: MboSourceInfo,
    pub ts_recv: u64,
    pub ts_event: u64,
    pub ts_in_delta: i32,
    pub sequence: u32,
    pub rtype: u8,
    pub publisher_id: u16,
    pub instrument_id: u32,
    pub action: MboAction,
    pub side: MboSide,
    pub price: MboPrice,
    pub size: u32,
    pub channel_id: u8,
    pub order_id: u64,
    pub flags: MboFlags,
    pub symbol: Option<String>,
}

impl MboEvent {
    pub fn book_event(
        source: MboSourceInfo,
        instrument_id: u32,
        order_id: u64,
        action: MboAction,
        side: MboSide,
        price: MboPrice,
        size: u32,
    ) -> Self {
        Self {
            source,
            ts_recv: 0,
            ts_event: 0,
            ts_in_delta: 0,
            sequence: 0,
            rtype: 0,
            publisher_id: 0,
            instrument_id,
            action,
            side,
            price,
            size,
            channel_id: 0,
            order_id,
            flags: MboFlags::empty(),
            symbol: None,
        }
    }
}

pub trait MboEventSource {
    fn source_info(&self) -> &MboSourceInfo;
    fn next_batch(&mut self, max_events: usize) -> Result<Vec<MboEvent>, String>;
}

#[derive(Debug, Clone)]
pub struct VecMboEventSource {
    source: MboSourceInfo,
    events: Vec<MboEvent>,
    cursor: usize,
}

impl VecMboEventSource {
    pub fn new(source: MboSourceInfo, events: Vec<MboEvent>) -> Self {
        Self {
            source,
            events,
            cursor: 0,
        }
    }
}

impl MboEventSource for VecMboEventSource {
    fn source_info(&self) -> &MboSourceInfo {
        &self.source
    }

    fn next_batch(&mut self, max_events: usize) -> Result<Vec<MboEvent>, String> {
        if max_events == 0 || self.cursor >= self.events.len() {
            return Ok(Vec::new());
        }

        let end = self
            .cursor
            .saturating_add(max_events)
            .min(self.events.len());
        let batch = self.events[self.cursor..end].to_vec();
        self.cursor = end;
        Ok(batch)
    }
}

#[cfg(test)]
mod tests {
    use super::{
        FIXED_PRICE_SCALE, MboAction, MboEvent, MboEventSource, MboFlags, MboPrice, MboSide,
        MboSourceInfo, VecMboEventSource,
    };

    #[test]
    fn mbo_codes_round_trip_databento_values() {
        assert_eq!(MboAction::from_code(b'A'), MboAction::Add);
        assert_eq!(MboAction::from_code(b'C'), MboAction::Cancel);
        assert_eq!(MboAction::from_code(b'M'), MboAction::Modify);
        assert_eq!(MboAction::from_code(b'R'), MboAction::Clear);
        assert_eq!(MboAction::from_code(b'T'), MboAction::Trade);
        assert_eq!(MboAction::from_code(b'F'), MboAction::Fill);
        assert_eq!(MboAction::from_code(b'N'), MboAction::None);
        assert_eq!(MboSide::from_code(b'A'), MboSide::Ask);
        assert_eq!(MboSide::from_code(b'B'), MboSide::Bid);
        assert_eq!(MboSide::from_code(b'N'), MboSide::None);
    }

    #[test]
    fn price_keeps_fixed_point_units() {
        let price = MboPrice::from_fixed(18_250 * FIXED_PRICE_SCALE + 250_000_000);

        assert_eq!(price.fixed(), 18_250_250_000_000);
        assert_eq!(price.as_f64(), Some(18_250.25));
        assert_eq!(MboPrice::UNDEFINED.as_f64(), None);
    }

    #[test]
    fn flag_helpers_match_dbn_bits() {
        let flags = MboFlags::new(MboFlags::LAST | MboFlags::SNAPSHOT | MboFlags::MAYBE_BAD_BOOK);

        assert!(flags.is_last());
        assert!(flags.is_snapshot());
        assert!(flags.is_maybe_bad_book());
        assert!(!flags.is_mbp());
    }

    #[test]
    fn vec_event_source_batches_without_transport_coupling() {
        let source = MboSourceInfo::new("GLBX.MDP3", "NQ.FUT", "parent", "instrument_id");
        let events = vec![
            MboEvent::book_event(
                source.clone(),
                1,
                100,
                MboAction::Add,
                MboSide::Bid,
                MboPrice::from_fixed(100 * FIXED_PRICE_SCALE),
                2,
            ),
            MboEvent::book_event(
                source.clone(),
                1,
                101,
                MboAction::Add,
                MboSide::Ask,
                MboPrice::from_fixed(101 * FIXED_PRICE_SCALE),
                3,
            ),
        ];
        let mut event_source = VecMboEventSource::new(source, events);

        assert_eq!(event_source.next_batch(1).expect("first batch").len(), 1);
        assert_eq!(event_source.next_batch(4).expect("second batch").len(), 1);
        assert!(event_source.next_batch(4).expect("empty batch").is_empty());
    }
}
