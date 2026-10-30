# Patches carried on `alfadoc` branches

Each upstream fork keeps its own `PATCHES.md` listing patches on its `alfadoc` branch. This file is
the template. **An empty list is the goal.**

| # | Fork | What | Why | Upstream PR | Status |
|---|---|---|---|---|---|
| — | — | (none yet) | | | |

Planned first patches (hooks only):

| Fork | Hook | Reason |
|---|---|---|
| gridcraft | `register_command(id, spec)` on the command registry | Let `alfadoc.*`-adjacent engine commands appear in its ribbon/palette |
| deckcraft | same | same |
| gridcraft, deckcraft | `Session::revision() -> u64`, a counter that increases on every document change | Exact change detection for live sync (today only the sticky `is_dirty()` exists) |
| deckcraft | token/key-file auth on `--control` | Security parity with GridCraft/WordCraft |
