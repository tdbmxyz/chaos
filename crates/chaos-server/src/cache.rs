//! A small bounded TTL cache that can also serve stale entries when a
//! refresh fails. Shared by the widget hub (widget payloads, geocoding)
//! and the ICS feed cache — the hand-rolled versions of this pattern were
//! unbounded, which mattered because weather cache keys come from the
//! user-controlled `?location=` query.

use std::collections::HashMap;
use std::hash::Hash;
use std::time::{Duration, Instant};

use tokio::sync::RwLock;

pub struct StaleCache<K, V> {
    max_entries: usize,
    inner: RwLock<Inner<K, V>>,
}

struct Inner<K, V> {
    entries: HashMap<K, Entry<V>>,
    /// Monotonic insertion counter: the entry with the smallest `seq` is
    /// the oldest-written and gets evicted first. (An `Instant` could tie;
    /// a counter cannot.)
    seq: u64,
}

struct Entry<V> {
    value: V,
    inserted: Instant,
    seq: u64,
}

impl<K: Eq + Hash + Clone, V: Clone> StaleCache<K, V> {
    pub fn new(max_entries: usize) -> Self {
        assert!(max_entries > 0, "cache must hold at least one entry");
        Self {
            max_entries,
            inner: RwLock::new(Inner {
                entries: HashMap::new(),
                seq: 0,
            }),
        }
    }

    /// The cached value if it is younger than `ttl`.
    pub async fn get_fresh(&self, key: &K, ttl: Duration) -> Option<V> {
        let inner = self.inner.read().await;
        let entry = inner.entries.get(key)?;
        (entry.inserted.elapsed() < ttl).then(|| entry.value.clone())
    }

    /// The cached value regardless of age (serve-stale-on-failure).
    pub async fn get_stale(&self, key: &K) -> Option<V> {
        self.inner
            .read()
            .await
            .entries
            .get(key)
            .map(|entry| entry.value.clone())
    }

    /// Insert or refresh a value. When the cache is full and the key is
    /// new, the oldest-written entry is evicted, bounding growth from
    /// user-controlled keys.
    pub async fn insert(&self, key: K, value: V) {
        let mut inner = self.inner.write().await;
        inner.seq += 1;
        let seq = inner.seq;
        if !inner.entries.contains_key(&key)
            && inner.entries.len() >= self.max_entries
            && let Some(oldest) = inner
                .entries
                .iter()
                .min_by_key(|(_, entry)| entry.seq)
                .map(|(key, _)| key.clone())
        {
            inner.entries.remove(&oldest);
        }
        inner.entries.insert(
            key,
            Entry {
                value,
                inserted: Instant::now(),
                seq,
            },
        );
    }

    /// Forget one entry (cache invalidation after an edit/delete).
    pub async fn remove(&self, key: &K) {
        self.inner.write().await.entries.remove(key);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn fresh_within_ttl_stale_after() {
        let cache = StaleCache::new(4);
        cache.insert("k", 1u32).await;
        assert_eq!(cache.get_fresh(&"k", Duration::from_secs(60)).await, Some(1));
        // A zero TTL makes any entry stale without sleeping.
        assert_eq!(cache.get_fresh(&"k", Duration::ZERO).await, None);
        assert_eq!(cache.get_stale(&"k").await, Some(1));
    }

    #[tokio::test]
    async fn missing_keys_yield_nothing() {
        let cache: StaleCache<&str, u32> = StaleCache::new(4);
        assert_eq!(cache.get_fresh(&"nope", Duration::from_secs(60)).await, None);
        assert_eq!(cache.get_stale(&"nope").await, None);
    }

    #[tokio::test]
    async fn eviction_drops_the_oldest_entry() {
        let cache = StaleCache::new(2);
        cache.insert("a", 1u32).await;
        cache.insert("b", 2).await;
        cache.insert("c", 3).await; // evicts "a"
        assert_eq!(cache.get_stale(&"a").await, None);
        assert_eq!(cache.get_stale(&"b").await, Some(2));
        assert_eq!(cache.get_stale(&"c").await, Some(3));
    }

    #[tokio::test]
    async fn refreshing_a_key_does_not_evict() {
        let cache = StaleCache::new(2);
        cache.insert("a", 1u32).await;
        cache.insert("b", 2).await;
        cache.insert("a", 10).await; // refresh in place, still 2 entries
        assert_eq!(cache.get_stale(&"a").await, Some(10));
        assert_eq!(cache.get_stale(&"b").await, Some(2));
    }

    #[tokio::test]
    async fn remove_forgets_the_entry() {
        let cache = StaleCache::new(2);
        cache.insert("a", 1u32).await;
        cache.remove(&"a").await;
        assert_eq!(cache.get_stale(&"a").await, None);
    }
}
