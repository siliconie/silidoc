# Alfadoc in the browser (experimental)

The same shell runs in a browser through eframe's web runner (WebGPU where available, WebGL2 otherwise).

## Build

One-time setup: `rustup target add wasm32-unknown-unknown` and `cargo install trunk`.

```
cargo check --target wasm32-unknown-unknown -p alfadoc-app -j 2     # compile check only
cd crates/alfadoc-app
trunk serve --release                                              # http://127.0.0.1:8772
trunk build --release                                              # output in dist/web
```

## What works differently

| | Desktop | Browser |
|---|---|---|
| Open / Import | native dialog | browser file picker |
| Save / Save As / Export | writes the file | downloads the file |
| Link to a file on disk | yes | no (a browser has no file system) |
| Editors' own File menu | works | not connected; use Alfadoc's buttons |
| Startup file argument | yes | no |

Everything else (the three editors, the Data tab, links between editors and the data store, `.alfadoc` packages) is the same code.

## Status

The first browser build has not been run through every code path. Expect rough edges: report what the browser console shows.
