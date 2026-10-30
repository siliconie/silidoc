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
| **Paged** | `maix/1.1-paged` | `sheets/*.mxb` etc., 4 KB pages, CRC-32C, B+ trees (1.0 draft §4–§10), stored uncompressed | Later; required only when a store no longer fits in memory |

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

**Status (v1).** The Data tab evaluates `DIMSUM`, `DIMAVG`, `DIMCOUNT`, `DIMSLICE` and the `@{…}` suffix in its own formulas (see `crates/alfadoc-model/src/formula.rs` for the exact grammar and error codes). Cells read each other's stored values and a recalculation repeats until nothing changes, so a circular reference stops after eight passes instead of being reported. The variant-block rule is implemented (`variant.rs`) and used for documents (Links ▸ Variant blocks): mark the selected words, then keep only the blocks for the current dimensions. A live show/hide view and slide shapes are not done.

**Formulas stay Excel-compatible.** The dimension suffix and `DIMSUM`, `DIMAVG`, `DIMSLICE` are
*additions* to GridCraft's existing formula language, never a replacement, so formulas imported
from Excel keep working.

## 5. Migrations

Unchanged from the 1.0 draft §16 (`.mxm` JSON operation lists, applied atomically in version order).
The `.alfadoc` container records its own version separately (ALFADOC-FORMAT).
