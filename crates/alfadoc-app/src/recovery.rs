//! Auto-recovery: if Alfadoc stops without being closed (a crash, a power cut), the work since the
//! last Save is not lost.
//!
//! While there are unsaved edits the shell writes a copy of the **whole package** (all three
//! editors, links and the data store) to `<data folder>/Alfadoc/recovery/`, next to a small
//! `.json` note that says what it is and carries a "still running" heartbeat. A normal Save, Open
//! or close removes the copy. On the next start, copies whose heartbeat has stopped are offered
//! back ("Recover unsaved work").
//!
//! Nothing here depends on an engine: the copy is an ordinary `.alfadoc` file.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// How often the shell looks for new unsaved edits.
pub const CHECK_SECS: f64 = 5.0;
/// The least time between two copies.
pub const COPY_SECS: f64 = 30.0;
/// How long after a Save or Open the editors count as settled.
const BASELINE_SECS: f64 = 1.5;
/// A copy whose heartbeat is older than this belongs to a window that is gone.
const STALE_MS: u64 = 120_000;
/// Copies older than this are dropped without asking.
const DROP_AFTER_MS: u64 = 30 * 24 * 3_600_000;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
struct Note {
    id: String,
    /// The file the work came from, if it had been saved.
    original: Option<String>,
    title: String,
    /// When the copy was made (milliseconds since 1970).
    saved_at: u64,
    /// The last time the window that owns the copy said it was alive.
    heartbeat: u64,
}

/// A copy left behind by a window that did not close normally.
#[derive(Clone, Debug)]
pub struct Found {
    pub id: String,
    pub title: String,
    pub original: Option<String>,
    pub saved_at: u64,
    package: PathBuf,
    note: PathBuf,
}

impl Found {
    pub fn read(&self) -> std::io::Result<Vec<u8>> {
        std::fs::read(&self.package)
    }

    /// Deletes the copy.
    pub fn discard(&self) {
        let _ = std::fs::remove_file(&self.package);
        let _ = std::fs::remove_file(&self.note);
    }
}

/// The folder recovery copies live in, or `None` where there is no file system (the browser).
fn folder() -> Option<PathBuf> {
    if cfg!(target_arch = "wasm32") {
        return None;
    }
    let base = if cfg!(windows) {
        std::env::var_os("LOCALAPPDATA").map(PathBuf::from)
    } else if cfg!(target_os = "macos") {
        std::env::var_os("HOME").map(|h| PathBuf::from(h).join("Library").join("Application Support"))
    } else {
        std::env::var_os("XDG_DATA_HOME").map(PathBuf::from).or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local").join("share")))
    };
    Some(base.unwrap_or_else(std::env::temp_dir).join("Alfadoc").join("recovery"))
}

/// Copies left by windows that are gone, newest first.
pub fn found(now_ms: u64) -> Vec<Found> {
    found_in(folder().as_deref(), now_ms, "")
}

fn found_in(dir: Option<&Path>, now_ms: u64, own_id: &str) -> Vec<Found> {
    let Some(dir) = dir else { return vec![] };
    let Ok(entries) = std::fs::read_dir(dir) else { return vec![] };
    let mut out = vec![];
    for entry in entries.flatten() {
        let note_path = entry.path();
        if note_path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let Some(note) = std::fs::read(&note_path).ok().and_then(|b| serde_json::from_slice::<Note>(&b).ok()) else { continue };
        let package = note_path.with_extension("alfadoc");
        if note.id == own_id || !package.exists() {
            continue;
        }
        let item = Found { id: note.id.clone(), title: note.title.clone(), original: note.original.clone(), saved_at: note.saved_at, package, note: note_path };
        if now_ms.saturating_sub(note.saved_at) > DROP_AFTER_MS {
            item.discard();
        } else if now_ms.saturating_sub(note.heartbeat) > STALE_MS {
            out.push(item);
        }
    }
    out.sort_by_key(|f| std::cmp::Reverse(f.saved_at));
    out
}

/// "just now", "5 min ago", "3 h ago", "2 d ago".
pub fn ago(now_ms: u64, then_ms: u64) -> String {
    let secs = now_ms.saturating_sub(then_ms) / 1000;
    match secs {
        0..=59 => "just now".to_string(),
        60..=3_599 => format!("{} min ago", secs / 60),
        3_600..=86_399 => format!("{} h ago", secs / 3_600),
        _ => format!("{} d ago", secs / 86_400),
    }
}

pub struct Recovery {
    id: String,
    dir: Option<PathBuf>,
    /// What the editors looked like when the work was last known to be saved.
    clean: Option<u64>,
    last_check: f64,
    last_copy: f64,
    last_beat: f64,
    /// A Save or Open happened: the next look records the new "clean" state.
    reset: bool,
    /// When (egui clock) the pending reset was first noticed: the editors get a moment to settle
    /// before their state counts as the saved one.
    reset_seen: Option<f64>,
    wrote: bool,
}

impl Recovery {
    pub fn new(id: String) -> Self {
        Recovery { id, dir: folder(), clean: None, last_check: 0.0, last_copy: f64::NEG_INFINITY, last_beat: 0.0, reset: true, reset_seen: None, wrote: false }
    }

    /// Whether it is time to look at the editors again (and, if so, restarts the wait).
    pub fn due(&mut self, now: f64) -> bool {
        if self.dir.is_none() || now - self.last_check < CHECK_SECS {
            return false;
        }
        self.last_check = now;
        true
    }

    /// The work was saved, or a package was opened: forget the copy and watch for new edits.
    pub fn saved(&mut self) {
        self.reset = true;
        self.reset_seen = None;
        self.remove_files();
    }

    /// Looks at `signature` (a number that changes with every edit). Returns true when a copy
    /// should be written now; call [`write`](Self::write) with its bytes.
    pub fn wants_copy(&mut self, now: f64, signature: u64) -> bool {
        if self.reset {
            self.reset = false;
            self.clean = Some(signature);
            return false;
        }
        if self.clean == Some(signature) {
            return false;
        }
        now - self.last_copy >= COPY_SECS
    }

    pub fn write(&mut self, now: f64, now_ms: u64, bytes: &[u8], original: Option<&Path>, title: &str) -> Result<(), String> {
        let dir = self.dir.as_ref().ok_or("no folder for recovery copies")?;
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        let package = dir.join(format!("{}.alfadoc", self.id));
        let tmp = dir.join(format!("{}.alfadoc.tmp", self.id));
        std::fs::write(&tmp, bytes).map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, &package).map_err(|e| e.to_string())?;
        self.write_note(now_ms, Some(now_ms), original, title)?;
        self.last_copy = now;
        self.last_beat = now;
        self.wrote = true;
        Ok(())
    }

    /// Keeps the "still running" heartbeat fresh while a copy exists.
    pub fn beat(&mut self, now: f64, now_ms: u64) {
        if !self.wrote || now - self.last_beat < COPY_SECS {
            return;
        }
        self.last_beat = now;
        let Some(dir) = &self.dir else { return };
        let note_path = dir.join(format!("{}.json", self.id));
        if let Some(mut note) = std::fs::read(&note_path).ok().and_then(|b| serde_json::from_slice::<Note>(&b).ok()) {
            note.heartbeat = now_ms;
            if let Ok(text) = serde_json::to_vec(&note) {
                let _ = std::fs::write(&note_path, text);
            }
        }
    }

    fn write_note(&self, heartbeat: u64, saved_at: Option<u64>, original: Option<&Path>, title: &str) -> Result<(), String> {
        let dir = self.dir.as_ref().ok_or("no folder for recovery copies")?;
        let note = Note { id: self.id.clone(), original: original.map(|p| p.to_string_lossy().into_owned()), title: title.to_string(), saved_at: saved_at.unwrap_or(heartbeat), heartbeat };
        let text = serde_json::to_vec(&note).map_err(|e| e.to_string())?;
        std::fs::write(dir.join(format!("{}.json", self.id)), text).map_err(|e| e.to_string())
    }

    /// True when the saved state still has to be recorded and the editors have had a moment to
    /// settle since the Save or Open. Call [`set_baseline`](Self::set_baseline) then.
    pub fn baseline_due(&mut self, now: f64) -> bool {
        if !self.reset {
            return false;
        }
        let since = *self.reset_seen.get_or_insert(now);
        now - since >= BASELINE_SECS
    }

    /// Whether a Save or Open is still waiting for its state to be recorded.
    pub fn baseline_pending(&self) -> bool {
        self.reset
    }

    /// Records `signature` as the saved state.
    pub fn set_baseline(&mut self, signature: u64) {
        self.reset = false;
        self.reset_seen = None;
        self.clean = Some(signature);
    }

    /// Whether the work differs from what was last saved or opened.
    pub fn is_dirty(&self, signature: u64) -> bool {
        !self.reset && self.clean != Some(signature)
    }

    /// Treats the current state as unsaved (after work was recovered), so a new copy follows.
    pub fn mark_unsaved(&mut self) {
        self.reset = false;
        self.clean = None;
    }

    pub fn remove_files(&mut self) {
        if let Some(dir) = &self.dir {
            let _ = std::fs::remove_file(dir.join(format!("{}.alfadoc", self.id)));
            let _ = std::fs::remove_file(dir.join(format!("{}.json", self.id)));
        }
        self.wrote = false;
    }
}

impl Drop for Recovery {
    /// A normal close removes the copy. A panic does not: that is the case it exists for.
    fn drop(&mut self) {
        if !std::thread::panicking() {
            self.remove_files();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("alfadoc-recovery-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn recovery_in(dir: &Path, id: &str) -> Recovery {
        let mut r = Recovery::new(id.into());
        r.dir = Some(dir.to_path_buf());
        r
    }

    #[test]
    fn a_copy_is_written_only_after_an_edit_and_not_too_often() {
        let dir = temp("edit");
        let mut r = recovery_in(&dir, "a");
        assert!(!r.wants_copy(100.0, 1), "the first look only records the clean state");
        assert!(!r.wants_copy(110.0, 1), "no edit, no copy");
        assert!(r.wants_copy(120.0, 2), "an edit gets a copy");
        r.write(120.0, 1_000, b"x", None, "Doc").unwrap();
        assert!(!r.wants_copy(125.0, 3), "but not within the wait");
        assert!(r.wants_copy(151.0, 3));
        r.saved();
        assert!(!dir.join("a.alfadoc").exists());
        assert!(!r.wants_copy(300.0, 9), "after a save the new state is the clean one");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn only_copies_of_windows_that_stopped_are_offered() {
        let dir = temp("found");
        let mut dead = recovery_in(&dir, "dead");
        dead.write(0.0, 1_000, b"old", Some(Path::new("C:/a.alfadoc")), "Budget").unwrap();
        let mut live = recovery_in(&dir, "live");
        live.write(0.0, 1_000_000, b"new", None, "Other").unwrap();
        let items = found_in(Some(&dir), 1_000_100, "me");
        assert_eq!(items.len(), 1, "the live window's copy is not offered");
        assert_eq!(items[0].id, "dead");
        assert_eq!(items[0].original.as_deref(), Some("C:/a.alfadoc"));
        assert_eq!(items[0].read().unwrap(), b"old");
        assert!(found_in(Some(&dir), 1_000_100, "dead").is_empty(), "a window never offers its own copy");
        items[0].discard();
        assert!(!dir.join("dead.alfadoc").exists());
        std::mem::forget(dead);
        std::mem::forget(live);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn unsaved_changes_are_told_from_the_saved_state() {
        let mut r = recovery_in(&temp("dirty"), "d");
        assert!(!r.is_dirty(7), "nothing is recorded yet, so nothing counts as changed");
        assert!(!r.baseline_due(10.0), "the editors get a moment to settle");
        assert!(r.baseline_due(11.6));
        r.set_baseline(7);
        assert!(!r.is_dirty(7));
        assert!(r.is_dirty(8), "an edit changes the signature");
        r.saved();
        assert!(!r.is_dirty(8), "after a Save the next state is recorded again");
        assert!(!r.baseline_due(20.0));
        assert!(r.baseline_due(21.6));
        r.set_baseline(8);
        r.mark_unsaved();
        assert!(r.is_dirty(8), "recovered work is unsaved");
    }

    #[test]
    fn ages_read_naturally() {
        assert_eq!(ago(10_000, 9_000), "just now");
        assert_eq!(ago(400_000, 100_000), "5 min ago");
        assert_eq!(ago(7_300_000, 0), "2 h ago");
        assert_eq!(ago(200_000_000, 0), "2 d ago");
    }

    #[test]
    fn the_heartbeat_keeps_a_running_window_from_looking_dead() {
        let dir = temp("beat");
        let mut r = recovery_in(&dir, "w");
        r.write(0.0, 1_000, b"x", None, "Doc").unwrap();
        r.beat(40.0, 500_000);
        assert!(found_in(Some(&dir), 500_100, "").is_empty());
        assert_eq!(found_in(Some(&dir), 900_000, "").len(), 1, "once the beats stop it counts as left behind");
        std::mem::forget(r);
        std::fs::remove_dir_all(&dir).ok();
    }
}
