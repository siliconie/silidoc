# Extending without touching upstream

Order of preference — stop at the first that works:

1. **Our own crate above the bridge.** New features (links, converters, variant blocks, dashboards)
   live in `alfadoc-*` crates and talk to engines through `Engine`/`RegionAccess` and command ids.
2. **`alfadoc.*` commands.** Register them on the `Dispatcher`. They never reach an engine, so they cannot
   collide with upstream ids. Anything the user can do should be a command (this is how the engines
   work too, and it keeps menus, shortcuts, scripting and tests on one path).
3. **Native markers.** Store our metadata in constructs the engines already preserve: content-control
   tags, defined names, shape names/tags, custom document properties. Do not invent new block types
   inside an engine's model.
4. **A small patch in a fork.** Only for a hook (a callback or trait), never a behaviour change.
   Record it in the fork's `PATCHES.md` (what, why, upstream PR link). Open the upstream PR at the same time.
5. **A bigger change** → discuss first: it probably belongs upstream.

## Rules for our code

- `unsafe` forbidden; no `unwrap`/`expect`/`panic`/`todo` in library code (enforced by lints in the root `Cargo.toml`).
- Parsers and importers are bounded (sizes, depth, counts) and return errors on bad input.
- No engine types in `alfadoc-model`, `alfadoc-sync`, `alfadoc-bridge`. Only adapters import engines.
- Pass time in as a parameter (`now_ms`); do not read the clock in logic you want to test.
- Every user-visible screen: permission/licence check where relevant, offline-capable behaviour,
  responsive layout; connection and sync status as a transient **toast**, not a banner.
- Write the test first when fixing a sync bug: reproduce it with in-memory `RegionAccess` fakes (see `link_sync.rs` tests).

## Safe-change checklist

- [ ] Does it still compile with the engines at the pinned revs, and with `cargo clippy -- -D warnings`?
- [ ] Do contract tests still pass? Did you add one for any new upstream surface you now depend on?
- [ ] Does it work with the web target (no threads, no direct file system) or is it behind `cfg(not(target_arch = "wasm32"))` with a web fallback?
- [ ] If it changes the `.alfadoc` layout: version bumped, reader still opens old files?
