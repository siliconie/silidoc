# maix 1.1

maix 1.1 = the **1.0 draft** (unchanged except where listed here) + **errata** + the **JSON-records
profile** that Alfadoc v1 stores + **how maix applies to documents and presentations**.

The 1.0 draft remains the reference for dimensions (§6), the metadata cascade (§9), relations
(§8.8), history (§8.9), access control (§8.10) and migrations (§16). The paged binary layout
(§4, §7, §8, §10) is a later profile (§3 below).

## 1. Errata to the 1.0 draft

Found while reading both copies of the draft side by side. Each is resolved here and reflected in code.

| # | Where | Problem | Resolution in 1.1 |
|---|---|---|---|
| E1 | §6.5 | Says the empty vector hashes to 0 "as the hash of the empty string". xxHash64 of `""` is not 0. | **The empty vector has `dim_hash = 0` by definition**, not by hashing. Any other vector whose hash is 0 is rehashed with the collision counter. |
| E2 | §6.3 | xxHash64 seed not stated. | Seed is **0**. |
| E3 | §8.1.2 vs §8.10.1 | Roles are defined with `parent` in one place and `inherits_from` + `capabilities` in the other. | Use §8.10.1 (`inherits_from` + `capabilities`). `parent` is removed. |
| E4 | §13.5 vs §11.4 | A cross-sheet formula needs an existing `references`/`depends_on` relation, but `depends_on` relations are *derived from* formulas. Circular. | Formulas may reference any sheet in the same file. `depends_on` relations are created automatically from formula references. The sandbox (§13.5) applies only to **reading another namespace's metadata** and to **dereferencing external links**. |
| E5 | §11.2 | `=[Sheet Name]A1` and `sheet_0002!A1` differ from Excel's `='Sheet Name'!A1`, so imported formulas would need rewriting. | Canonical form is Excel's `'Sheet Name'!A1`. The other two are accepted on read and rewritten. The dimension suffix `@{…}` is appended after the reference: `'Costs'!A1@{scenario:"base"}`. |
| E6 | §8.6 | `updated_at` is `uint48` here and `uint64` everywhere else. | `uint64` everywhere. |
| E7 | §8.10, §13.1 | Role rules and cell-level access are enforced by the reader, but anyone holding the file can unzip it and read everything. | Access rules are **advisory** unless the namespace is encrypted (§13.3). Documented as such; the UI says so when a user sets a rule on an unencrypted namespace. |
| E8 | §13.4 | If the HMAC key for the audit chain is stored in the file, whoever holds the file can rewrite the chain. | The key must be kept **outside** the file (OS keystore or the user's passphrase). Without it the chain is only checksum-consistent, and tools must say so. |
| E9 | §2.1 vs §4 | "Custom binary pages, 4 KB aligned for memory-mapping" cannot be combined with a compressed zip: compressed entries cannot be mapped or updated in place, so each save rewrites the entry. | See §3: v1 uses the JSON-records profile. The paged profile stores page files **uncompressed** in the zip (or unpacked in a working folder) so mapping works. |
| E10 | §8.7.4 | `dim_scope = filter` stores the filter as raw bytes without saying the encoding. | JSON object of string pairs, UTF-8. |

## 2. Dimensions, cells, metadata — unchanged

A cell is addressed `(sheet, row, col, dim_vector)`. The vector is hashed (canonical string →
xxHash64 seed 0, collisions rehashed with `|__collision=N`) and resolved through the Dimension
Vector Table. Metadata is namespaced and cascades cell → column → sheet → workbook → schema default
with `__cascade: replace` to stop inheritance. Implemented in `alfadoc-model::{dims, maix}`.

## 3. Profiles

| Profile | Marker (`Maix.profile`) | Storage | Status |
|---|---|---|---|
| **JSON-records** | `maix/1.1-json` | `data/maix.json`: axes, vector table, sheets with a record per cell, history, app registry | **Implemented, used by Alfadoc v1** |
| **Paged** | `maix/1.1-paged` | `data/maix.pages`: 4 KB pages, CRC-32C, sorted leaf chain plus a leaf index, stored uncompressed (see "Paged profile" below) | Used automatically for stores of 200,000 cells or more |

Both profiles carry the same logical content, so a store converts between them without loss. A
reader that does not know a profile marker refuses the file.

The JSON profile is sufficient while a store fits comfortably in memory (hundreds of thousands of
cells). Nothing above the storage layer changes when the paged profile arrives.

## 4. Applying maix to documents and presentations

The 1.0 draft treats the **cell** as the only multidimensional entity. 1.1 generalises "cell" to
**node**: any addressable piece of content that holds a value. The rules (dimensions on the node,
namespaced metadata, cascade, history) stay identical.

| Part | Node | Address |
|---|---|---|
| Spreadsheet | cell | `(sheet, row, col, dim_vector)` |
| Document | content control (a field, a block, a live table) | `(part, control tag, dim_vector)` |
| Presentation | shape or table cell | `(slide id, shape name, [row, col], dim_vector)` |

**What is implemented in v1:** maix is the store of record for tabular data; documents and slides
*reference* it through links (LINKS-AND-SYNC). A Word table, a field in a sentence, or a slide
table is a view of a maix range at a dimension vector.

**What 1.1 specifies for later (M4):** *variant blocks*. A content control tagged
`alfadoc:variant={"audience":"client"}` is shown only when the document's active dimension vector
matches. One proposal can then carry `audience: client | internal` or `language: en | sw`, and the
view switches by choosing the vector. The same tag works on slide shapes. Because the tag is a plain
Word/PowerPoint tag, an exported file keeps all variants; a "flatten for this audience" export
removes the others.

**Status (v1).** The Data tab evaluates `DIMSUM`, `DIMAVG`, `DIMCOUNT`, `DIMSLICE` and the `@{…}` suffix in its own formulas (see `crates/alfadoc-model/src/formula.rs` for the exact grammar and error codes). Cells read each other's stored values and a recalculation repeats until nothing changes, so a circular reference stops after eight passes instead of being reported. The variant-block rule is implemented (`variant.rs`) and used for documents (Links ▸ Variant blocks): mark the selected words, then keep only the blocks for the current dimensions. Slide shapes work too (Links ▸ Variant blocks ▸ Slides): **Mark the selected shapes** stores the tag in each shape's alt text (after any alt text it had; the shape's name, and so links to it, stay as they are), and **Show only these shapes** shows the tagged shapes that fit the current dimensions and hides the others with the Selection Pane's Show/Hide, so Undo and the `.pptx` keep working. Documents still delete the other blocks instead of hiding them; a live show/hide view for documents is not done. The **Export** window can write a "flatten for this audience" copy of a document or slide deck (the blocks and shapes for other values are left out; your document is not changed). Variant tags that fit stay in the exported file.

### Paged profile (`maix/1.1-paged`)

A store with 200,000 or more cells (and at most 100 sheets) is saved in two entries instead of one big JSON document: `data/maix.json` keeps everything except the cells (sheets, columns, formats, dimensions, history) and carries the profile `maix/1.1-paged`; `data/maix.pages` holds the cells, **stored uncompressed** in the zip (E9). A smaller store stays one JSON document. Loading restores the plain in-memory store, so nothing above the storage layer changes; saving chooses the form by size, and a file written either way opens in either version of the program that knows the profile.

`data/maix.pages` (`alfadoc-model::paged`) is a sequence of 4096-byte pages, each starting with a CRC-32C of the rest of the page, a kind, a record count and the number of the next page in its chain:

* **Page 0, header:** magic `MXPG`, version 1, page size, page count, cell count, and for each sheet its id, cell count, first leaf, leaf count and first index page.
* **Leaf pages:** the sheet's cells in (row, column, dimension hash) order, each record `row u32, col u16, dim u64`, then the cell body inline (value, formula, metadata, version, time) or, when the body is over 1 KB, the number of its **overflow pages**.
* **Index pages:** for each leaf its first key and page number, so one cell is found by reading the index and a single leaf (`PagedStore::find`).

Reading never trusts the file: every page it touches is checksummed, chains cannot loop (they stop at the declared length), lengths cannot run past their page, keys must be in order and counts must match the header. A damaged page is refused with the page number instead of loading wrong data. All numbers are little-endian.

**Formulas stay Excel-compatible.** The dimension suffix and `DIMSUM`, `DIMAVG`, `DIMSLICE` are
*additions* to GridCraft's existing formula language, never a replacement, so formulas imported
from Excel keep working.

## 5. Migrations

Unchanged from the 1.0 draft §16 (`.mxm` JSON operation lists, applied atomically in version order).
The `.alfadoc` container records its own version separately (ALFADOC-FORMAT).

## Alfadoc extension: sheet `format`

A sheet record may carry an optional `format` object (ignored by readers that do not know it, absent when the sheet has no custom look):

```json
"format": {
  "layout": { "col_widths": {"2": 140.0}, "row_heights": {"0": 40.0}, "hidden_cols": [5], "hidden_rows": [], "freeze_rows": 1, "freeze_cols": 0 },
  "styles": [ { "bold": true, "fill": 16774400, "numfmt": "#,##0.00" } ],
  "cells": [ { "row": 0, "col": 1, "style": 1 } ],
  "cols": { "3": 1 },
  "rows": { "0": 1 }
}
```

Style ids are 1-based indexes into `styles`; 0 and an unknown id mean the plain style. A cell's own style wins over its column's, which wins over its row's. Colours are `0xRRGGBB` integers; `numfmt` is an Excel number-format code. Limits: 1,048,576 rows, 16,384 columns.
