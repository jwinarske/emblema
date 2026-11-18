//! A pass's transient attachments, kept from one submission to the next.
//!
//! A multisampled pass renders into a color buffer it resolves out of, and a
//! clipped one into a stencil buffer; neither is read after the pass. They were
//! allocated and freed inside every submission, which costs nothing a desktop
//! driver notices and several milliseconds a frame on a small one: on a
//! PowerVR BXE-4-32 at 1280x720 and four samples, 2.3 ms to make the two and
//! 1.2 ms to free them, of a 16.7 ms frame.
//!
//! A frame loop submits the same pass again and again, so the buffers a
//! submission ends with are the ones the next begins with. They are kept by
//! what makes one usable in place of another: extent, format and sample count.
//!
//! # Why so few
//!
//! [`CAPACITY`] of each. A buffer here is the size of its target times its
//! sample count, 33 MB for a 1920x1080 target at four samples, so keeping one
//! per size ever drawn would hold memory nothing uses. Two covers a frame and
//! the layer inside it; a submission that finds none makes its own, as every
//! submission did before, and the least recently used is freed to make room.
//!
//! # Why taking, not borrowing
//!
//! A submission takes a buffer out and puts it back when the GPU is done with
//! it. Nothing here is ever in use by a submission, so nothing here needs to
//! know about one.

use ash::vk;
use emblema_hal::{Extent2D, PixelFormat};

/// How many buffers of each kind are kept.
pub(crate) const CAPACITY: usize = 2;

/// What makes one multisample color buffer usable in place of another.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ColorKey {
    pub extent: Extent2D,
    pub format: PixelFormat,
    pub samples: u32,
}

/// What makes one stencil buffer usable in place of another.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct StencilKey {
    pub extent: Extent2D,
    pub format: vk::Format,
    pub samples: u32,
}

/// Up to [`CAPACITY`] buffers by key, the least recently put back first.
#[derive(Debug)]
pub(crate) struct Kept<K, T> {
    buffers: Vec<(K, T)>,
}

impl<K, T> Default for Kept<K, T> {
    fn default() -> Self {
        Self {
            buffers: Vec::with_capacity(CAPACITY + 1),
        }
    }
}

impl<K: PartialEq, T> Kept<K, T> {
    /// A buffer for `key`, no longer kept, if one is.
    pub(crate) fn take(&mut self, key: &K) -> Option<T> {
        let at = self.buffers.iter().position(|(k, _)| k == key)?;
        Some(self.buffers.remove(at).1)
    }

    /// Keeps `buffer`. The one it pushes out, if the cache was full, is the
    /// caller's to destroy.
    #[must_use = "a buffer pushed out holds device memory until it is destroyed"]
    pub(crate) fn put(&mut self, key: K, buffer: T) -> Option<T> {
        self.buffers.push((key, buffer));
        (self.buffers.len() > CAPACITY).then(|| self.buffers.remove(0).1)
    }

    /// Everything kept, for destroying.
    pub(crate) fn drain(&mut self) -> impl Iterator<Item = T> + '_ {
        self.buffers.drain(..).map(|(_, buffer)| buffer)
    }

    pub(crate) fn len(&self) -> usize {
        self.buffers.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_buffer_taken_is_no_longer_kept() {
        let mut kept = Kept::default();
        assert!(kept.put(1, "a").is_none());
        assert_eq!(kept.take(&1), Some("a"));
        assert_eq!(kept.take(&1), None);
    }

    #[test]
    fn a_key_with_no_buffer_takes_nothing_and_disturbs_nothing() {
        let mut kept = Kept::default();
        assert!(kept.put(1, "a").is_none());
        assert_eq!(kept.take(&2), None);
        assert_eq!(kept.len(), 1);
    }

    #[test]
    fn the_least_recently_put_back_is_pushed_out() {
        let mut kept = Kept::default();
        assert!(kept.put(1, "a").is_none());
        assert!(kept.put(2, "b").is_none());
        // Used again, so it is the newer of the two.
        let a = kept.take(&1).unwrap();
        assert!(kept.put(1, a).is_none());
        assert_eq!(kept.put(3, "c"), Some("b"));
        assert_eq!(kept.len(), CAPACITY);
        assert_eq!(kept.take(&1), Some("a"));
    }

    #[test]
    fn two_buffers_of_one_key_are_both_kept() {
        // Two submissions' worth put back before either is taken again.
        let mut kept = Kept::default();
        assert!(kept.put(1, "a").is_none());
        assert!(kept.put(1, "b").is_none());
        assert_eq!(kept.take(&1), Some("a"));
        assert_eq!(kept.take(&1), Some("b"));
    }

    #[test]
    fn draining_leaves_nothing() {
        let mut kept = Kept::default();
        assert!(kept.put(1, "a").is_none());
        assert!(kept.put(2, "b").is_none());
        assert_eq!(kept.drain().collect::<Vec<_>>(), ["a", "b"]);
        assert_eq!(kept.len(), 0);
    }
}
