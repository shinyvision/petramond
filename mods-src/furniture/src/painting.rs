//! The painting table: one slot and a canvas a flag's design is painted on. The client half
//! ([`painter`]) owns the canvas and every stroke; the server half ([`table`]) hears one request,
//! "paint the flag in my table like this", and repaints the stack. What can be painted, and as
//! what, is row data ([`paintable`]).

mod design;
mod paintable;
mod painter;
mod table;

use mod_sdk::{ClientFrameData, ClientUiEvent, PlayerId};

use crate::keys;
use paintable::Plans;
use painter::Painter;

/// The canvas the table's document lays out, in texels: the one design size it paints.
pub(crate) const CANVAS_TEXELS: [u8; 2] = [16, 12];

pub(crate) enum Painting {
    Server(Plans),
    Client(Box<Painter>),
}

impl Painting {
    pub fn init(client: bool) -> Painting {
        let plans = Plans::load();
        if client {
            Painting::Client(Box::new(Painter::new(plans)))
        } else {
            Painting::Server(plans)
        }
    }

    /// A client's request to paint the flag in the table it has open.
    pub fn on_client_event(&self, player: PlayerId, key: &str, data: &[u8]) {
        if let (Painting::Server(plans), keys::PAINT_EVENT) = (self, key) {
            table::paint(plans, player, data);
        }
    }

    pub fn client_frame(&mut self, frame: &ClientFrameData) {
        if let Painting::Client(painter) = self {
            painter.frame(frame);
        }
    }

    pub fn client_ui(&mut self, kind_key: &str, event: &ClientUiEvent) {
        if let (Painting::Client(painter), keys::TABLE_GUI) = (self, kind_key) {
            painter.ui(event);
        }
    }
}
