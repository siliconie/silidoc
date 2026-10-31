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
- Data tab: a virtualised grid editor (selection, fx bar, copy/paste TSV, undo/redo, sheet tabs, rename) painted from the theme colours; formulas starting with `=` and a Dimensions menu (see MAIX-1.1.md); ranges that links keep in step are tinted.
- Links: conflicts are settled cell by cell in a Resolve dialog.

## Known risks to look at first when you run it

1. Colours or fonts wrong after switching tabs: the snapshot/restore in `ctxstate.rs`.
2. A Word dialog or document window that does not draw in place: the embed switch in `main.rs`.
3. Window title not following the active document: each engine sets the title for its own document.
4. Start-up time and memory of the Document tab: this is the measurement the roadmap's spike asks for.

## Links

The **Link…** button opens a window to join two editors (the **Links** tab, Alt+5, looks after the links that exist; see below). Each end is one of:

* **Sheet**: a sheet name and range (the *Use selection* button fills them from the current selection). The range becomes a workbook defined name `alfadoc_<id>`.
* **Document**: the table the caret is in. Its Alt Text description is set to `alfadoc:link=<id>`.
* **Slides**: the table selected on the current slide. It is renamed `alfadoc_<id>` in the Selection Pane.

Both tags are stored in the files, so a link survives a round trip through Excel, Word and PowerPoint. On creation, end A's content wins. After that the shell watches the editors: when something changes and stays quiet for 0.6 s, each link is merged three ways against its last agreed contents and only the difference is written, through each editor's own commands (Undo works on it). A cell changed on both sides is left alone and shown as a conflict, with **Keep A** / **Keep B** in the window. Links are saved with the `.alfadoc` file.

Limits for now: the two ends must be in different editors; both editors must have been opened this session; each changed cell is one undo step in Document and Slides; dates reach a sheet as serial numbers.

### Data end (maix)

Either end of a link can also be **Data**: a named sheet and range of the shared maix store inside the package (for example `Costs`, `A1:C10`). Link a sheet range to Data, then a slide table to the same Data range, and the sheet and the slide stay equal through it, with every change recorded in the maix history. The data store is saved with the `.alfadoc` file.

### Links tab (Alt+5)

A list of every link with its status (in step, conflicts, problem, paused, waiting for an editor, never synced) and a filter, and for the selected link:

- a **name** you can give it, the id, when it was made and last synced;
- both **ends**: place, part or file, and the anchor (data sheet and range, defined name, content-control tag, slide and shape), what was read from it (size, cells with content) or why it could not be read, and **Show in editor**;
- **Direction** (both ways, A to B, B to A) and **Paused** (a paused link is skipped by automatic syncing; **Sync once** still works);
- **Sync once**, **Refresh**, **Swap A ↔ B**, **Make B equal A** / **Make A equal B** (every differing cell is copied; asks twice) and **Remove link** (asks twice);
- the **conflicts** with Keep A / Keep B per cell or for all;
- a **comparison** of the two ends cell by cell with the last agreed value and what the next sync will do with the cell (A changed → goes to B, conflict, …), optionally only the differences, up to 300 rows;
- the **activity** of this session (not saved in the file).

Names, the paused switch and the creation time are saved with the link (`links.json`: `name`, `paused`, `created`; older files open with empty values). Creating links stays in the Link… window.

### Data tab (Alt+4)

The **Data** tab shows the shared store as a grid with GridCraft's limits (1,048,576 rows by 16,384 columns). Pick or add a data sheet, type into any cell, and every link attached to those cells updates its sheet, document table or slide table once typing stops. `12` is stored as a number, `007` as text, `=` starts a formula. Cells cleared here are stored as empty (the store keeps its history).

- **Sizing:** drag the edge of a row or column header to resize (all selected lines at once when whole lines are selected); double-click the edge to fit the content. Right-click ▸ Row height / Column width for exact values; Hide / Unhide on headers.
- **Navigation:** wheel and scroll bars (they grow as you approach the end), Ctrl+wheel to zoom 40–300 %, Ctrl+arrows to jump between blocks, Ctrl+Home/End, Ctrl+Space / Shift+Space to select a column / row, and the address box (type `B7`, `A1:D20`, `C:C`, `3:5`; also Ctrl+G). View ▸ Freeze panes keeps the rows above and columns left of the active cell in place.
- **Menus:** a menu bar (Edit, Insert, Format, Data, View), a formatting toolbar (undo/redo, number format with +/- decimals in Excel format codes, font size, bold/italic/underline/strike, text and fill colour, alignment, wrap), and right-click menus on cells, row headers, column headers, the corner and sheet tabs (rename, duplicate, move, delete).
- **Rows and columns:** insert and delete (Ctrl++ / Ctrl+-); formulas that point at moved cells are rewritten (`=A3` becomes `=A5`, a deleted target becomes `#REF!`) and links that cover the range follow it. Sort A→Z / Z→A / range with a key column; formulas travel with their rows. Fill down / right (Ctrl+D / Ctrl+R) moves relative references.
- **Clipboard:** inside the window copy/cut/paste keeps formulas (relative references move) and formatting; Paste special offers values, formulas or formats only. Text from other programs pastes as tab-separated values. The Paste menu entry only sees the in-window clipboard: use Ctrl+V for text from elsewhere.
- **Undo** covers values, formats, sizes, hiding, freezing, inserts, deletes and sorts. Deleting a sheet cannot be undone and asks first. Undoing an insert or delete moves linked ranges back by the opposite shift, which is not an exact inverse when the range had shrunk.
- **Find and replace** (Ctrl+F or Ctrl+H, Edit menu): next/previous, replace, replace all, match case, whole cell, look in formulas, selection only. Computed values are not replaced; search formulas to change a formula.
- **Filters** (Data ▸ Create a filter, Ctrl+Shift+L): put a filter on the selection (the first row holds the titles) and each title gets a drop-down with a search box, tick boxes with counts, and sort A→Z / Z→A. Filtered-out rows use the sheet's hidden rows, so they save and undo with it. Inserting or deleting rows or columns removes the filter and shows its rows again.
- **Fill handle:** drag the small square at the corner of the selection down or right. Two or more numbers or dates continue as a series (1, 2 → 3, 4, 5); anything else repeats, formulas move their relative references and styles are copied.
- **Paste special…** (Edit ▸ Paste special): everything, values, formulas or formatting only, skip blanks, and transpose.
- **Compare vectors** (Data menu): the selected cells under two or more values of one dimension side by side (actual, budget), with the difference and the change in percent for two; Copy table gives tab-separated text.
- Not there yet: merged cells, conditional formatting, data validation, comments, grouping, double-click on the fill handle to fill as far as the neighbouring column goes.

### File end (an Excel, Word or PowerPoint file on disk)

An end can also be **File**: an `.xlsx`, `.docx` or `.pptx` file anywhere on disk.

* **Workbook:** give a sheet and a range. Alfadoc stores a name for the range inside the file.
* **Word or PowerPoint file:** give a **table number** (1 = the first table in reading order). Alfadoc tags that table (Word: the table's Alt Text; PowerPoint: the shape name) so the link finds it again even if tables are added before it.

Alfadoc opens the file in a private session, keeps the table or range equal to the other end, and writes the file back when a sync changed it. When Excel, Word or PowerPoint (or anything else) saves the file, Alfadoc reloads it and syncs. The path is saved in the `.alfadoc` file; the file is read again when the package is opened.

Alfadoc writes the file with the engine's own writer (GridCraft, WordCraft or DeckCraft), so features the engine does not model can be lost the first time. A copy of the original is kept beside it as `<name>.alfadoc-backup.<ext>` before that first write. Close the file in its Office program while Alfadoc links to it, or the program may hold it locked. (`.xlsm` is not linkable: the writer would drop the macros.)

### Several documents per editor

Each editor can have several documents open. **Save** stores all of them in the package (the active one as `main.<ext>`, the others as `doc2.<ext>`, …) and **Open** brings them all back. **Open** and **Import** replace a blank, untouched workbook or presentation instead of adding a second one. Links act on the *active* document of each editor.
