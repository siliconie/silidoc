//! Where rows and columns sit along one axis of the grid.
//!
//! A sheet has a million rows, so positions are `f64` and are worked out from the few lines that
//! differ from the default size (resized or hidden ones) instead of being stored per line.
//! Everything here is in sheet units (points); the view multiplies by its zoom.

use std::collections::BTreeMap;

/// One axis (rows or columns): a default size plus the lines that differ.
#[derive(Clone, Debug, Default)]
pub struct Lines {
    default: f64,
    /// `(index, size, extra before this line)`, sorted by index. `extra` is the sum of
    /// `size - default` over all earlier entries.
    marks: Vec<(u32, f64, f64)>,
}

impl Lines {
    pub fn new(default: f32, sizes: impl IntoIterator<Item = (u32, f32)>) -> Lines {
        let default = f64::from(default.max(1.0));
        let sorted: BTreeMap<u32, f64> = sizes.into_iter().map(|(i, s)| (i, f64::from(s.max(0.0)))).collect();
        let mut extra = 0.0;
        let mut marks = Vec::with_capacity(sorted.len());
        for (i, size) in sorted {
            marks.push((i, size, extra));
            extra += size - default;
        }
        Lines { default, marks }
    }

    /// Where line `i` starts.
    pub fn start(&self, i: u32) -> f64 {
        let before = self.marks.partition_point(|m| m.0 < i);
        let extra = match before.checked_sub(1) {
            Some(k) => {
                let (_, size, e) = self.marks[k];
                e + size - self.default
            }
            None => 0.0,
        };
        f64::from(i) * self.default + extra
    }

    pub fn size(&self, i: u32) -> f64 {
        match self.marks.binary_search_by_key(&i, |m| m.0) {
            Ok(k) => self.marks[k].1,
            Err(_) => self.default,
        }
    }

    pub fn is_hidden(&self, i: u32) -> bool {
        self.size(i) <= 0.0
    }

    /// The line that contains `pos` (the last line when `pos` is past `limit` lines).
    pub fn index_at(&self, pos: f64, limit: u32) -> u32 {
        if pos <= 0.0 || limit == 0 {
            return 0;
        }
        let (mut lo, mut hi) = (0u32, limit - 1);
        while lo < hi {
            let mid = lo + (hi - lo) / 2;
            if self.start(mid + 1) > pos {
                hi = mid;
            } else {
                lo = mid + 1;
            }
        }
        lo
    }

    /// The first line at or after `i` that is not hidden (`i` itself when none after it is).
    pub fn next_visible(&self, i: u32, limit: u32) -> u32 {
        let mut j = i;
        while j + 1 < limit && self.is_hidden(j) {
            j += 1;
        }
        if self.is_hidden(j) { i } else { j }
    }

    /// The nearest visible line before `i` (`i` itself when there is none).
    pub fn prev_visible(&self, i: u32) -> u32 {
        let mut j = i;
        while j > 0 {
            j -= 1;
            if !self.is_hidden(j) {
                return j;
            }
        }
        i
    }

    /// Total length of the first `count` lines.
    pub fn total(&self, count: u32) -> f64 {
        self.start(count)
    }
}

/// Maps lines of one axis to the screen: a frozen part that stays put and a scrolling part.
#[derive(Clone, Debug)]
pub struct Frame<'a> {
    pub lines: &'a Lines,
    /// How many lines at the start are frozen.
    pub freeze: u32,
    /// Where the scrolling part is scrolled to (a sheet offset, never before the frozen length).
    pub scroll: f64,
    pub zoom: f64,
    /// Screen position where line 0 starts (just after the header).
    pub origin: f64,
}

impl Frame<'_> {
    fn frozen_len(&self) -> f64 {
        self.lines.start(self.freeze)
    }

    /// Screen position where line `i` starts.
    pub fn pos(&self, i: u32) -> f32 {
        let at = if i < self.freeze { self.lines.start(i) } else { self.frozen_len() + self.lines.start(i) - self.scroll.max(self.frozen_len()) };
        (self.origin + at * self.zoom).clamp(-1.0e7, 1.0e7) as f32
    }

    pub fn len_of(&self, i: u32) -> f32 {
        (self.lines.size(i) * self.zoom) as f32
    }

    /// Screen position where the scrolling part begins.
    pub fn scroll_start(&self) -> f32 {
        (self.origin + self.frozen_len() * self.zoom) as f32
    }

    /// The line under screen position `p`.
    pub fn index_at(&self, p: f32, limit: u32) -> u32 {
        let rel = (f64::from(p) - self.origin) / self.zoom;
        let frozen = self.frozen_len();
        if self.freeze > 0 && rel < frozen {
            self.lines.index_at(rel, self.freeze.min(limit))
        } else {
            self.lines.index_at(self.scroll.max(frozen) + (rel - frozen), limit)
        }
    }

    /// The scrolling lines that show in `len` screen pixels (from the scroll edge to the end of the view).
    pub fn visible(&self, view_end: f32, limit: u32) -> std::ops::Range<u32> {
        let first = self.lines.index_at(self.scroll.max(self.frozen_len()), limit).max(self.freeze.min(limit));
        let last_off = self.scroll.max(self.frozen_len()) + (f64::from(view_end) - f64::from(self.scroll_start())) / self.zoom;
        let last = self.lines.index_at(last_off, limit).saturating_add(1).min(limit);
        first..last
    }

    pub fn frozen(&self, limit: u32) -> std::ops::Range<u32> {
        0..self.freeze.min(limit)
    }
}

/// The scroll offset that brings line `i` fully into a view of `view_len` pixels (sheet units),
/// changing nothing when it already shows.
pub fn scroll_to_show(lines: &Lines, freeze: u32, scroll: f64, view_len: f64, i: u32) -> f64 {
    if i < freeze {
        return scroll;
    }
    let frozen = lines.start(freeze);
    let scroll = scroll.max(frozen);
    let (start, end) = (lines.start(i), lines.start(i + 1));
    let room = (view_len - frozen).max(1.0);
    if start < scroll {
        start
    } else if end > scroll + room {
        (end - room).max(frozen)
    } else {
        scroll
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines() -> Lines {
        // Default 10; line 2 is 30 wide, line 5 is hidden.
        Lines::new(10.0, [(2, 30.0), (5, 0.0)])
    }

    #[test]
    fn positions_account_for_resized_and_hidden_lines() {
        let l = lines();
        assert_eq!(l.start(0), 0.0);
        assert_eq!(l.start(2), 20.0);
        assert_eq!(l.start(3), 50.0);
        assert_eq!(l.start(5), 70.0);
        assert_eq!(l.start(6), 70.0); // line 5 takes no room
        assert_eq!(l.start(7), 80.0);
        assert_eq!(l.size(2), 30.0);
        assert!(l.is_hidden(5));
        assert!(!l.is_hidden(4));
    }

    #[test]
    fn a_position_finds_its_line_and_skips_hidden_ones() {
        let l = lines();
        assert_eq!(l.index_at(0.0, 100), 0);
        assert_eq!(l.index_at(19.9, 100), 1);
        assert_eq!(l.index_at(20.0, 100), 2);
        assert_eq!(l.index_at(49.9, 100), 2);
        assert_eq!(l.index_at(70.0, 100), 6);
        assert_eq!(l.index_at(-5.0, 100), 0);
        assert_eq!(l.index_at(1.0e9, 100), 99);
        assert_eq!(l.index_at(5.0, 0), 0);
    }

    #[test]
    fn a_million_rows_stay_exact() {
        let l = Lines::new(24.0, [(10, 48.0)]);
        let i = 1_000_000;
        assert_eq!(l.start(i), 24.0 * 1_000_000.0 + 24.0);
        assert_eq!(l.index_at(l.start(i) + 1.0, 1_048_576), i);
    }

    #[test]
    fn stepping_over_hidden_lines() {
        let l = lines();
        assert_eq!(l.next_visible(5, 100), 6);
        assert_eq!(l.prev_visible(6), 4);
        assert_eq!(l.prev_visible(0), 0);
        assert_eq!(l.next_visible(3, 100), 3);
    }

    #[test]
    fn frozen_lines_stay_while_the_rest_scroll() {
        let l = Lines::new(10.0, []);
        let f = Frame { lines: &l, freeze: 2, scroll: 100.0, zoom: 1.0, origin: 50.0 };
        assert_eq!(f.pos(0), 50.0);
        assert_eq!(f.pos(1), 60.0);
        assert_eq!(f.scroll_start(), 70.0);
        assert_eq!(f.pos(10), 70.0); // the line at the scroll offset sits at the scroll edge
        assert_eq!(f.pos(12), 90.0);
        assert_eq!(f.index_at(55.0, 1000), 0);
        assert_eq!(f.index_at(75.0, 1000), 10);
        let v = f.visible(170.0, 1000);
        assert_eq!(v.start, 10);
        assert_eq!(v.end, 21);
        assert_eq!(f.frozen(1000), 0..2);
    }

    #[test]
    fn zoom_scales_screen_positions() {
        let l = Lines::new(10.0, []);
        let f = Frame { lines: &l, freeze: 0, scroll: 0.0, zoom: 2.0, origin: 0.0 };
        assert_eq!(f.pos(3), 60.0);
        assert_eq!(f.len_of(3), 20.0);
        assert_eq!(f.index_at(61.0, 100), 3);
    }

    #[test]
    fn scrolling_to_a_line_moves_only_as_far_as_needed() {
        let l = Lines::new(10.0, []);
        assert_eq!(scroll_to_show(&l, 0, 0.0, 100.0, 5), 0.0);
        assert_eq!(scroll_to_show(&l, 0, 0.0, 100.0, 15), 60.0);
        assert_eq!(scroll_to_show(&l, 0, 200.0, 100.0, 3), 30.0);
        assert_eq!(scroll_to_show(&l, 2, 0.0, 100.0, 1), 0.0);
        assert!(scroll_to_show(&l, 2, 0.0, 100.0, 30) > 0.0);
    }
}
