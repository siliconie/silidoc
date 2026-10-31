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
- ✅ Dispatcher-driven debounced sync; per-cell conflict dialog (Links ▸ Resolve…, keep A or B for each cell or for all); linked Data ranges tinted in the Data tab; Data ends can be picked from the sheet list or from the Data tab selection; one sync pass is one undo step in the document and on slides (the sheet already did this); dates written to the sheet get a date format and read back as dates.
- ✅ Inline fields: a Document end can be "Words in a sentence" (the selected words become a plain-text content control tagged with the link; it is a one-cell region). Document and Slides tables are chosen from a list of every table, not only the selected one.

## M3 — External files
- Import `.xlsx`, `.docx`, `.pptx` as linked sources; file watcher + echo guard.
- Patch-in-place writers: xlsx cells first, then docx content controls, then pptx shapes.
- Lock handling and approval of external paths; web: File System Access API path.

## M4 — Convert and extend maix
- Export to `.docx`/`.xlsx`/`.pptx` with live content flattened to values; link tags preserved.
- ✅ In the Data tab: formulas (`=B2*2`, `SUM`, `AVERAGE`, `MIN`, `MAX`, `COUNT`, `ABS`, `ROUND`), the `@{…}` suffix, `DIMSUM`, `DIMAVG`, `DIMCOUNT`, `DIMSLICE`, and a Dimensions menu that chooses which vector the grid shows and writes (`alfadoc-model::formula`).
- ✅ Data tab at GridCraft's standard (m13): 1,048,576 × 16,384 cells, resizable/hidable rows and columns, frozen panes, zoom, context menus, menu bar and toolbar, cell styles and Excel number formats (`gridcraft-numfmt`), insert/delete rows and columns with formula and link adjustment, sort, fill, in-app clipboard with formulas and formats, undo for all of it. Row/column/style data is saved per sheet under `format` in `data/maix.json`. 
- ✅ Data tab, second pass (m14): find/replace, filters with drop-downs, fill handle with number/date series, paste-special dialog (values/formulas/formats, skip blanks, transpose), compare vectors side by side. Merged cells, conditional formatting, data validation, comments and grouping followed in m16.
- ✅ m15: slide variants (mark shapes, show only the shapes for the current dimensions) and Compare vectors ▸ Put on a new sheet (which is also the way to get DIMSUM-style results into a GridCraft sheet: compare or compute in Data, then link).
- ✅ Links tab (Alt+5): list with status and filter, details of both ends, live cell-by-cell comparison, rename, direction, pause, sync once, make one end equal the other, swap, remove, conflicts and a session activity list. Link records gained `name`, `paused`, `created` (optional in the file).
- ✅ Variant blocks in the document: Links ▸ Variant blocks marks the selected words with the current dimensions (a rich-text content control tagged `alfadoc:variant=…`) and "Keep only these blocks" deletes the blocks for other values (one Undo restores them). The editors cannot *hide* content, so this is a flatten, not a live view. Slides do not have it yet.
- ✅ m16: merged cells, conditional formatting (rules and colour scales), data validation (lists, whole, decimal, text length), comments and row/column grouping in the Data tab (`alfadoc-model::extras`); the "Save changes?" question on close and before a dropped file replaces unsaved work; drag and drop of `.alfadoc` and Office files; an Export window with PDF and "only the current dimensions" copies of a document or slides; the paged maix store (`maix/1.1-paged`, `alfadoc-model::paged`) for stores of 200,000+ cells; the Dimensions menu now stays open while you type a dimension or value.
- ✅ m17: the Dimensions menu moved to the Data menu bar and lists every created dimension and value (declared values are saved in `axes[].values`); first 3D view in the Data tab (Data ▸ 3D view…). 3D for the Sheet and Slides tabs, surfaces and lines are not started.
- ✅ Passphrase protection: Protect ▸ encrypt this document (every Save and recovery copy is encrypted), unlock on Open, sign a copy, check a signature (`alfadoc-model::security`, `protect.rs`).
- Not started: `DIMSUM` and friends typed inside the *sheet's* own formulas (GridCraft has no function hook, and the engines stay untouched; compute them in the Data tab and link the cells instead).

## Not planned
- A mobile app. Whole-body two-way sync of free text between `.docx`/`.pptx` and Alfadoc.

**Import / Export (tab bar).** Import copies an .xlsx/.docx/.pptx (also .xls, .ods, .csv, .doc, .odt, .rtf) into its editor; Export writes the current editor's document as .xlsx, .docx or .pptx. Remaining in M3: external files kept linked and watched on disk; M4: round-trip tests. PDF export and the Export window are in (m16).

**Cross-crate tests (`alfadoc-tests`).** `cargo test -p alfadoc-tests -j 2` runs links across the real engines (sheet ↔ data store ↔ slide table: edits flow both ways, a quiet pass writes nothing, a clear clears everywhere, a conflict is left alone) and checks that link anchors and data survive `.xlsx`, `.pptx`, `.docx` and `.alfadoc` round trips. Run it after every engine update (docs/UPSTREAM.md).
