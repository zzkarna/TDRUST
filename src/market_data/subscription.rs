use std::collections::HashMap;

use super::{Resolution, SubscriptionId};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubscriptionKey {
    pub symbol: String,
    pub resolution: Resolution,
}

#[derive(Debug, Clone)]
pub struct SubscriptionRouter {
    next_id: u64,
    active: HashMap<SubscriptionId, SubscriptionKey>,
}

impl Default for SubscriptionRouter {
    fn default() -> Self {
        Self::new()
    }
}

impl SubscriptionRouter {
    pub fn new() -> Self {
        Self {
            next_id: 1,
            active: HashMap::new(),
        }
    }

    pub fn subscribe(&mut self, key: SubscriptionKey) -> SubscriptionId {
        let id = SubscriptionId(self.next_id);
        self.next_id = self.next_id.wrapping_add(1).max(1);
        self.active.insert(id, key);
        id
    }

    pub fn unsubscribe(&mut self, id: SubscriptionId) -> Option<SubscriptionKey> {
        self.active.remove(&id)
    }

    pub fn accepts(&self, id: SubscriptionId, symbol: &str, resolution: Resolution) -> bool {
        self.active.get(&id).is_some_and(|key| {
            key.symbol.eq_ignore_ascii_case(symbol) && key.resolution == resolution
        })
    }

    pub fn len(&self) -> usize {
        self.active.len()
    }

    pub fn is_empty(&self) -> bool {
        self.active.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::{SubscriptionKey, SubscriptionRouter};
    use crate::market_data::Resolution;

    #[test]
    fn routes_updates_to_exact_symbol_and_resolution() {
        let mut router = SubscriptionRouter::new();
        let one_minute = router.subscribe(SubscriptionKey {
            symbol: "BTCUSDT".to_string(),
            resolution: Resolution::Minutes(1),
        });
        let five_minute = router.subscribe(SubscriptionKey {
            symbol: "BTCUSDT".to_string(),
            resolution: Resolution::Minutes(5),
        });

        assert!(router.accepts(one_minute, "btcusdt", Resolution::Minutes(1)));
        assert!(!router.accepts(one_minute, "BTCUSDT", Resolution::Minutes(5)));
        assert!(router.accepts(five_minute, "BTCUSDT", Resolution::Minutes(5)));
    }

    #[test]
    fn unsubscribe_blocks_late_updates() {
        let mut router = SubscriptionRouter::new();
        let id = router.subscribe(SubscriptionKey {
            symbol: "ETHUSDT".to_string(),
            resolution: Resolution::Minutes(1),
        });

        assert!(router.accepts(id, "ETHUSDT", Resolution::Minutes(1)));
        assert!(router.unsubscribe(id).is_some());
        assert!(!router.accepts(id, "ETHUSDT", Resolution::Minutes(1)));
        assert!(router.is_empty());
    }
}
