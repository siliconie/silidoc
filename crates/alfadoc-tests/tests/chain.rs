//! A sheet range, the shared data store and a slide table kept equal through links, using the real
//! GridCraft and DeckCraft engines.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use alfadoc_adapter_deck::DeckEngine;
use alfadoc_adapter_grid::GridEngine;
use alfadoc_bridge::{CellChange, Engine, MaixAccess, RegionAccess, sync_link};
use alfadoc_model::{Anchor, DimVector, Endpoint, Link, Maix, SyncMode, Target, Value};
use alfadoc_sync::Policy;
use serde_json::json;

struct World {
    grid: GridEngine,
    deck: DeckEngine,
    maix: Maix,
    grid_anchor: Anchor,
    deck_anchor: Anchor,
    data_anchor: Anchor,
    l1: Link,
    l2: Link,
}

fn endpoint(part: &str, anchor: &Anchor) -> Endpoint {
    Endpoint { target: Target::Internal { part: part.into() }, anchor: anchor.clone() }
}

fn world() -> World {
    let mut grid = GridEngine::new();
    grid.define_region("alfadoc_t", "Sheet1", "A1:B3").unwrap();
    let grid_anchor = Anchor::GridName { name: "alfadoc_t".into() };

    let mut deck = DeckEngine::new();
    let made = deck.execute("insert.table", &json!({ "rows": 3, "cols": 2 })).unwrap();
    deck.name_table(0, made["id"].as_u64().unwrap() as u32, "alfadoc_t").unwrap();
    let slide_id = deck.session().doc().unwrap().doc.slides[0].id.to_string();
    let deck_anchor = Anchor::DeckShape { slide_id, shape_name: "alfadoc_t".into() };

    let mut maix = Maix::default();
    let sheet = maix.add_sheet("Costs");
    let data_anchor = Anchor::Maix { sheet, top: 0, left: 0, bottom: 2, right: 1, dims: DimVector::new() };

    let l1 = Link { id: "L1".into(), a: endpoint("sheets/main.xlsx", &grid_anchor), b: endpoint("data/maix", &data_anchor), mode: SyncMode::Mirror, base: None };
    let l2 = Link { id: "L2".into(), a: endpoint("data/maix", &data_anchor), b: endpoint("slides/main.pptx", &deck_anchor), mode: SyncMode::Mirror, base: None };
    World { grid, deck, maix, grid_anchor, deck_anchor, data_anchor, l1, l2 }
}

impl World {
    /// Syncs both links in the order the app uses (sheet ↔ data, then data ↔ slide), repeating
    /// until nothing is written (a change can need the second link's turn to travel on). Returns
    /// the cells written on each link in total.
    fn sync(&mut self, now: u64) -> (usize, usize) {
        let (mut total1, mut total2) = (0, 0);
        for _ in 0..5 {
            let o1 = sync_link(&mut self.l1, &mut self.grid, &mut MaixAccess::new(&mut self.maix, "test", now), Policy::PreferA, now).unwrap();
            let o2 = sync_link(&mut self.l2, &mut MaixAccess::new(&mut self.maix, "test", now), &mut self.deck, Policy::PreferA, now).unwrap();
            let (w1, w2) = (o1.wrote_a + o1.wrote_b, o2.wrote_a + o2.wrote_b);
            total1 += w1;
            total2 += w2;
            if w1 + w2 == 0 {
                break;
            }
        }
        (total1, total2)
    }
}

fn set(e: &mut impl RegionAccess, a: &Anchor, row: u32, col: u16, v: Value) {
    e.write_changes(a, &[CellChange { row, col, value: Some(v) }]).unwrap();
}

fn get(e: &impl RegionAccess, a: &Anchor, row: u32, col: u16) -> Option<Value> {
    e.read_region(a).unwrap().cells.get(&(row, col)).cloned()
}

#[test]
fn a_value_typed_in_the_sheet_reaches_the_slide_through_the_data_store() {
    let mut w = world();
    let ga = w.grid_anchor.clone();
    set(&mut w.grid, &ga, 0, 0, Value::Text("Item".into()));
    set(&mut w.grid, &ga, 1, 0, Value::Text("Pens".into()));
    set(&mut w.grid, &ga, 1, 1, Value::Int(12));
    set(&mut w.grid, &ga, 2, 1, Value::Float(3.5));
    w.sync(1);
    assert_eq!(get(&w.deck, &w.deck_anchor, 1, 0), Some(Value::Text("Pens".into())));
    assert_eq!(get(&w.deck, &w.deck_anchor, 1, 1), Some(Value::Int(12)));
    assert_eq!(get(&w.deck, &w.deck_anchor, 2, 1), Some(Value::Float(3.5)));
    assert_eq!(w.maix.get(1, 1, 1, &DimVector::new()).unwrap(), Some(&Value::Int(12)));
}

#[test]
fn a_later_edit_in_the_sheet_follows_and_a_quiet_pass_writes_nothing() {
    let mut w = world();
    let ga = w.grid_anchor.clone();
    set(&mut w.grid, &ga, 1, 1, Value::Int(12));
    w.sync(1);
    set(&mut w.grid, &ga, 1, 1, Value::Int(99));
    w.sync(2);
    assert_eq!(get(&w.deck, &w.deck_anchor, 1, 1), Some(Value::Int(99)));
    assert_eq!(w.sync(3), (0, 0), "nothing changed, so nothing is written");
}

#[test]
fn an_edit_on_the_slide_flows_back_to_the_sheet() {
    let mut w = world();
    let ga = w.grid_anchor.clone();
    set(&mut w.grid, &ga, 0, 0, Value::Text("Item".into()));
    w.sync(1);
    let da = w.deck_anchor.clone();
    set(&mut w.deck, &da, 0, 0, Value::Text("Product".into()));
    w.sync(2);
    assert_eq!(get(&w.grid, &w.grid_anchor, 0, 0), Some(Value::Text("Product".into())));
    assert_eq!(w.maix.get(1, 0, 0, &DimVector::new()).unwrap(), Some(&Value::Text("Product".into())));
}

#[test]
fn clearing_a_sheet_cell_clears_it_everywhere() {
    let mut w = world();
    let ga = w.grid_anchor.clone();
    set(&mut w.grid, &ga, 2, 0, Value::Text("temp".into()));
    w.sync(1);
    assert_eq!(get(&w.deck, &w.deck_anchor, 2, 0), Some(Value::Text("temp".into())));
    w.grid.write_changes(&ga, &[CellChange { row: 2, col: 0, value: None }]).unwrap();
    w.sync(2);
    assert_eq!(get(&w.deck, &w.deck_anchor, 2, 0), None);
}

#[test]
fn a_cell_changed_on_both_sides_is_a_conflict_that_manual_policy_leaves_alone() {
    let mut w = world();
    let (ga, da) = (w.grid_anchor.clone(), w.deck_anchor.clone());
    set(&mut w.grid, &ga, 1, 1, Value::Int(1));
    w.sync(1);
    // Edit the same cell on the sheet and on the slide, then sync the data-store ends by hand.
    set(&mut w.grid, &ga, 1, 1, Value::Int(2));
    set(&mut w.deck, &da, 1, 1, Value::Int(3));
    sync_link(&mut w.l1, &mut w.grid, &mut MaixAccess::new(&mut w.maix, "test", 2), Policy::Manual, 2).unwrap();
    let o = sync_link(&mut w.l2, &mut MaixAccess::new(&mut w.maix, "test", 2), &mut w.deck, Policy::Manual, 2).unwrap();
    assert_eq!(o.conflicts.len(), 1);
    assert_eq!(get(&w.deck, &w.deck_anchor, 1, 1), Some(Value::Int(3)), "the slide keeps its own value");
    let _ = &w.data_anchor;
}
