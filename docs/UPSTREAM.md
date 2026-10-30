# Taking upstream updates

GridCraft, WordCraft and DeckCraft keep changing. Alfadoc takes their updates on purpose, when we
choose, with tests that tell us if something we rely on moved.

## Layout: sibling checkouts, path dependencies

```
D:\artcraft\
  alfadoc\        this repository
  gridcraft\      clone of the upstream GridCraft repository   (unchanged)
  wordcraft\      clone of the upstream WordCraft repository   (unchanged)
  deckcraft\      clone of the upstream DeckCraft repository   (unchanged)
  (pdfcraft, photocraft, vectorcraft, … other ArtCraft apps; not used yet)
```

Alfadoc's root `Cargo.toml` refers to the engines by relative path, with **no versions and no revs**:

```toml
gridcraft-engine = { path = "../gridcraft/crates/engine" }
wordcraft-engine = { path = "../wordcraft/crates/engine" }
deckcraft-engine = { path = "../deckcraft/crates/engine" }
```

Each engine keeps its own workspace (its own `Cargo.toml`, `assets/`, `build.rs` inputs), so nothing in
them is copied or edited. **Every cargo command in Alfadoc needs all three folders to exist**, even for
`-p alfadoc-model`, because Cargo loads every path dependency of the workspace.

## The one thing that must be kept in sync: the `[patch]` table

The engines build against ArtCraft's forks of egui, winit and (for PDF) krilla. They declare this in
their own `[patch.crates-io]`. **Cargo honours `[patch]` only in the root workspace**, so the engines'
tables are ignored when they are built from Alfadoc. Alfadoc's root `Cargo.toml` therefore carries the
union of their tables:

| Patched crate(s) | Source | In |
|---|---|---|
| `egui`, `epaint`, `ecolor`, `emath`, `egui-winit`, `egui-wgpu`, `egui_glow`, `egui_extras`, `egui_kittest`, `egui_system_font_provider`, `eframe`, `egui_demo_lib`, `egui_inspection`, `epaint_default_fonts` | `storytold/egui` @ one rev | all three (identical) |
| `winit` | `storytold/winit` @ one rev | all three (identical) |
| `krilla` | `storytold/craft-libs` @ the craft-libs rev | wordcraft, deckcraft |

Without it Cargo would fetch stock egui and the engines would not build, or would build against
different code than their authors test.

**When you update an engine, compare its `[patch.crates-io]` and its `craft-libs` `rev` with ours.**
If they moved, update Alfadoc's table to match. If two engines disagree (one moved and the others have
not), update the lagging engine first, or hold the leading one back. Today all three use the same
`craft-libs` rev (`92d392cf…`) and the same egui/winit revs.

## Caveat: Alfadoc has its own `Cargo.lock`

Cargo ignores the engines' `Cargo.lock` files when they are built from Alfadoc, so third-party crates
(`skrifa`, `zip`, `image`, …) resolve to whatever Alfadoc's lock says, which can be newer than what an
engine's authors tested. That is why the update routine runs the full test suite and why `Cargo.lock` is
committed. If an engine misbehaves only inside Alfadoc, compare `cargo tree -p <crate>` here with the
engine's own `Cargo.lock` first.

## The update routine

```powershell
# 1. update the engine (repeat for wordcraft, deckcraft)
cd D:\artcraft\gridcraft
git pull --ff-only

# 2. in alfadoc: compare the patch tables and craft-libs rev (see above); edit Cargo.toml if they moved
cd D:\artcraft\alfadoc

# 3. build and test everything, including the contract tests
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings

# 4. check that the build still has exactly one egui (and one craft-libs rev)
cargo tree -d | Select-String "egui|craft-"

# 5. record the engine commits this version of Alfadoc was tested against
$g = git -C ..\gridcraft rev-parse HEAD
$w = git -C ..\wordcraft rev-parse HEAD
$d = git -C ..\deckcraft rev-parse HEAD
"gridcraft $g", "wordcraft $w", "deckcraft $d" | Set-Content UPSTREAM.lock
git add UPSTREAM.lock Cargo.toml Cargo.lock
git commit -m "Update engines (see UPSTREAM.lock)"
```

If step 3 or 4 fails, **do not commit**: `git -C ..\gridcraft checkout <commit from UPSTREAM.lock>`
returns the engine to the last tested state. `UPSTREAM.lock` is what makes a build reproducible, and
what CI uses to check out matching engine commits.

## Contract tests (in each `crates/alfadoc-adapter-*`)

A contract test uses *exactly* the upstream surface Alfadoc relies on and nothing else: specific command
ids with specific parameters, and the read/write round-trips the adapters need. When upstream renames
`cell.set`, this fails at update time instead of in the app. See `contract/README.md`.

Prefer calling **command ids with JSON** over engine-internal Rust types: ids are the stable surface of
all three engines and are what their own CLI, MCP and UI use.

## When we need to change an engine

Prefer not to (docs/EXTENDING.md). If a patch is unavoidable, fork that engine into
`github.com/alfasiliconn/<engine>`, make the change on an `alfadoc` branch, list it in `PATCHES.md`, open
the upstream pull request, and point the sibling folder's `origin` at the fork (with `upstream` as the
original). Until then **no forks are needed**; the sibling clones are enough.

First upstream pull-request candidates:

1. A hook for outside code to register commands (GridCraft and DeckCraft lists are fixed; WordCraft's
   `Registry::new(specs)` already takes a list).
2. `Session::revision()` on GridCraft and DeckCraft, for exact change detection (PATCHES.md).
3. Authentication for DeckCraft's control port (GridCraft requires a token, WordCraft a key file).

## Licences

All three engines are `MIT OR Apache-2.0`. Keep their copyright notices and the About/credits attribution
("ArtCraft Team and contributors"). Alfadoc's own crates are `MIT OR Apache-2.0` (change `license` in
the root `Cargo.toml` if you decide otherwise; it affects only Alfadoc's own crates).
