# Roadmap

Ordered by dependency. Each milestone ends with something that runs and is tested.

## M0 — Foundations ✅ done
- Workspace, lints, CI.
- `alfadoc-model`: `.alfadoc` container (bounded reader, zip-slip checks), maix JSON-records store with
  history and metadata cascade, dimension vectors and hashing with collision handling, link registry.
- `alfadoc-sync`: three-way merge, one-way sync, debouncer, echo guard.
- `alfadoc-bridge`: `Engine` / `RegionAccess` traits, `alfadoc.*` dispatcher, `sync_link`.
- Docs: this set. maix 1.1 errata.

## M1 — Engines in one window (in progress)
1. ✅ Engines are sibling path dependencies; the full workspace (three engines + UI stack) builds and passes on Windows.
2. ✅ `alfadoc-adapter-grid`: `Engine` + `RegionAccess` over GridCraft, anchored by a defined name; writes go through `range.setValues` / `cell.set` in one undo group; contract tests pin every command and field used. The `Engine` trait gained an optional `revision()` (GridCraft and DeckCraft expose one), which the dispatcher prefers over command flags.
3. ✅ `alfadoc-adapter-deck` + `.alfadoc` Open/Save in the window (Open, Save, Save As in the tab bar; Ctrl+Alt+S / Ctrl+Alt+O). Each editor's document is stored as xlsx/docx/pptx inside the package. Done: Word adapter (`alfadoc-adapter-word`, tables tagged through Alt Text) and live links (Links window in the tab bar, change watching with a quiet period, three-way merge, conflicts left for the user). Original note: (tables on slides, anchored by shape name/tag; DeckCraft has `revision` and shape `tags`), then `alfadoc-adapter-word` (content-control tables). maix ↔ sheet adapter for the in-file store.
4. 🔶 **Spike / first window:** `alfadoc-app` shows `SheetApp`, `WordApp` and `SlideApp` in tabs of one egui window (docs/SHELL.md). Written, awaiting its first run: measure start-up and memory of the Document tab and decide shared-fonts work from the numbers.
5. `alfadoc-app`: desktop shell, tabs, drawer, toasts; open/save `.alfadoc`; web build via Trunk.

## M2 — Live links inside one file
- Create a link from a selection (`alfadoc.link.create`): maix range ↔ Word table ↔ slide table, all directions.
- Dispatcher-driven debounced sync; conflict toast with a resolve dialog (a/b/both).
- Inline fields (single-cell links in a sentence).

## M3 — External files
- Import `.xlsx`, `.docx`, `.pptx` as linked sources; file watcher + echo guard.
- Patch-in-place writers: xlsx cells first, then docx content controls, then pptx shapes.
- Lock handling and approval of external paths; web: File System Access API path.

## M4 — Convert and extend maix
- Export to `.docx`/`.xlsx`/`.pptx` with live content flattened to values; link tags preserved.
- Variant blocks (dimensions on document and slide nodes); `DIMSUM`/`DIMSLICE` and the `@{…}` suffix in formulas.
- Paged maix profile if stores outgrow memory; namespace encryption and signature.

## Not planned
- A mobile app. Whole-body two-way sync of free text between `.docx`/`.pptx` and Alfadoc.

**Import / Export (tab bar).** Import copies an .xlsx/.docx/.pptx (also .xls, .ods, .csv, .doc, .odt, .rtf) into its editor; Export writes the current editor's document as .xlsx, .docx or .pptx. Remaining in M3: external files kept linked and watched on disk; M4: PDF export, round-trip tests.

**Cross-crate tests (`alfadoc-tests`).** `cargo test -p alfadoc-tests -j 2` runs links across the real engines (sheet ↔ data store ↔ slide table: edits flow both ways, a quiet pass writes nothing, a clear clears everywhere, a conflict is left alone) and checks that link anchors and data survive `.xlsx`, `.pptx`, `.docx` and `.alfadoc` round trips. Run it after every engine update (docs/UPSTREAM.md).
