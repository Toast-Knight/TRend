use std::collections::VecDeque;

#[derive(Clone, Debug)]
pub struct BoundedQueue<T> {
    items: VecDeque<T>,
    capacity: usize,
}

impl<T> BoundedQueue<T> {
    pub fn new(capacity: usize) -> Result<Self, &'static str> {
        if capacity == 0 {
            return Err("queue capacity must be non-zero");
        }
        Ok(Self {
            items: VecDeque::with_capacity(capacity),
            capacity,
        })
    }

    pub const fn capacity(&self) -> usize {
        self.capacity
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn push_file(&mut self, item: T) -> Result<(), T> {
        if self.items.len() == self.capacity {
            return Err(item);
        }
        self.items.push_back(item);
        Ok(())
    }

    pub fn push_live(&mut self, item: T) -> Option<T> {
        let dropped = if self.items.len() == self.capacity {
            self.items.pop_front()
        } else {
            None
        };
        self.items.push_back(item);
        dropped
    }

    pub fn pop_front(&mut self) -> Option<T> {
        self.items.pop_front()
    }

    pub fn clear(&mut self) {
        self.items.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_queue_applies_backpressure_at_capacity() {
        let mut queue = BoundedQueue::new(2).unwrap();
        queue.push_file(1).unwrap();
        queue.push_file(2).unwrap();

        assert_eq!(queue.push_file(3), Err(3));
        assert_eq!(queue.len(), 2);
    }

    #[test]
    fn live_queue_drops_oldest_without_growing() {
        let mut queue = BoundedQueue::new(3).unwrap();
        for item in 0..10 {
            queue.push_live(item);
        }

        assert_eq!(queue.len(), 3);
        assert_eq!(queue.pop_front(), Some(7));
        assert_eq!(queue.pop_front(), Some(8));
        assert_eq!(queue.pop_front(), Some(9));
    }
}
