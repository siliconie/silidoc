//! Quiet-period debouncing and echo suppression for file-watch driven sync.

use std::collections::VecDeque;

/// Fires once changes stop for `quiet_ms`, but never later than `max_wait_ms` after the first
/// change (so a long typing burst still syncs). Times are caller-supplied milliseconds.
#[derive(Clone, Debug)]
pub struct Debouncer {
    quiet_ms: u64,
    max_wait_ms: u64,
    first: Option<u64>,
    last: Option<u64>,
}

impl Debouncer {
    pub fn new(quiet_ms: u64, max_wait_ms: u64) -> Self {
        Debouncer { quiet_ms, max_wait_ms: max_wait_ms.max(quiet_ms), first: None, last: None }
    }

    pub fn event(&mut self, now: u64) {
        self.first.get_or_insert(now);
        self.last = Some(now);
    }

    pub fn ready(&self, now: u64) -> bool {
        match (self.first, self.last) {
            (Some(f), Some(l)) => now.saturating_sub(l) >= self.quiet_ms || now.saturating_sub(f) >= self.max_wait_ms,
            _ => false,
        }
    }

    /// Returns true and resets if it is time to sync.
    pub fn take(&mut self, now: u64) -> bool {
        if self.ready(now) {
            self.first = None;
            self.last = None;
            true
        } else {
            false
        }
    }

    pub fn pending(&self) -> bool {
        self.first.is_some()
    }
}

/// Remembers content hashes of files we just wrote, so the file watcher does not treat our own
/// write as an outside change (which would loop forever).
#[derive(Clone, Debug)]
pub struct EchoGuard {
    recent: VecDeque<String>,
    cap: usize,
}

impl EchoGuard {
    pub fn new(cap: usize) -> Self {
        EchoGuard { recent: VecDeque::new(), cap: cap.max(1) }
    }

    pub fn wrote(&mut self, sha256: &str) {
        if self.recent.len() == self.cap {
            self.recent.pop_front();
        }
        self.recent.push_back(sha256.to_string());
    }

    /// True (once) if `sha256` is the hash of something we wrote.
    pub fn is_echo(&mut self, sha256: &str) -> bool {
        match self.recent.iter().position(|h| h == sha256) {
            Some(i) => {
                self.recent.remove(i);
                true
            }
            None => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fires_after_quiet_period() {
        let mut d = Debouncer::new(1000, 5000);
        assert!(!d.ready(0));
        d.event(100);
        assert!(!d.take(600));
        d.event(900);
        assert!(!d.take(1500));
        assert!(d.take(1900));
        assert!(!d.pending());
    }

    #[test]
    fn max_wait_forces_a_sync_during_a_long_burst() {
        let mut d = Debouncer::new(1000, 3000);
        for t in (0..=3000).step_by(500) {
            d.event(t);
        }
        assert!(d.take(3000));
    }

    #[test]
    fn echo_is_recognised_once() {
        let mut g = EchoGuard::new(2);
        g.wrote("a");
        g.wrote("b");
        assert!(g.is_echo("a"));
        assert!(!g.is_echo("a"));
        assert!(!g.is_echo("zzz"));
    }

    #[test]
    fn echo_guard_forgets_oldest() {
        let mut g = EchoGuard::new(1);
        g.wrote("a");
        g.wrote("b");
        assert!(!g.is_echo("a"));
        assert!(g.is_echo("b"));
    }
}
