use crate::geometry::Viewport;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PendingResize {
    pub viewport: Viewport,
    pub generation: u64,
}

#[derive(Clone, Debug, Default)]
pub struct ResizeCoalescer {
    generation: u64,
    pending: Option<PendingResize>,
}

impl ResizeCoalescer {
    pub const fn new() -> Self {
        Self {
            generation: 0,
            pending: None,
        }
    }

    pub fn submit(&mut self, viewport: Viewport) -> u64 {
        self.generation = self.generation.wrapping_add(1);
        if self.generation == 0 {
            self.generation = 1;
        }
        self.pending = Some(PendingResize {
            viewport,
            generation: self.generation,
        });
        self.generation
    }

    pub fn take_pending(&mut self) -> Option<PendingResize> {
        self.pending.take()
    }

    pub const fn latest_generation(&self) -> u64 {
        self.generation
    }

    pub const fn is_current(&self, generation: u64) -> bool {
        generation == self.generation
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rapid_resizes_keep_only_latest_request() {
        let mut coalescer = ResizeCoalescer::new();
        for width in 1..=32 {
            coalescer.submit(Viewport { width, height: 20 });
        }

        let pending = coalescer.take_pending().unwrap();
        assert_eq!(
            pending.viewport,
            Viewport {
                width: 32,
                height: 20
            }
        );
        assert_eq!(pending.generation, 32);
        assert!(coalescer.take_pending().is_none());
    }

    #[test]
    fn newer_resize_invalidates_in_progress_generation() {
        let mut coalescer = ResizeCoalescer::new();
        let old = coalescer.submit(Viewport {
            width: 80,
            height: 24,
        });
        coalescer.submit(Viewport {
            width: 120,
            height: 40,
        });

        assert!(!coalescer.is_current(old));
    }
}
