//! Which items the table paints, read from their `furniture:paintable` rows:
//! `{"canvas": [x, y, w, h]}` on an item painted as itself, the rect being the part of its tile
//! its cloth and sprite both show; `{"becomes": "<item>", "design": {"tile": "<tile>", "at": [x, y]}}`
//! on an item whose own art is its starting design and which turns into `becomes` once painted.

use mod_sdk::ItemStackData;

use crate::painting::design::{Canvas, Design, PAINT_KEY, TINT_KEY, WHITE};

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Spec {
    #[serde(default)]
    canvas: Option<[u8; 4]>,
    #[serde(default)]
    becomes: Option<String>,
    #[serde(default)]
    design: Option<Seed>,
}

/// The art a design starts from: a tile, and where on it the design's top left sits.
#[derive(Clone, Debug, PartialEq, Eq, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Seed {
    pub tile: String,
    pub at: [u8; 2],
}

/// What painting one item means.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Plan {
    /// The item a painted stack is.
    pub result: String,
    pub canvas: Canvas,
    pub seed: Option<Seed>,
}

#[derive(Default)]
pub(crate) struct Plans(Vec<(String, Plan)>);

impl Plans {
    pub fn load() -> Plans {
        let rows = mod_sdk::items_with_data_as::<Spec>(crate::keys::PAINTABLE);
        let names = mod_sdk::item_names(rows.iter().map(|(item, _)| *item).collect());
        let specs = rows
            .into_iter()
            .zip(names)
            .filter_map(|((_, spec), name)| Some((name?, spec)))
            .collect();
        let (plans, rejected) = Plans::resolve(specs);
        for item in rejected {
            mod_sdk::log(&format!(
                "'{item}' is not paintable: the item it is painted as declares no {}x{} canvas \
                 inside its tile",
                super::CANVAS_TEXELS[0],
                super::CANVAS_TEXELS[1]
            ));
        }
        mod_sdk::log(&format!("{} paintable items", plans.0.len()));
        plans
    }

    /// The plans the rows give, and the items whose rows give none.
    fn resolve(specs: Vec<(String, Spec)>) -> (Plans, Vec<String>) {
        let canvas_of = |item: &str| {
            let (_, spec) = specs.iter().find(|(name, _)| name == item)?;
            Canvas::new(spec.canvas?).filter(|canvas| canvas.size() == super::CANVAS_TEXELS)
        };
        let mut rejected = Vec::new();
        let plans = specs
            .iter()
            .filter_map(|(item, spec)| {
                let result = spec.becomes.as_deref().unwrap_or(item);
                let Some(canvas) = canvas_of(result) else {
                    rejected.push(item.clone());
                    return None;
                };
                let plan = Plan {
                    result: result.to_owned(),
                    canvas,
                    seed: spec.design.clone(),
                };
                Some((item.clone(), plan))
            })
            .collect();
        (Plans(plans), rejected)
    }

    pub fn of(&self, item: &str) -> Option<&Plan> {
        self.0
            .iter()
            .find_map(|(name, plan)| (name == item).then_some(plan))
    }
}

impl Plan {
    /// The design a stack shows before anyone paints it: its paint, else its dye, else the art
    /// its row starts it from (`tile_pixels` reads a tile), else bare cloth.
    pub fn starting_design(
        &self,
        stack: &ItemStackData,
        tile_pixels: impl FnOnce(&str) -> Option<Vec<u8>>,
    ) -> Design {
        let value = |key: &str| {
            let (_, value) = stack.data.iter().find(|(k, _)| k == key)?;
            Some(value.as_slice())
        };
        let painted = || Design::from_paint(self.canvas, value(PAINT_KEY)?);
        let dyed = || {
            Some(Design::filled(
                self.canvas,
                value(TINT_KEY)?.try_into().ok()?,
            ))
        };
        let seeded = || {
            let seed = self.seed.as_ref()?;
            Design::from_tile(self.canvas, seed.at, &tile_pixels(&seed.tile)?)
        };
        painted()
            .or_else(dyed)
            .or_else(seeded)
            .unwrap_or_else(|| Design::filled(self.canvas, WHITE))
    }

    /// `stack` wearing `design`: the plan's result item, the paint in place of any dye. A stack
    /// that turns into another item leaves its own data behind.
    pub fn painted(&self, stack: &ItemStackData, design: &Design) -> ItemStackData {
        let mut data: Vec<(String, Vec<u8>)> = if stack.item == self.result {
            let kept = |(key, _): &&(String, Vec<u8>)| key != PAINT_KEY && key != TINT_KEY;
            stack.data.iter().filter(kept).cloned().collect()
        } else {
            Vec::new()
        };
        data.push((PAINT_KEY.to_owned(), design.to_paint()));
        ItemStackData {
            item: self.result.clone(),
            count: stack.count,
            data,
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) const FLAG: &str = "test:flag";
    pub(crate) const EMBLEM: &str = "test:emblem_flag";

    pub(crate) fn plans() -> Plans {
        let spec = |json: &str| serde_json::from_str::<Spec>(json).expect("a paintable row");
        let (plans, rejected) = Plans::resolve(vec![
            (FLAG.to_owned(), spec(r#"{"canvas": [0, 2, 16, 12]}"#)),
            (
                EMBLEM.to_owned(),
                spec(
                    r#"{"becomes": "test:flag", "design": {"tile": "test:emblem", "at": [0, 0]}}"#,
                ),
            ),
            (
                "test:stray".to_owned(),
                spec(r#"{"becomes": "test:missing"}"#),
            ),
            (
                "test:banner".to_owned(),
                spec(r#"{"canvas": [0, 0, 16, 16]}"#),
            ),
        ]);
        assert_eq!(rejected, ["test:stray", "test:banner"]);
        plans
    }

    pub(crate) fn stack(item: &str, data: &[(&str, Vec<u8>)]) -> ItemStackData {
        ItemStackData {
            item: item.to_owned(),
            count: 3,
            data: data
                .iter()
                .map(|(k, v)| ((*k).to_owned(), v.clone()))
                .collect(),
        }
    }

    fn no_tile(_: &str) -> Option<Vec<u8>> {
        None
    }

    #[test]
    fn an_item_painted_as_another_takes_that_items_canvas() {
        let plans = plans();
        let (flag, emblem) = (
            plans.of(FLAG).expect("flag"),
            plans.of(EMBLEM).expect("emblem"),
        );
        assert_eq!(emblem.canvas, flag.canvas);
        assert_eq!(emblem.result, FLAG);
        assert!(plans.of("test:stray").is_none());
    }

    #[test]
    fn a_dyed_flag_starts_as_its_dye_and_a_painted_one_as_its_paint() {
        let plans = plans();
        let plan = plans.of(FLAG).expect("flag");
        let dyed = stack(FLAG, &[(TINT_KEY, vec![200, 40, 40])]);
        let design = plan.starting_design(&dyed, no_tile);
        assert_eq!(design, Design::filled(plan.canvas, [200, 40, 40]));

        let mut drawn = design.clone();
        drawn.set([2, 2], [0, 0, 255]);
        let painted = plan.painted(&dyed, &drawn);
        assert_eq!(plan.starting_design(&painted, no_tile), drawn);
        assert!(
            painted.data.iter().all(|(key, _)| key != TINT_KEY),
            "the paint replaces the dye rather than sitting under it"
        );
    }

    #[test]
    fn painting_keeps_a_flags_other_data_and_its_count() {
        let plans = plans();
        let plan = plans.of(FLAG).expect("flag");
        let noted = stack(FLAG, &[("test:note", vec![7])]);
        let painted = plan.painted(&noted, &Design::filled(plan.canvas, WHITE));
        assert_eq!(painted.count, noted.count);
        assert!(painted.data.contains(&("test:note".to_owned(), vec![7])));
    }

    #[test]
    fn an_emblem_flag_starts_from_its_art_and_is_painted_into_a_plain_flag() {
        let plans = plans();
        let plan = plans.of(EMBLEM).expect("emblem");
        let mut art = vec![255u8; 16 * 16 * 4];
        art[..3].copy_from_slice(&[10, 20, 30]);
        let emblem = stack(EMBLEM, &[("test:note", vec![7])]);
        let design = plan.starting_design(&emblem, |tile| (tile == "test:emblem").then_some(art));
        assert_eq!(design.get([0, 0]), Some([10, 20, 30]));

        let painted = plan.painted(&emblem, &design);
        assert_eq!(painted.item, FLAG);
        assert_eq!(painted.data, [(PAINT_KEY.to_owned(), design.to_paint())]);
    }
}
