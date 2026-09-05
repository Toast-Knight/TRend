use crate::CellFrame;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CacheInsert {
    Inserted,
    Replaced,
    Disabled,
    RejectedTooLarge,
}

#[derive(Clone, Debug)]
struct Entry<K> {
    key: K,
    frame: CellFrame,
    last_used: u64,
}

#[derive(Clone, Debug)]
pub struct CellFrameCache<K> {
    entries: Vec<Entry<K>>,
    max_entries: usize,
    max_bytes: usize,
    used_bytes: usize,
    access_counter: u64,
}

impl<K: Eq> CellFrameCache<K> {
    pub fn new(max_entries: usize, max_bytes: usize) -> Self {
        Self {
            entries: Vec::new(),
            max_entries,
            max_bytes,
            used_bytes: 0,
            access_counter: 0,
        }
    }

    pub fn get(&mut self, key: &K) -> Option<&CellFrame> {
        let index = self.entries.iter().position(|entry| &entry.key == key)?;
        self.access_counter = self.access_counter.saturating_add(1);
        self.entries[index].last_used = self.access_counter;
        Some(&self.entries[index].frame)
    }

    pub fn insert(&mut self, key: K, frame: CellFrame) -> CacheInsert {
        if self.max_entries == 0 || self.max_bytes == 0 {
            return CacheInsert::Disabled;
        }

        let frame_bytes = frame.allocated_bytes();
        if frame_bytes > self.max_bytes {
            return CacheInsert::RejectedTooLarge;
        }

        let existing = self.entries.iter().position(|entry| entry.key == key);
        let result = if let Some(index) = existing {
            let old = self.entries.remove(index);
            self.used_bytes = self.used_bytes.saturating_sub(old.frame.allocated_bytes());
            CacheInsert::Replaced
        } else {
            CacheInsert::Inserted
        };

        while self.entries.len() >= self.max_entries
            || frame_bytes > self.max_bytes - self.used_bytes
        {
            self.evict_lru();
        }

        self.access_counter = self.access_counter.saturating_add(1);
        self.used_bytes += frame_bytes;
        self.entries.push(Entry {
            key,
            frame,
            last_used: self.access_counter,
        });
        result
    }

    pub fn clear(&mut self) {
        self.entries.clear();
        self.used_bytes = 0;
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub const fn max_entries(&self) -> usize {
        self.max_entries
    }

    pub const fn max_bytes(&self) -> usize {
        self.max_bytes
    }

    pub const fn used_bytes(&self) -> usize {
        self.used_bytes
    }

    fn evict_lru(&mut self) {
        let Some((index, _)) = self
            .entries
            .iter()
            .enumerate()
            .min_by_key(|(_, entry)| entry.last_used)
        else {
            return;
        };
        let removed = self.entries.remove(index);
        self.used_bytes = self
            .used_bytes
            .saturating_sub(removed.frame.allocated_bytes());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(width: u16, height: u16) -> CellFrame {
        let mut frame = CellFrame::default();
        frame.resize(width, height).unwrap();
        frame
    }

    #[test]
    fn cache_never_exceeds_byte_budget() {
        let frame_bytes = frame(2, 2).allocated_bytes();
        let mut cache = CellFrameCache::new(4, frame_bytes);

        assert_eq!(cache.insert(1, frame(2, 2)), CacheInsert::Inserted);
        assert_eq!(cache.insert(2, frame(2, 2)), CacheInsert::Inserted);

        assert!(cache.used_bytes() <= cache.max_bytes());
        assert_eq!(cache.len(), 1);
        assert!(cache.get(&1).is_none());
        assert!(cache.get(&2).is_some());
    }

    #[test]
    fn access_refreshes_lru_order() {
        let frame_bytes = frame(1, 1).allocated_bytes();
        let mut cache = CellFrameCache::new(2, frame_bytes * 3);
        cache.insert(1, frame(1, 1));
        cache.insert(2, frame(1, 1));
        assert!(cache.get(&1).is_some());
        cache.insert(3, frame(1, 1));

        assert!(cache.get(&1).is_some());
        assert!(cache.get(&2).is_none());
        assert!(cache.get(&3).is_some());
    }

    #[test]
    fn oversized_replacement_keeps_existing_entry() {
        let small = frame(1, 1);
        let budget = small.allocated_bytes();
        let mut cache = CellFrameCache::new(2, budget);
        cache.insert(1, small);

        assert_eq!(cache.insert(1, frame(4, 4)), CacheInsert::RejectedTooLarge);
        assert!(cache.get(&1).is_some());
        assert_eq!(cache.used_bytes(), budget);
    }

    #[test]
    fn zero_limits_disable_cache_without_retaining_frame() {
        let mut cache = CellFrameCache::new(0, 0);
        assert_eq!(cache.insert(1, frame(1, 1)), CacheInsert::Disabled);
        assert!(cache.is_empty());
        assert_eq!(cache.used_bytes(), 0);
    }
}
