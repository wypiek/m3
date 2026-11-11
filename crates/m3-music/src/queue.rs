//! The play queue: order, shuffle and repeat (pure logic, unit-tested).

/// Repeat mode (MPRIS `LoopStatus`: None / Playlist / Track).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Repeat {
    #[default]
    Off,
    All,
    One,
}

impl Repeat {
    pub fn cycle(self) -> Self {
        match self {
            Repeat::Off => Repeat::All,
            Repeat::All => Repeat::One,
            Repeat::One => Repeat::Off,
        }
    }

    pub fn mpris(self) -> &'static str {
        match self {
            Repeat::Off => "None",
            Repeat::All => "Playlist",
            Repeat::One => "Track",
        }
    }

    pub fn from_mpris(s: &str) -> Option<Self> {
        match s {
            "None" => Some(Repeat::Off),
            "Playlist" => Some(Repeat::All),
            "Track" => Some(Repeat::One),
            _ => None,
        }
    }
}

/// A queue of track ids (indices into the library) with a play order.
#[derive(Debug, Clone, Default)]
pub struct Queue {
    /// Tracks in their natural (list) order.
    items: Vec<usize>,
    /// Play order: positions into `items`.
    order: Vec<usize>,
    /// Index into `order` of the current track.
    pos: Option<usize>,
    shuffle: bool,
    pub repeat: Repeat,
    rng: u64,
}

impl Queue {
    pub fn new(seed: u64) -> Self {
        Self { rng: seed | 1, ..Default::default() }
    }

    /// Replace the queue with `items`, starting at `start` (an index into
    /// `items`).
    pub fn set(&mut self, items: Vec<usize>, start: usize) {
        self.items = items;
        self.order = (0..self.items.len()).collect();
        if self.items.is_empty() {
            self.pos = None;
            return;
        }
        let start = start.min(self.items.len() - 1);
        if self.shuffle {
            self.shuffle_order(start);
            self.pos = Some(0);
        } else {
            self.pos = Some(start);
        }
    }

    /// Tracks in their natural order.
    pub fn items(&self) -> &[usize] {
        &self.items
    }

    /// Position of the current track in [`Queue::items`].
    pub fn current_item(&self) -> Option<usize> {
        self.pos.and_then(|p| self.order.get(p)).copied()
    }

    pub fn current(&self) -> Option<usize> {
        self.pos.and_then(|p| self.order.get(p)).and_then(|&i| self.items.get(i)).copied()
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn shuffle(&self) -> bool {
        self.shuffle
    }

    /// Turn shuffle on/off, keeping the current track playing.
    pub fn set_shuffle(&mut self, on: bool) {
        if on == self.shuffle {
            return;
        }
        self.shuffle = on;
        let cur = self.pos.and_then(|p| self.order.get(p).copied());
        if on {
            if let Some(c) = cur {
                self.shuffle_order(c);
                self.pos = Some(0);
            } else {
                self.shuffle_order(0);
            }
        } else {
            self.order = (0..self.items.len()).collect();
            self.pos = cur;
        }
    }

    /// Advance. `auto` is true at the end of a track (repeat-one replays
    /// it); a user "next" always moves on. Returns the new current track.
    pub fn next(&mut self, auto: bool) -> Option<usize> {
        let p = self.pos?;
        if auto && self.repeat == Repeat::One {
            return self.current();
        }
        if p + 1 < self.order.len() {
            self.pos = Some(p + 1);
        } else if self.repeat != Repeat::Off {
            if self.shuffle {
                // A fresh shuffle for the next round.
                let last = self.order[p];
                self.shuffle_order(usize::MAX);
                if self.order.len() > 1 && self.order[0] == last {
                    self.order.swap(0, 1);
                }
            }
            self.pos = Some(0);
        } else {
            return None;
        }
        self.current()
    }

    /// Go back one track (wrapping when repeating).
    pub fn previous(&mut self) -> Option<usize> {
        let p = self.pos?;
        if p > 0 {
            self.pos = Some(p - 1);
        } else if self.repeat != Repeat::Off && !self.order.is_empty() {
            self.pos = Some(self.order.len() - 1);
        }
        self.current()
    }

    pub fn has_next(&self) -> bool {
        match self.pos {
            Some(p) => p + 1 < self.order.len() || self.repeat != Repeat::Off,
            None => false,
        }
    }

    pub fn has_previous(&self) -> bool {
        match self.pos {
            Some(p) => p > 0 || self.repeat != Repeat::Off,
            None => false,
        }
    }

    /// Upcoming tracks in play order (after the current one).
    pub fn upcoming(&self) -> Vec<usize> {
        let Some(p) = self.pos else { return Vec::new() };
        self.order.iter().skip(p + 1).filter_map(|&i| self.items.get(i).copied()).collect()
    }

    fn rand(&mut self) -> u64 {
        // xorshift64*
        let mut x = self.rng;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.rng = x;
        x.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }

    /// Shuffle the order; `first` (an index into `items`) goes first.
    fn shuffle_order(&mut self, first: usize) {
        let mut order: Vec<usize> = (0..self.items.len()).collect();
        for i in (1..order.len()).rev() {
            let j = (self.rand() % (i as u64 + 1)) as usize;
            order.swap(i, j);
        }
        if let Some(k) = order.iter().position(|&i| i == first) {
            order.swap(0, k);
        }
        self.order = order;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn linear_play_and_end() {
        let mut q = Queue::new(1);
        q.set(vec![10, 11, 12], 1);
        assert_eq!(q.current(), Some(11));
        assert_eq!(q.next(true), Some(12));
        assert_eq!(q.next(true), None, "stops at the end without repeat");
        assert_eq!(q.previous(), Some(11));
        assert!(q.has_next() && q.has_previous());
    }

    #[test]
    fn repeat_modes() {
        let mut q = Queue::new(1);
        q.set(vec![1, 2], 1);
        q.repeat = Repeat::One;
        assert_eq!(q.next(true), Some(2), "repeat-one replays at track end");
        assert_eq!(q.next(false), Some(1), "but next moves on (and wraps)");
        q.repeat = Repeat::All;
        assert_eq!(q.next(true), Some(2));
        assert_eq!(q.next(true), Some(1), "repeat-all wraps");
        assert_eq!(q.previous(), Some(2), "previous wraps too");
        assert_eq!(Repeat::Off.cycle().cycle().cycle(), Repeat::Off);
        assert_eq!(Repeat::from_mpris(Repeat::All.mpris()), Some(Repeat::All));
    }

    #[test]
    fn shuffle_keeps_current_and_covers_all() {
        let mut q = Queue::new(42);
        q.set((0..50).collect(), 7);
        q.set_shuffle(true);
        assert_eq!(q.current(), Some(7), "shuffling keeps the current track");
        let mut seen = vec![7];
        while let Some(t) = q.next(true) {
            seen.push(t);
        }
        seen.sort_unstable();
        assert_eq!(seen, (0..50).collect::<Vec<_>>(), "every track once");
        q.set_shuffle(false);
        assert_eq!(q.upcoming().len(), 49 - q.current().unwrap_or(0));
    }

    #[test]
    fn shuffled_start() {
        let mut q = Queue::new(3);
        q.set_shuffle(true);
        q.set(vec![5, 6, 7, 8], 2);
        assert_eq!(q.current(), Some(7), "a shuffled queue starts with the clicked track");
        assert_eq!(q.upcoming().len(), 3);
        let mut empty = Queue::new(3);
        empty.set(Vec::new(), 0);
        assert_eq!((empty.current(), empty.next(false)), (None, None));
    }
}
