# Alfadoc

One app for documents, spreadsheets and presentations, with its own `.alfadoc` format.
Data is shared between all three and stays in sync with the files you import it from.

- **Rust + egui**, desktop (Windows, macOS, Linux) and web. No mobile build.
- Built on three open-source ArtCraft engines used **unmodified, as sibling path dependencies**
  (`../wordcraft` documents, `../gridcraft` spreadsheets, `../deckcraft` presentations). All MIT OR Apache-2.0.
- **maix** is the data layer: a multidimensional, namespaced, versioned cell store that all three views read.

## Documents

| Doc | What it answers |
|---|---|
| [ARCHITECTURE](docs/ARCHITECTURE.md) | Layers, crates, how the three engines share one window and one data store |
| [ALFADOC-FORMAT](docs/ALFADOC-FORMAT.md) | The `.alfadoc` package: entries, limits, versioning |
| [MAIX-1.1](docs/MAIX-1.1.md) | maix 1.0 draft + errata + the JSON-records profile + documents and slides |
| [LINKS-AND-SYNC](docs/LINKS-AND-SYNC.md) | Anchors, links, three-way merge, writing back into real `.xlsx/.docx/.pptx` files |
| [UPSTREAM](docs/UPSTREAM.md) | Forks, pinning, the update routine, contract tests |
| [EXTENDING](docs/EXTENDING.md) | Rules for adding features without touching upstream code |
| [ROADMAP](docs/ROADMAP.md) | Milestones and what is done |

## Status

Milestone M0 is done: the data model, `.alfadoc` container, maix store, link registry, three-way
merge and link-sync core are implemented and tested (42 tests), and the workspace resolves together
with all three engines on one egui. The engine adapters and the app shell are M1.

Expected layout (see [UPSTREAM](docs/UPSTREAM.md)): `alfadoc` next to `gridcraft`, `wordcraft`, `deckcraft`.

```
cargo test                      # data-layer crates only: fast, light on memory
cargo test --workspace          # also builds the three engines and their UI stack (heavy, first run is long)
cargo clippy --workspace --all-targets -- -D warnings
```

### Troubleshooting (Windows): "paging file is too small" / "out of memory" / `STATUS_STACK_BUFFER_OVERRUN`

Building the engines compiles many large crates (`read-fonts`, `skrifa`, `vello_cpu`, `rustls`, …) at
the same time with optimisation on, and `rustc` can exhaust virtual memory. The messages that follow
(`only metadata stub found`, `failed to mmap rmeta`, `can't find crate`) are side effects, not real errors.

1. Close memory-heavy apps and build with fewer parallel jobs: `cargo test --workspace -j 2`
   (`.cargo/config.toml` already limits Cargo to 4 jobs; lower it if needed).
2. Enlarge the Windows paging file: *System > Advanced system settings > Performance > Settings >
   Advanced > Virtual memory > Change*: untick "Automatically manage", choose a drive with free space,
   custom size 16384 MB initial / 32768 MB maximum, then restart.
3. After a crashed build, delete half-written artifacts once: `Remove-Item -Recurse -Force target`.

## Code rules

Same as the engines: `unsafe` is forbidden; library code may not `unwrap`, `expect` or `panic`;
every parser is bounded and must return an error on hostile input.
