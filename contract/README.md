# Contract tests

Per-adapter tests that pin down the upstream surface Alfadoc uses. They live in each adapter crate's
`tests/contract.rs` (created in M1); this folder documents the pattern and holds shared fixtures.

Template (GridCraft adapter), checked against the GridCraft sources we have:

```rust
// crates/alfadoc-adapter-grid/tests/contract.rs
use gridcraft_engine::Session;
use serde_json::json;

#[test]
fn cell_set_runs_and_is_undoable() {
    let mut s = Session::new();
    s.new_workbook();
    // execute takes the parameters by value; cell.set uses "cell" plus "input" (parsed like typing) or "value".
    s.execute("cell.set", json!({ "cell": "A1", "input": 123.5 })).unwrap();
    // …read A1 back through the call the adapter uses and assert 123.5.
}

#[test]
fn every_command_the_adapter_calls_still_exists() {
    let s = Session::new();
    let ids: Vec<&str> = s.commands().iter().map(|c| c.id).collect();
    for needed in ["cell.set" /* , every other id the adapter calls */] {
        assert!(ids.contains(&needed), "command {needed} vanished upstream");
    }
}
```

Notes from reading the sources: `Session::execute(&mut self, id: &str, params: serde_json::Value)`;
`Session::commands()` returns `CommandInfo { id, label, menu, shortcut, params, enabled, .. }` and has
**no mutating flag** (WordCraft's command JSON does: `mutates`). Whether `Session::new()` already has a
workbook open is confirmed when the adapter is written.

The point is that **every upstream call Alfadoc makes appears
in a test**, so a rename fails at update time.
