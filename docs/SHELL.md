# The Alfadoc window (alfadoc-app)

`cargo run -p alfadoc-app` opens one window with three tabs: **Sheet** (GridCraft), **Document**
(WordCraft) and **Slides** (DeckCraft). Each tab is the engine's complete editor, not a look-alike.
`alfadoc --tab doc` starts on the Document tab; Alt+1/2/3 switch tabs.

This is the M1 shell. It hosts the editors; it does not yet open or save `.alfadoc` files or link
data between tabs (M2).

## How three engines share one window

| Problem | What the shell does |
|---|---|
| WordCraft opens a native OS window per document on desktop. | `egui::Context::set_embed_viewports(true)`: egui's own switch to draw extra windows inside the one window. WordCraft already handles this path (it shows the active document alone). |
| All three engines install their own fonts, colours and zoom on the one egui context, and WordCraft caches "theme applied". | `ContextState` snapshots the context when a tab is left and restores it when the tab returns. Nothing inside an engine is touched. |
| Start-up cost: WordCraft's font database is large. | Engines are created lazily, on the first visit to their tab. |
| Input and per-frame work. | Only the active engine gets `logic`, `ui` and `raw_input_hook`. |

## Not connected yet (each engine's own app wires these; the editors work without them)

- Saved preferences, update checks, the local control channel.
- A combined "unsaved changes" guard when the window closes: only the visible tab's guard runs.
- Drag and drop of files, opening files from the command line.

## Connected on native builds

- System clipboard (text and HTML), printing (Sheet and Document) and slide audio, wired in `platform.rs` and `hosts.rs`. The web build keeps egui's clipboard only.
- Auto-recovery (`recovery.rs`): while there are unsaved edits a full `.alfadoc` copy is written every 30 s to `%LOCALAPPDATA%\Alfadoc\recovery` (or the platform equivalent) with a heartbeat note. After a crash the next start offers "Recover unsaved work". A normal exit removes the copy.
- Data tab: a virtualised grid editor (selection, fx bar, copy/paste TSV, undo/redo, sheet tabs, rename) painted from the theme colours.

## Known risks to look at first when you run it

1. Colours or fonts wrong after switching tabs: the snapshot/restore in `ctxstate.rs`.
2. A Word dialog or document window that does not draw in place: the embed switch in `main.rs`.
3. Window title not following the active document: each engine sets the title for its own document.
4. Start-up time and memory of the Document tab: this is the measurement the roadmap's spike asks for.

## Links

The **Links** button opens a window to join two editors. Each end is one of:

* **Sheet**: a sheet name and range (the *Use selection* button fills them from the current selection). The range becomes a workbook defined name `alfadoc_<id>`.
* **Document**: the table the caret is in. Its Alt Text description is set to `alfadoc:link=<id>`.
* **Slides**: the table selected on the current slide. It is renamed `alfadoc_<id>` in the Selection Pane.

Both tags are stored in the files, so a link survives a round trip through Excel, Word and PowerPoint. On creation, end A's content wins. After that the shell watches the editors: when something changes and stays quiet for 0.6 s, each link is merged three ways against its last agreed contents and only the difference is written, through each editor's own commands (Undo works on it). A cell changed on both sides is left alone and shown as a conflict, with **Keep A** / **Keep B** in the window. Links are saved with the `.alfadoc` file.

Limits for now: the two ends must be in different editors; both editors must have been opened this session; each changed cell is one undo step in Document and Slides; dates reach a sheet as serial numbers.

### Data end (maix)

Either end of a link can also be **Data**: a named sheet and range of the shared maix store inside the package (for example `Costs`, `A1:C10`). Link a sheet range to Data, then a slide table to the same Data range, and the sheet and the slide stay equal through it, with every change recorded in the maix history. The data store is saved with the `.alfadoc` file.

### Data tab (Alt+4)

The **Data** tab shows the shared store as a grid. Pick or add a data sheet, type into any cell, and every link attached to those cells updates its sheet, document table or slide table once typing stops. `12` is stored as a number, `007` as text. Cells cleared here are stored as empty (the store keeps its history).

### File end (an Excel, Word or PowerPoint file on disk)

An end can also be **File**: an `.xlsx`, `.docx` or `.pptx` file anywhere on disk.

* **Workbook:** give a sheet and a range. Alfadoc stores a name for the range inside the file.
* **Word or PowerPoint file:** give a **table number** (1 = the first table in reading order). Alfadoc tags that table (Word: the table's Alt Text; PowerPoint: the shape name) so the link finds it again even if tables are added before it.

Alfadoc opens the file in a private session, keeps the table or range equal to the other end, and writes the file back when a sync changed it. When Excel, Word or PowerPoint (or anything else) saves the file, Alfadoc reloads it and syncs. The path is saved in the `.alfadoc` file; the file is read again when the package is opened.

Alfadoc writes the file with the engine's own writer (GridCraft, WordCraft or DeckCraft), so features the engine does not model can be lost the first time. A copy of the original is kept beside it as `<name>.alfadoc-backup.<ext>` before that first write. Close the file in its Office program while Alfadoc links to it, or the program may hold it locked. (`.xlsm` is not linkable: the writer would drop the macros.)

### Several documents per editor

Each editor can have several documents open. **Save** stores all of them in the package (the active one as `main.<ext>`, the others as `doc2.<ext>`, …) and **Open** brings them all back. **Open** and **Import** replace a blank, untouched workbook or presentation instead of adding a second one. Links act on the *active* document of each editor.
