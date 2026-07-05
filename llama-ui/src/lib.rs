//! llama-ui: the renderer-agnostic GUI runtime shared by the game and the
//! GUI builder.
//!
//! A GUI is a [`Document`] (a tree of themed nodes with auto-layout props and
//! state-key bindings) interpreted at runtime against a [`Theme`] kit and a
//! host-supplied [`UiState`]. One `frame()` call solves layout, runs widget
//! behavior over the host's input events, and emits a draw list of textured
//! quads plus resolved widget events and named rects. The same code renders
//! in-game (wgpu upload) and in the builder preview (software raster), so
//! what artists see is exactly what ships.
//!
//! This crate never learns game types: slots are role strings, dynamic values
//! are state keys, host-drawn regions are `hook` nodes.

pub mod doc;
pub mod input;
mod interact;
pub mod layout;
pub mod paint;
mod paint_walk;
#[cfg(feature = "raster")]
pub mod raster;
pub mod runtime;
pub mod state;
pub mod text;
pub mod text_edit;
pub mod theme;
pub mod tree;
pub mod validate;
mod widget;

pub use doc::{
    AbsPos, Align, AlertLevel, Anchor, AnchorEdge, Bindings, Dir, DocClass, DocError, Document,
    GaugeMode, ImageFit, Justify, LayoutProps, Node, NodeKind, ScrollAxis, Size, FORMAT_VERSION,
};
pub use input::{FrameState, InputEvent, NavKey, PointerButton, PreviewState, UiEvent};
pub use layout::{grid_cell, solve, LayoutEnv, RectI, SlotMetrics, Solved};
pub use paint::{Batch, DrawList, Painter, TexId, UiVertex};
pub use paint_walk::{DocImages, NoImages};
pub use runtime::{FrameArgs, FrameOutput, SlotRectOut, UiRuntime};
pub use state::{UiMap, UiState, UiValue};
pub use text_edit::{TextClipboard, TextInput, TextInputRender};
pub use theme::{ImageData, Part, PartFace, Theme, ThemeEnv, ThemeError};
pub use tree::{Inst, InstKey, InstTree};
pub use validate::{DocIssue, SlotContract, StyleLookup};
