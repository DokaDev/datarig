//! The mouse on the explorer: a click on a row's arrow expands or collapses it (every kind of
//! row, a table's structure included) and only selects it; a click elsewhere on a row selects
//! it, a double click opens it; the arrow under the pointer is lit.

mod common;

use common::*;
use datarig_core::config::IconsSetting;
use datarig_core::driver::{DbCommand, DbEvent, SchemaObjects};
use datarig_i18n::Lang;
use ratatui::crossterm::event::{KeyCode, MouseButton, MouseEventKind};
use ratatui::style::Modifier;

/// Connected `local-pg`, the explorer focused, `shop` closed with its objects not read yet.
fn explorer() -> Harness {
    let mut h = Harness::connected(Lang::En);
    h.app.icons = IconsSetting::Off;
    h.db(DbEvent::Databases(Ok(vec!["datarig".into()])));
    h.key(KeyCode::BackTab);
    h.draw(120, 40);
    h.sent();
    h
}

/// `shop`'s objects as the server lists them.
fn objects(h: &mut Harness) {
    let objects = SchemaObjects {
        tables: vec!["orders".into(), "users".into()],
        views: vec!["order_summary".into()],
        ..Default::default()
    };
    h.db(DbEvent::Objects { schema: "shop".into(), result: Ok(objects) });
    h.draw(120, 40);
}

/// The index of the row the harness shows as `name`.
fn index(h: &Harness, name: &str) -> usize {
    h.rows().iter().position(|r| r.trim() == name).unwrap_or_else(|| panic!("no {name} in {:?}", h.rows()))
}

/// The screen cell of the arrow of the row shown as `name` (drawn first, so that the
/// explorer's area is known; the list fits, unscrolled).
fn arrow(h: &mut Harness, name: &str) -> (u16, u16) {
    h.draw(120, 40);
    let i = index(h, name);
    let area = h.app.explorer.area;
    let depth = h.app.explorer_rows()[i].depth;
    (area.x + (depth * 2) as u16, area.y + i as u16)
}

fn click(h: &mut Harness, (x, y): (u16, u16)) {
    h.mouse(MouseEventKind::Down(MouseButton::Left), x, y);
    h.mouse(MouseEventKind::Up(MouseButton::Left), x, y);
    h.draw(120, 40);
}

fn shown(h: &Harness, name: &str) -> bool {
    h.rows().iter().any(|r| r.trim() == name)
}

/// A click on a schema's arrow opens it (its objects are asked for) and selects it; a second
/// click closes it again; neither opens a tab.
#[test]
fn a_click_on_the_arrow_opens_and_closes_a_schema() {
    let mut h = explorer();
    let tabs = h.app.tabs.len();
    let at = arrow(&mut h, "shop");
    click(&mut h, at);
    assert!(
        h.sent().iter().any(|c| matches!(c, DbCommand::LoadObjects { schema } if schema == "shop")),
        "its objects are asked for"
    );
    assert!(h.explorer_line().contains("shop"), "selected: {}", h.explorer_line());
    objects(&mut h);
    assert!(shown(&h, "users"), "{:?}", h.rows());
    // The second click comes at once: a toggle again, not a double click.
    click(&mut h, at);
    assert!(!shown(&h, "users"), "closed: {:?}", h.rows());
    assert_eq!(h.app.tabs.len(), tabs, "no tab opened");
    assert!(h.sent().is_empty(), "the objects read are kept");
}

/// A table's arrow opens its structure as `l` does (its structure is read, no tab opens), and
/// closes it; a double click on its name still opens its tab.
#[test]
fn a_click_on_a_tables_arrow_opens_its_structure_and_a_double_click_its_tab() {
    let mut h = explorer();
    let shop = arrow(&mut h, "shop");
    click(&mut h, shop);
    objects(&mut h);
    h.sent();
    let tabs = h.app.tabs.len();
    let rows = h.rows().len();
    let at = arrow(&mut h, "users");
    click(&mut h, at);
    assert!(
        h.sent()
            .iter()
            .any(|c| matches!(c, DbCommand::LoadStructure { schema, table } if schema == "shop" && table == "users")),
        "its structure is read"
    );
    assert!(h.explorer_line().contains("users"), "selected: {}", h.explorer_line());
    assert!(h.rows().len() > rows, "open: {:?}", h.rows());
    click(&mut h, at);
    assert_eq!(h.rows().len(), rows, "closed: {:?}", h.rows());
    assert_eq!(h.app.tabs.len(), tabs, "no tab opened");
    // A double click on the name opens the table.
    let name = (at.0 + 4, at.1);
    click(&mut h, name);
    assert_eq!(h.rows().len(), rows, "a click on the name only selects");
    click(&mut h, name);
    assert_eq!(h.app.tabs.len(), tabs + 1, "the double click opens its tab");
}

/// The connected profile's arrow collapses and expands it like any other row.
#[test]
fn a_click_on_a_profiles_arrow_collapses_and_expands_it() {
    let mut h = explorer();
    let at = arrow(&mut h, "local-pg");
    click(&mut h, at);
    assert!(!shown(&h, "shop"), "collapsed: {:?}", h.rows());
    click(&mut h, at);
    assert!(shown(&h, "shop"), "expanded: {:?}", h.rows());
}

/// The arrow under the pointer is drawn in the accent colour and bold; moving off it (onto the
/// row's name) leaves it as it was.
#[test]
fn the_arrow_under_the_pointer_is_lit() {
    let mut h = explorer();
    let (x, y) = arrow(&mut h, "shop");
    let plain = h.draw(120, 40).backend().buffer()[(x, y)].clone();
    assert_eq!(plain.symbol(), "▸");
    h.mouse(MouseEventKind::Moved, x + 1, y);
    let t = h.draw(120, 40);
    let lit = &t.backend().buffer()[(x, y)];
    assert_eq!(lit.symbol(), "▸");
    assert_eq!(lit.fg, datarig_tui::theme::cur().accent);
    assert!(lit.modifier.contains(Modifier::BOLD));
    h.mouse(MouseEventKind::Moved, x + 4, y);
    assert_eq!(h.draw(120, 40).backend().buffer()[(x, y)], plain);
}
