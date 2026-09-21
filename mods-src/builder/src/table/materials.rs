//! The materials page: what the site still needs, against what the chests
//! offer this project.

use std::collections::BTreeMap;

use mod_sdk::*;

use crate::jobs::Builder;
use crate::table::{project_at, Tone};

struct Row {
    item: String,
    name: String,
    need: u32,
    short: u32,
}

#[derive(Default)]
pub struct MaterialsPanel {
    rows: Vec<Row>,
}

impl PanelState for MaterialsPanel {
    fn values(&self) -> Vec<(&'static str, GuiValue)> {
        let rows = self
            .rows
            .iter()
            .map(|row| {
                let note = match row.short {
                    0 => String::new(),
                    short => format!("{short} short"),
                };
                BTreeMap::from([
                    ("item".to_owned(), gui_text(&row.item)),
                    (
                        "text".to_owned(),
                        gui_text(format!("{}x {}", row.need, row.name)),
                    ),
                    ("done".to_owned(), gui_flag(row.short == 0)),
                    ("note".to_owned(), gui_text(note)),
                    ("note_palette".to_owned(), gui_text(Tone::Danger.palette())),
                ])
            })
            .collect();
        vec![("builder:bill", GuiValue::List(rows))]
    }
}

/// The bill of the project the table at `anchor` speaks for: what is short
/// first, the largest gap on top.
pub fn bill(builder: &mut Builder, anchor: [i32; 3], now: u64) -> MaterialsPanel {
    let Some(project) = project_at(builder, anchor)
        .and_then(|id| builder.projects.get(id))
        .map(|p| p.brief())
    else {
        return MaterialsPanel::default();
    };
    let have = builder.available(&project, now);
    let Some(summary) = builder.jobs.map.get(&project.id).and_then(|j| j.summary()) else {
        return MaterialsPanel::default();
    };
    let mut rows: Vec<Row> = summary
        .bill
        .iter()
        .map(|(key, need)| Row {
            item: key.0.clone(),
            name: builder.caches.display_name(&key.0),
            need: *need,
            short: need.saturating_sub(have.get(key).copied().unwrap_or(0)),
        })
        .collect();
    rows.sort_by(|a, b| {
        (a.short == 0)
            .cmp(&(b.short == 0))
            .then(b.short.cmp(&a.short))
            .then_with(|| a.name.cmp(&b.name))
    });
    MaterialsPanel { rows }
}
