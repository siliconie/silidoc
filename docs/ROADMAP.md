# Roadmap

Ordered by dependency. Each milestone ends with something that runs and is tested.

## M0 — Foundations ✅ done
- Workspace, lints, CI.
- `alfadoc-model`: `.alfadoc` container (bounded reader, zip-slip checks), maix JSON-records store with
  history and metadata cascade, dimension vectors and hashing with collision handling, link registry.
- `alfadoc-sync`: three-way merge, one-way sync, debouncer, echo guard.
- `alfadoc-bridge`: `Engine` / `RegionAccess` traits, `alfadoc.*` dispatcher, `sync_link`.
- Docs: this set. maix 1.1 errata.

## M1 — Engines in one window
1. Engines are sibling path dependencies (done in this layout); `alfadoc-engines` holds the adapters and the first contract tests.
2. **Spike:** can `WordApp`, `SheetApp` and `SlideApp` be shown in tabs of one egui app? Measure font-database memory and startup. Decide shared-fonts work from the numbers.
3. `alfadoc-adapter-grid` first (`RegionAccess` over GridCraft; maix ↔ sheet). Then word, then deck.
4. `alfadoc-app`: desktop shell, tabs, drawer, toasts; open/save `.alfadoc`; web build via Trunk.

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
