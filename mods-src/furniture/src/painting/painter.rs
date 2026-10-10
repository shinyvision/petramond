//! The painting table's client half: the canvas, the colour picker and the tools, all drawn and
//! driven here. Painting is local and instant; the design goes to the server when the pointer
//! reaches for a slot or the table closes, the moments a flag could leave it.

mod color;
mod raster;
mod work;

use mod_sdk::{
    client_emit_event, client_image_set, client_menu, client_tile_pixels, client_ui_state_set,
    ClientFrameData, ClientPointerButton, ClientPointerPhase, ClientUiEvent, GuiValue,
};

use crate::keys;
use crate::painting::design::Rgb;
use crate::painting::paintable::Plans;
use color::Hsv;
use raster::Picture;
use work::{Tool, Work};

const CANVAS_PICTURE: &str = "furniture:canvas";
const WHEEL_PICTURE: &str = "furniture:wheel";
const HUE_PICTURE: &str = "furniture:hue";
const VALUE_PICTURE: &str = "furniture:value";
const SWATCHES_PICTURE: &str = "furniture:swatches";

/// The swatches a painter starts with; colours they paint with push in at the front.
const STARTING_SWATCHES: [Rgb; 16] = [
    [255, 255, 255],
    [26, 28, 44],
    [214, 40, 40],
    [247, 127, 0],
    [252, 212, 64],
    [56, 176, 72],
    [41, 121, 255],
    [123, 63, 196],
    [188, 196, 208],
    [94, 102, 122],
    [140, 28, 52],
    [150, 82, 44],
    [240, 168, 196],
    [24, 110, 96],
    [112, 198, 240],
    [44, 52, 140],
];

/// The parts of the screen that are redrawn only when what they show changes.
#[derive(Default)]
struct Stale {
    canvas: bool,
    picker: bool,
    swatches: bool,
    tools: bool,
}

impl Stale {
    const ALL: Stale = Stale {
        canvas: true,
        picker: true,
        swatches: true,
        tools: true,
    };
}

pub(crate) struct Painter {
    plans: Plans,
    color: Hsv,
    tool: Tool,
    swatches: Vec<Rgb>,
    work: Option<Work>,
    open: bool,
    stale: Stale,
}

impl Painter {
    pub fn new(plans: Plans) -> Painter {
        Painter {
            plans,
            color: Hsv::from_rgb(STARTING_SWATCHES[2], 0.0),
            tool: Tool::Pencil,
            swatches: STARTING_SWATCHES.to_vec(),
            work: None,
            open: false,
            stale: Stale::default(),
        }
    }

    pub fn frame(&mut self, frame: &ClientFrameData) {
        let open = frame.open_gui.as_deref() == Some(keys::TABLE_GUI);
        if !open {
            self.open = false;
            self.work = None;
            return;
        }
        if !self.open {
            self.open = true;
            self.stale = Stale::ALL;
            for (state, picture) in [
                (keys::CANVAS_IMAGE, CANVAS_PICTURE),
                (keys::WHEEL_IMAGE, WHEEL_PICTURE),
                (keys::HUE_IMAGE, HUE_PICTURE),
                (keys::VALUE_IMAGE, VALUE_PICTURE),
                (keys::SWATCHES_IMAGE, SWATCHES_PICTURE),
            ] {
                client_ui_state_set(state, GuiValue::Str(picture.to_owned()));
            }
        }
        let flag = client_menu()
            .filter(|menu| menu.kind_key == keys::TABLE_GUI)
            .and_then(|menu| menu.slots.into_iter().next()?);
        if Work::follow(&mut self.work, &self.plans, flag, client_tile_pixels) {
            self.stale.canvas = true;
            self.stale.tools = true;
        }
        self.show();
    }

    pub fn ui(&mut self, event: &ClientUiEvent) {
        match event {
            ClientUiEvent::ImagePointer {
                id,
                phase,
                x,
                y,
                button,
            } => self.pointer(id, *phase, [*x, *y], *button),
            ClientUiEvent::Toggle { id, .. } => {
                self.tool = match id.as_str() {
                    keys::FILL => Tool::Fill,
                    _ => Tool::Pencil,
                };
                self.stale.tools = true;
            }
            ClientUiEvent::Click { id, .. } => {
                let Some(work) = &mut self.work else {
                    return;
                };
                match id.as_str() {
                    keys::UNDO => work.undo(),
                    keys::CLEAR => work.clear(),
                    _ => return,
                }
                self.stale.canvas = true;
                self.stale.tools = true;
            }
            ClientUiEvent::Hover { id: Some(id), .. }
                if [keys::FLAG_SLOT, keys::INVENTORY, keys::HOTBAR].contains(&id.as_str()) =>
            {
                self.save();
            }
            ClientUiEvent::Dismiss => self.save(),
            _ => {}
        }
    }

    fn save(&mut self) {
        if let Some(work) = &mut self.work {
            work.save(|paint| client_emit_event(keys::PAINT_EVENT, paint));
        }
    }

    fn pointer(
        &mut self,
        id: &str,
        phase: ClientPointerPhase,
        [x, y]: [f32; 2],
        button: ClientPointerButton,
    ) {
        let held = matches!(phase, ClientPointerPhase::Down | ClientPointerPhase::Move);
        match id {
            keys::CANVAS => self.canvas_pointer(phase, raster::canvas_texel(x, y), button),
            keys::WHEEL if held => {
                let picked = raster::wheel_pick(x, y, self.color.v);
                self.pick(Hsv {
                    s: picked.s.min(1.0),
                    ..picked
                });
            }
            keys::HUE if held => self.pick(Hsv {
                h: raster::bar_pick(x),
                ..self.color
            }),
            keys::VALUE if held => self.pick(Hsv {
                v: raster::bar_pick(x),
                ..self.color
            }),
            keys::SWATCHES if phase == ClientPointerPhase::Down => {
                let swatch = raster::swatch_at(x, y).and_then(|i| self.swatches.get(i));
                if let Some(&rgb) = swatch {
                    self.pick(Hsv::from_rgb(rgb, self.color.h));
                }
            }
            _ => {}
        }
    }

    fn canvas_pointer(
        &mut self,
        phase: ClientPointerPhase,
        at: [i32; 2],
        button: ClientPointerButton,
    ) {
        let Some(work) = &mut self.work else {
            return;
        };
        let ink = self.color.to_rgb();
        match (phase, button) {
            (ClientPointerPhase::Down, ClientPointerButton::Secondary) => {
                if let Some(rgb) = work.design().get(at) {
                    self.pick(Hsv::from_rgb(rgb, self.color.h));
                }
                return;
            }
            (ClientPointerPhase::Down, ClientPointerButton::Primary) => {
                work.press(at, self.tool, ink);
                self.keep_swatch(ink);
                self.stale.tools = true;
            }
            (ClientPointerPhase::Move, ClientPointerButton::Primary) => work.drag(at, ink),
            (ClientPointerPhase::Up | ClientPointerPhase::Leave, _) => work.release(),
            (ClientPointerPhase::Move, ClientPointerButton::Secondary) => return,
        }
        self.stale.canvas = true;
    }

    fn pick(&mut self, color: Hsv) {
        self.color = color;
        self.stale.picker = true;
    }

    /// Moves a colour just painted with to the front of the swatches.
    fn keep_swatch(&mut self, rgb: Rgb) {
        if self.swatches.first() == Some(&rgb) {
            return;
        }
        let len = self.swatches.len();
        self.swatches.retain(|&kept| kept != rgb);
        self.swatches.insert(0, rgb);
        self.swatches.truncate(len);
        self.stale.swatches = true;
    }

    fn show(&mut self) {
        let stale = std::mem::take(&mut self.stale);
        let set = |key: &str, picture: Picture| {
            client_image_set(key, picture.w, picture.h, picture.rgba);
        };
        if stale.canvas {
            let design = self.work.as_ref().map(Work::design);
            set(
                CANVAS_PICTURE,
                design.map_or_else(raster::blank, raster::canvas),
            );
        }
        if stale.picker {
            set(WHEEL_PICTURE, raster::wheel(self.color));
            set(HUE_PICTURE, raster::hue_bar(self.color));
            set(VALUE_PICTURE, raster::value_bar(self.color));
        }
        if stale.swatches {
            set(SWATCHES_PICTURE, raster::swatches(&self.swatches));
        }
        if stale.tools {
            let flag = |on: bool| GuiValue::I32(on.into());
            let can_undo = self.work.as_ref().is_some_and(Work::can_undo);
            client_ui_state_set(keys::EMPTY, flag(self.work.is_none()));
            client_ui_state_set(keys::PENCIL_ON, flag(self.tool == Tool::Pencil));
            client_ui_state_set(keys::FILL_ON, flag(self.tool == Tool::Fill));
            client_ui_state_set(keys::CAN_UNDO, flag(can_undo));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn widget<'a>(node: &'a serde_json::Value, id: &str) -> Option<&'a serde_json::Value> {
        if node["id"] == id {
            return Some(node);
        }
        node["children"]
            .as_array()?
            .iter()
            .find_map(|child| widget(child, id))
    }

    /// A pointer position on a picture is read as a pixel of it, which holds only while the
    /// document lays each one out at the size it is drawn.
    #[test]
    fn every_picture_is_laid_out_at_the_size_it_is_drawn() {
        let document: serde_json::Value = serde_json::from_str(include_str!(
            "../../pack/ui/documents/painting_table.gui.json"
        ))
        .expect("the document is JSON");
        let [cols, rows] = raster::SWATCH_GRID;
        assert_eq!(usize::from(cols * rows), STARTING_SWATCHES.len());
        let swatches = raster::swatches(&STARTING_SWATCHES);
        let canvas = crate::painting::CANVAS_TEXELS.map(|n| u16::from(n) * raster::CANVAS_ZOOM);
        let drawn = [
            (keys::CANVAS, canvas),
            (keys::WHEEL, [raster::WHEEL; 2]),
            (keys::HUE, raster::BAR),
            (keys::VALUE, raster::BAR),
            (keys::SWATCHES, [swatches.w, swatches.h]),
        ];
        for (id, [w, h]) in drawn {
            let layout = &widget(&document["root"], id).expect("the widget exists")["layout"];
            assert_eq!([&layout["w"], &layout["h"]], [w, h].map(u64::from), "{id}");
        }
    }
}
