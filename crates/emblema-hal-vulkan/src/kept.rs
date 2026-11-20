//! A pass's transient attachments, kept from one submission to the next.
//!
//! A multisampled pass renders into a color buffer it resolves out of, and a
//! clipped one into a stencil buffer; neither is read after the pass. They were
//! allocated and freed inside every submission, which costs nothing a desktop
//! driver notices and several milliseconds a frame on a small one: on a
//! PowerVR BXE-4-32 at 1280x720 and four samples, 2.3 ms to make the two and
//! 1.2 ms to free them, of a 16.7 ms frame.
//!
//! A frame loop submits the same pass again and again, so the buffers one
//! submission used are the ones the next uses. They are kept by what makes one
//! usable in place of another: extent, format and sample count.
//!
//! # Shared by the submissions in flight
//!
//! Every pass clears these at its start and discards them at its end, so no
//! pass reads what another left, and the render pass orders each pass's
//! writes to them after those of every pass submitted before it on the queue
//! (`build_render_pass`'s dependency from outside the pass). So frames in
//! flight use one buffer of a key between them, where each had its own: a
//! frame loop three deep at 3840x2160 and four samples held three of each, a
//! multisample buffer and a stencil buffer of 127 MiB apiece, 760 MiB where
//! 254 does, and a Pi 5 took 80 ms to make each one more in the frame that
//! first needed it.
//!
//! A buffer counts the submissions using it, and is freed only once none is.
//!
//! # Why so few
//!
//! [`CAPACITY`] of each that nothing is using. A buffer here is the size of
//! its target times its sample count, 33 MB for a 1920x1080 target at four
//! samples, so keeping one per size ever drawn would hold memory nothing uses.
//! Two covers a frame and the layer inside it; a submission that finds none
//! makes its own, and the least recently used of those idle is freed to make
//! room.

use ash::vk;
use emblema_hal::{Extent2D, PixelFormat};

/// How many buffers of each kind are kept with no submission using them.
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

/// Which kept buffer a submission is using, for [`Kept::release`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Use(u64);

#[derive(Debug)]
struct Entry<K, T> {
    key: K,
    buffer: T,
    id: u64,
    /// The submissions using it.
    users: usize,
    /// Freed once `users` is none, whatever the capacity: given back by
    /// [`Kept::drain`] while a submission still had it.
    doomed: bool,
}

/// Buffers by key, the least recently used first.
#[derive(Debug)]
pub(crate) struct Kept<K, T> {
    buffers: Vec<Entry<K, T>>,
    next: u64,
}

impl<K, T> Default for Kept<K, T> {
    fn default() -> Self {
        Self {
            buffers: Vec::with_capacity(CAPACITY + 1),
            next: 0,
        }
    }
}

impl<K: PartialEq, T> Kept<K, T> {
    /// The buffer kept for `key`, now used by one more submission, if one is.
    pub(crate) fn share(&mut self, key: &K) -> Option<(Use, &T)> {
        let at = self
            .buffers
            .iter()
            .position(|e| !e.doomed && &e.key == key)?;
        let mut entry = self.buffers.remove(at);
        entry.users += 1;
        let id = Use(entry.id);
        self.buffers.push(entry);
        let entry = self.buffers.last().expect("pushed above");
        Some((id, &entry.buffer))
    }

    /// Keeps `buffer`, made for a submission that is using it.
    pub(crate) fn keep(&mut self, key: K, buffer: T) -> (Use, &T) {
        let id = self.next;
        self.next += 1;
        self.buffers.push(Entry {
            key,
            buffer,
            id,
            users: 1,
            doomed: false,
        });
        let entry = self.buffers.last().expect("pushed above");
        (Use(id), &entry.buffer)
    }

    /// One submission fewer using a buffer. What that leaves past the
    /// capacity, or given back while in use, is the caller's to destroy.
    #[must_use = "a buffer pushed out holds device memory until it is destroyed"]
    pub(crate) fn release(&mut self, id: Use) -> Vec<T> {
        if let Some(entry) = self.buffers.iter_mut().find(|e| e.id == id.0) {
            entry.users = entry.users.saturating_sub(1);
        }
        self.trim()
    }

    /// The idle buffers past the capacity, the least recently used first, and
    /// the doomed ones no submission has.
    fn trim(&mut self) -> Vec<T> {
        let mut freed = Vec::new();
        let mut at = 0;
        while at < self.buffers.len() {
            let entry = &self.buffers[at];
            let idle = self.buffers.iter().filter(|e| e.users == 0).count();
            if entry.users == 0 && (entry.doomed || idle > CAPACITY) {
                freed.push(self.buffers.remove(at).buffer);
            } else {
                at += 1;
            }
        }
        freed
    }

    /// Everything no submission is using, for destroying. A buffer still in
    /// use is freed by the [`Self::release`] that leaves it with none.
    pub(crate) fn drain(&mut self) -> Vec<T> {
        for entry in &mut self.buffers {
            entry.doomed = true;
        }
        self.trim()
    }

    /// Buffers kept, in use or not.
    pub(crate) fn len(&self) -> usize {
        self.buffers.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_buffer_kept_is_shared_by_the_next_submission() {
        let mut kept = Kept::default();
        let (first, _) = kept.keep(1, "a");
        let (second, buffer) = kept.share(&1).expect("shared");
        assert_eq!(*buffer, "a");
        assert_eq!(first, second, "one buffer, two users");
        assert_eq!(kept.len(), 1);
    }

    #[test]
    fn a_key_with_no_buffer_shares_nothing_and_disturbs_nothing() {
        let mut kept = Kept::default();
        let _ = kept.keep(1, "a");
        assert!(kept.share(&2).is_none());
        assert_eq!(kept.len(), 1);
    }

    #[test]
    fn a_buffer_in_use_is_never_pushed_out() {
        let mut kept = Kept::default();
        let (a, _) = kept.keep(1, "a");
        let (b, _) = kept.keep(2, "b");
        let (c, _) = kept.keep(3, "c");
        // All three in use: past the capacity, and nothing to free.
        assert_eq!(kept.len(), 3);
        assert!(kept.release(a).is_empty(), "the first idle one is kept");
        assert!(kept.release(b).is_empty(), "and the second");
        assert_eq!(kept.release(c), ["a"], "the third pushes out the oldest");
        assert_eq!(kept.len(), CAPACITY);
    }

    #[test]
    fn a_buffer_is_idle_only_once_every_user_has_released_it() {
        let mut kept = Kept::default();
        let (first, _) = kept.keep(1, "a");
        let (second, _) = kept.share(&1).expect("shared");
        let _ = kept.keep(2, "b");
        let _ = kept.keep(3, "c");
        assert!(kept.release(first).is_empty());
        let (b, _) = kept.share(&2).expect("shared");
        let (c, _) = kept.share(&3).expect("shared");
        // b and c have two users each now; release one of each and the
        // second of a's.
        assert!(kept.release(b).is_empty());
        assert!(kept.release(c).is_empty());
        assert!(
            kept.release(second).is_empty(),
            "two idle: a, and none else"
        );
    }

    #[test]
    fn the_least_recently_used_idle_buffer_is_pushed_out() {
        let mut kept = Kept::default();
        let (a, _) = kept.keep(1, "a");
        assert!(kept.release(a).is_empty());
        let (b, _) = kept.keep(2, "b");
        assert!(kept.release(b).is_empty());
        // Used again, so it is the newer of the two.
        let (a, _) = kept.share(&1).expect("shared");
        assert!(kept.release(a).is_empty());
        let (c, _) = kept.keep(3, "c");
        assert_eq!(kept.release(c), ["b"]);
        assert!(kept.share(&1).is_some());
    }

    #[test]
    fn draining_frees_the_idle_now_and_the_rest_when_released() {
        let mut kept = Kept::default();
        let (a, _) = kept.keep(1, "a");
        let (b, _) = kept.keep(2, "b");
        assert!(kept.release(a).is_empty());
        assert_eq!(kept.drain(), ["a"]);
        assert_eq!(kept.len(), 1, "b is still in use");
        assert!(kept.share(&2).is_none(), "and handed to no one new");
        assert_eq!(kept.release(b), ["b"]);
        assert_eq!(kept.len(), 0);
    }
}
