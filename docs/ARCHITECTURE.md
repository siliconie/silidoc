# Architecture

## Goals

1. One application and one file format (`.alfadoc`) that does what Word, Excel and PowerPoint do.
2. Data entered anywhere can appear everywhere: a spreadsheet range as a table in a document, a document value in a slide, and back.
3. Imported `.xlsx`, `.docx` and `.pptx` files stay linked: edits in Alfadoc flow into the file and edits in the file flow in, within a second or two.
4. Export to Word, Excel and PowerPoint formats at any time.
5. Desktop and web from one codebase. Upstream engines keep evolving and we keep taking their updates.

## Layers

```
┌────────────────────────────────────────────────────────────────────────┐
│ alfadoc-app            host window: tabs, drawer, toasts, file watcher  │  (grid: done; deck, word: M1)
├────────────────────────────────────────────────────────────────────────┤
│ alfadoc-adapter-word / -grid / -deck     implement Engine + RegionAccess │  (M1+)
├────────────────────────────────────────────────────────────────────────┤
│ alfadoc-bridge   traits, alfadoc.* dispatcher, sync_link                 │  done
│ alfadoc-sync     merge3, one_way, Debouncer, EchoGuard                   │  done
│ alfadoc-model    .alfadoc container, maix store, dims, links             │  done
╞════════════════════════════════════════════════════════════════════════╡
│  your code above  ·  unmodified upstream code below (sibling path deps)    │
├────────────────────────────────────────────────────────────────────────┤
│ wordcraft-*      gridcraft-*      deckcraft-*      craft-libs            │
└────────────────────────────────────────────────────────────────────────┘
```

The three bottom crates have **no dependency on any engine**. They compile and test on their own,
which keeps the data layer portable (including to WebAssembly) and keeps upstream churn out of it.

## Why this shape

The three engines are built the same way, which is what makes this feasible:

- **Every action is a command.** Each engine routes all edits through one `execute(id, json)` entry
  point with stable string ids (`cell.set`, `format.bold`, …). Adapters call that; they do not reach
  into engine internals. The command ids are the contract.
- **Knowing that something changed differs per engine.** WordCraft publishes a `mutates` flag per
  command. GridCraft and DeckCraft publish only a coarse `is_dirty()`. Their adapters therefore treat
  every command except known queries as a *possible* change and let the debounced `sync_link` pass
  decide, which is cheap because it writes nothing when nothing differs. An upstream hook that exposes a
  revision counter (PATCHES.md) would make this exact.
- **The UI is a thin layer.** `WordApp`, `SheetApp` and `SlideApp` each expose
  `pub fn ui(&mut self, ui: &mut egui::Ui)`, so one host window can show any of them in a tab. *Whether they
  behave when embedded together (window chrome, fonts, focus) is the first thing the M1 spike verifies.*
- **Each format has a native way to mark a region** that survives export: Word content controls
  (`tag`), spreadsheet defined names, slide-shape names and tags. Links use these (LINKS-AND-SYNC).

## Data flow

```
 external .xlsx ──watch──▶ import ─▶ GridCraft session ◀──┐
                                          ▲  │            │ RegionAccess (read/write cells
 external .docx ──watch──▶ import ─▶ WordCraft session    │  through engine commands)
                                          ▲  │            │
 external .pptx ──watch──▶ import ─▶ DeckCraft session ◀──┤
                                                          │
                       maix store (data/maix.json) ◀──────┘     sync_link(link, a, b, policy)
                                                                three-way merge against link.base
```

Every link has two endpoints. `sync_link` reads both, merges each cell against the last-synced
snapshot, writes **only the cells that changed** through the owning engine's commands (so undo,
journaling and recalculation keep working), then advances the snapshot. If a write fails, the
snapshot does not advance and the next attempt re-merges from the same base.

## One window, three engines

The host owns the window, the document list, the drawer navigation and the sync toasts. Each open
`.alfadoc` holds up to three engine sessions (document, workbook, deck). Switching tabs switches
which `ui()` is drawn. Commands go through the bridge `Dispatcher`: `alfadoc.*` commands are ours,
everything else goes to the active engine, and mutating commands are recorded so the sync layer
knows when to look at linked regions.

## Known risks (tracked in the roadmap)

| Risk | Mitigation |
|---|---|
| Three engines in one binary must agree on egui, winit, krilla and `craft-libs` | Alfadoc's root carries the union of the engines' `[patch.crates-io]` tables (Cargo ignores patches outside the root); checked at resolution time today: one egui, one craft-libs rev; re-checked on every update (UPSTREAM.md) |
| Each engine loads its own font database | Measure in the M1 spike; share one database through `craft-fonts` if the cost is real |
| Engine command ids or types change upstream | Contract tests (UPSTREAM.md); the old pinned rev stays until they pass |
| Writing into real Office files could lose data the engines do not model | Patch-in-place writer, never a full rewrite (LINKS-AND-SYNC) |
| Web cannot watch folders or silently rewrite files | Web sync is import/export plus the File System Access API in Chromium |
| Upstream registries (GridCraft, DeckCraft) cannot take outside commands | `alfadoc.*` is handled by our dispatcher; upstream PR for a registration hook |
