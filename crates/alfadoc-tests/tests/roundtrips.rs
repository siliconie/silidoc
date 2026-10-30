//! What survives a trip through the Office formats and through a `.alfadoc` file: the links'
//! anchors (defined names, shape names, table alt text) and the data in them.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use alfadoc_adapter_deck::DeckEngine;
use alfadoc_adapter_grid::GridEngine;
use alfadoc_adapter_word::WordEngine;
use alfadoc_bridge::{CellChange, Engine, MaixAccess, RegionAccess, sync_link};
use alfadoc_model::{Alfadoc, Anchor, DimVector, Endpoint, Link, Stamp, SyncMode, Target, Value};
use alfadoc_sync::Policy;
use serde_json::json;

fn cells() -> Vec<CellChange> {
    vec![
        CellChange { row: 0, col: 0, value: Some(Value::Text("Item".into())) },
        CellChange { row: 1, col: 0, value: Some(Value::Text("Pens".into())) },
        CellChange { row: 1, col: 1, value: Some(Value::Int(12)) },
        CellChange { row: 2, col: 1, value: Some(Value::Float(3.5)) },
    ]
}

#[test]
fn a_sheet_range_and_its_name_survive_an_xlsx_file() {
    let mut a = GridEngine::new();
    a.define_region("alfadoc_x", "Sheet1", "A1:B3").unwrap();
    let anchor = Anchor::GridName { name: "alfadoc_x".into() };
    a.write_changes(&anchor, &cells()).unwrap();
    let saved = a.execute("file.saveBytes", &json!({ "format": "xlsx" })).unwrap();

    let mut b = GridEngine::new();
    b.execute("file.open", &json!({ "name": "round.xlsx", "base64": saved["base64"] })).unwrap();
    let back = b.read_region(&anchor).unwrap();
    assert_eq!(back, a.read_region(&anchor).unwrap());
}

#[test]
fn a_slide_table_and_its_name_survive_a_pptx_file() {
    let mut a = DeckEngine::new();
    let made = a.execute("insert.table", &json!({ "rows": 3, "cols": 2 })).unwrap();
    a.name_table(0, made["id"].as_u64().unwrap() as u32, "alfadoc_x").unwrap();
    let slide_id = a.session().doc().unwrap().doc.slides[0].id.to_string();
    let anchor = Anchor::DeckShape { slide_id, shape_name: "alfadoc_x".into() };
    a.write_changes(&anchor, &cells()).unwrap();
    let saved = a.execute("file.saveBytes", &json!({ "format": "pptx" })).unwrap();

    let mut b = DeckEngine::new();
    b.execute("file.openBytes", &json!({ "name": "round.pptx", "data": saved["data"] })).unwrap();
    // The slide may be numbered differently after the trip; the table's name finds it anyway.
    assert_eq!(b.tables().unwrap().iter().filter(|t| t.shape_name == "alfadoc_x").count(), 1);
    assert_eq!(b.read_region(&anchor).unwrap(), a.read_region(&anchor).unwrap());
}

#[test]
fn a_document_table_and_its_tag_survive_a_docx_file() {
    let mut a = WordEngine::new();
    a.execute("insert.table", &json!({ "rows": 3, "cols": 2 })).unwrap();
    a.tag_selected_table("alfadoc:link=R1").unwrap();
    let anchor = Anchor::WordControl { tag: "alfadoc:link=R1".into() };
    a.write_changes(&anchor, &cells()).unwrap();
    let bytes = wordcraft_engine::io::save_bytes("round.docx", &a.session().doc).unwrap();

    let doc = wordcraft_engine::io::open_bytes("round.docx", &bytes).unwrap();
    let b = WordEngine::from_session(wordcraft_engine::Session::new(doc));
    assert_eq!(b.tables().len(), 1);
    assert_eq!(b.read_region(&anchor).unwrap(), a.read_region(&anchor).unwrap());
}

#[test]
fn links_and_the_data_store_survive_an_alfadoc_file_and_resync_quietly() {
    let mut grid = GridEngine::new();
    grid.define_region("alfadoc_p", "Sheet1", "A1:B3").unwrap();
    let ga = Anchor::GridName { name: "alfadoc_p".into() };
    grid.write_changes(&ga, &cells()).unwrap();

    let mut pkg = Alfadoc::new("00000000-0000-4000-8000-0000000000aa", "Round trip", 1);
    let sheet = pkg.maix.add_sheet("Costs");
    let da = Anchor::Maix { sheet, top: 0, left: 0, bottom: 2, right: 1, dims: DimVector::new() };
    let mut link = Link {
        id: "P1".into(),
        a: Endpoint { target: Target::Internal { part: "sheets/main.xlsx".into() }, anchor: ga.clone() },
        b: Endpoint { target: Target::Internal { part: "data/maix".into() }, anchor: da.clone() },
        mode: SyncMode::Mirror,
        base: None,
    };
    sync_link(&mut link, &mut grid, &mut MaixAccess::new(&mut pkg.maix, "t", 5), Policy::PreferA, 5).unwrap();
    pkg.links.push(link);
    let saved = grid.execute("file.saveBytes", &json!({ "format": "xlsx" })).unwrap();
    pkg.parts.insert("sheets/main.xlsx".into(), saved["base64"].as_str().unwrap().as_bytes().to_vec());

    let bytes = pkg.to_bytes().unwrap();
    let mut back = Alfadoc::from_bytes(&bytes).unwrap();
    assert_eq!(back, pkg);

    // Reopened: the sheet comes from the stored part, the link from the registry.
    let mut grid2 = GridEngine::new();
    let b64 = String::from_utf8(back.parts["sheets/main.xlsx"].clone()).unwrap();
    grid2.execute("file.open", &json!({ "name": "p.xlsx", "base64": b64 })).unwrap();
    let mut link = back.links.remove(0);
    let o = sync_link(&mut link, &mut grid2, &mut MaixAccess::new(&mut back.maix, "t", 6), Policy::Manual, 6).unwrap();
    assert_eq!((o.wrote_a, o.wrote_b, o.conflicts.len()), (0, 0, 0), "after a save and reopen the two ends already agree");
    let _ = Stamp::new("t", 0);
}
