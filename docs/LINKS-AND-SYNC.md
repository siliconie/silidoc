# Links and sync

## Vocabulary

- **Endpoint** — a *target* (a part inside the package, or an external file) plus an *anchor*.
- **Anchor** — a named region, using the host format's own construct:

| Anchor | Names… | Native construct (so it survives export) |
|---|---|---|
| `Maix {sheet, top, left, bottom, right, dims}` | a rectangle of the maix store at a dimension vector | maix itself |
| `WordControl {tag}` | a table, block or inline field in a document | Word content control (`w:sdt`) with tag `alfadoc:link=<id>` |
| `GridName {name}` | a range in a workbook | Excel defined name |
| `DeckShape {slide_id, shape_name}` | a table or text shape on a slide | shape name + `alfadoc:link=<id>` tag |

- **Link** — two endpoints (`a`, `b`), a mode, and a `base` snapshot of the last agreed contents.
- **Mode** — `Mirror` (both ways), `AToB`, `BToA`.
- **Region** — the grid of values behind an anchor, with coordinates relative to its top-left.
  A Word table, a sheet range and a slide table all read and write as a Region.

Any engine can sit on either end, so every direction works: sheet→document, document→sheet,
slide↔sheet, document↔slide, and external file↔any of them.

## Why anchors, not new block types

A sheet range shown in a document is **not a new object inside WordCraft**. It is an ordinary
WordCraft table wrapped in a tagged content control. WordCraft lays it out, edits, prints and
exports it with no change. The bridge keeps the table's cells equal to the maix range. The same
holds for slides. This is what keeps the engines untouched and lets exported files keep their links.

## The sync step (`alfadoc_bridge::sync_link`)

```
ra   = A.read_region(link.a.anchor)
rb   = B.read_region(link.b.anchor)
base = link.base                      (empty on first sync)

Mirror:  merge3(base, ra, rb, policy) → write_a, write_b, result, conflicts
AToB:    write_b = one_way(ra → rb)       BToA: write_a = one_way(rb → ra)

apply write_a to A, write_b to B      (only changed cells, through engine commands)
link.base = snapshot(result)          (only if every write succeeded)
```

Per cell, with `None` meaning absent:

- `a == b` → nothing.
- only one side differs from `base` → that side wins and is copied across (deletions too).
- both differ from `base`, differently → **conflict**, settled by policy:
  `PreferA`, `PreferB`, or `Manual` (both sides left as they are, listed in `Outcome.conflicts`,
  `base` keeps the old value so it is still a conflict next round).

Properties (all covered by tests): a second sync with no edits writes nothing; a failed write
leaves `base` unchanged so the retry merges from the same point; writing an equal value is a no-op
in the maix store (no version bump, no history entry), which prevents echo loops.

## Real time

"Real time" means **about one to two seconds after typing stops**, not per keystroke.

- **Inside the app** (all engines are in one process): the dispatcher records commands the adapter
  reports as possibly mutating (`drain_changes`; exact for WordCraft's `mutates` flag, conservative for
  GridCraft and DeckCraft). A `Debouncer` (design values: quiet 400 ms, max wait 2 s) then runs
  `sync_link` for links whose regions may have changed. Running it when nothing changed is harmless.
- **External files:** a file watcher notifies; a `Debouncer` (quiet 1 s, max wait 5 s) absorbs bursts of
  saves; an `EchoGuard` ignores the file's own change event after *we* write it (matched by SHA-256 of
  what we wrote), then `sync_link` runs.

## Writing into real Office files

The engines' writers rebuild a whole file. Anything they do not model (macros, some pivot options,
custom XML, comments' extras) would be lost. **Alfadoc never rewrites a linked external file wholesale.**
It uses a *patch-in-place writer* per format:

1. Open the existing zip.
2. Change only the parts that hold the linked region:
   - `.xlsx`: the cells in `xl/worksheets/sheetN.xml` (values; inline text or shared strings); mark
     the workbook for full recalculation on open; leave every other entry byte-identical.
   - `.docx`: the content inside the `w:sdt` with the link tag.
   - `.pptx`: the table cells or text of the shape with the link name.
3. Write to a temporary file in the same folder, verify it reopens, then replace the original.
4. Record the SHA-256 in the `EchoGuard` and in the link's `Target::External`.

If the engine cannot express a change (e.g. a formula it does not support), the cell is left
untouched and reported, never silently altered.

**Locks.** Excel and Word lock open files on Windows. A locked write is retried with backoff and
shown as a transient toast; the link's `base` does not advance meanwhile.

**Word and PowerPoint files** sync only *marked regions* (content controls, named shapes), never
arbitrary text. Whole-body two-way sync of free text is not reliable and is not offered.

## Web

A browser page cannot watch a folder or silently rewrite a file. On the web:

- files opened through the File System Access API (Chromium) sync two-way like desktop;
- elsewhere, external links are import/export; internal links (inside one `.alfadoc`) behave exactly as on desktop.

## Security

- Link targets that point outside the package require the user to approve the path once; the
  approval is stored per machine, not in the file, so a downloaded `.alfadoc` cannot make the app
  read or write arbitrary paths.
- Conflicts and failed writes are never resolved silently.
