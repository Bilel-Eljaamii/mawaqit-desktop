use std::{
    collections::HashMap,
    sync::Mutex,
    time::{Duration, Instant},
};

/// Minimal in-process TTL cache (the reference mawaqit-api uses Redis for
/// this).
pub(crate) struct TtlCache<V> {
    entries: Mutex<HashMap<String, (Instant, V)>>,
    ttl: Duration,
}

impl<V: Clone> TtlCache<V> {
    pub fn new(ttl: Duration) -> Self {
        Self { entries: Mutex::new(HashMap::new()), ttl }
    }

    pub fn get(&self, key: &str) -> Option<V> {
        let mut entries = self.entries.lock().ok()?;
        let (created, value) = entries.get(key)?;
        if created.elapsed() >= self.ttl {
            entries.remove(key);
            return None;
        }
        Some(value.clone())
    }

    pub fn insert(&self, key: String, value: V) {
        if let Ok(mut entries) = self.entries.lock() {
            entries.insert(key, (Instant::now(), value));
        }
    }

    pub fn invalidate(&self, key: &str) {
        if let Ok(mut entries) = self.entries.lock() {
            entries.remove(key);
        }
    }

    pub fn clear(&self) {
        if let Ok(mut entries) = self.entries.lock() {
            entries.clear();
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    #[test]
    fn stores_and_expires() {
        let cache: TtlCache<u32> = TtlCache::new(Duration::from_secs(60));
        cache.insert("a".into(), 1);
        assert_eq!(cache.get("a"), Some(1));
        cache.invalidate("a");
        assert_eq!(cache.get("a"), None);
    }

    #[test]
    fn zero_ttl_expires_immediately() {
        let cache: TtlCache<u32> = TtlCache::new(Duration::ZERO);
        cache.insert("a".into(), 1);
        assert_eq!(cache.get("a"), None);
    }
}
