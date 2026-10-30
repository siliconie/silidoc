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

- System clipboard bridges (HTML/RTF/images) and printing. Copy and paste of plain text works through egui.
- Auto-recovery of unsaved work, saved preferences, update checks, the local control channel.
- Audio output for slide media (DeckCraft plays silently).
- A combined "unsaved changes" guard when the window closes: only the visible tab's guard runs.
- Drag and drop of files, opening files from the command line.

## Known risks to look at first when you run it

1. Colours or fonts wrong after switching tabs: the snapshot/restore in `ctxstate.rs`.
2. A Word dialog or document window that does not draw in place: the embed switch in `main.rs`.
3. Window title not following the active document: each engine sets the title for its own document.
4. Start-up time and memory of the Document tab: this is the measurement the roadmap's spike asks for.
