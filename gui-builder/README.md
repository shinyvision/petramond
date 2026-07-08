# Petramond GUI Builder

A document editor for the game's petramond-ui GUIs. GUIs are live widget-tree
documents (`*.gui.json`) interpreted at runtime against a theme kit; the
builder edits those documents and previews them through the **real petramond-ui
runtime + software rasterizer**, so the canvas is pixel-exactly what the game
renders.

## Run

```sh
cargo run --release                      # open the editor
cargo run --release -- samples/pause.llgui
```

The builder loads the game theme from `../assets/ui/theme/theme.json` when it
exists, else falls back to petramond-ui's placeholder kit (toolbar shows which).

## Layout

- **Left**: document tree (drag rows to reorder/reparent, right-click for add
  child / duplicate / wrap / delete) + component palette and presets.
- **Center**: canvas. Click selects (topmost), drag moves abs-positioned
  nodes, handles resize (writes `w`/`h` px), dragging a flow child shows an
  insertion caret and reorders on release. Ctrl+wheel zooms; View menu toggles
  the pixel grid and editor overlay. Double-click a label/button to jump to
  its text in the inspector.
- **Right**: inspector — id, type props, layout, style (theme part keys),
  bindings. Binding fields offer the kind's catalog keys (type-filtered, docs
  as tooltips; inside a list template also the item's fields) with freetext
  for open-ended mod keys.
- **Bottom**: live validation against the engine's per-kind slot contract
  (click an issue to select the offending node), plus the **Screen data**
  panel: the kind's data catalog from `assets/ui/bindings.json` — every state
  key the game populates and every widget id it reacts to. The same catalog
  auto-seeds preview sample data: bind a list to `worlds` and three rows
  appear immediately (opened projects are seeded non-destructively at preview
  time; New documents persist the seeds in their sample_state).
- **Toolbar**: theme reload, preview gui scale (1–4x), screen presets, forced
  hover/pressed/focus on the selection, sample-state editor.

Keyboard: `Ctrl+Z` / `Ctrl+Shift+Z` undo/redo, `Ctrl+S` save, `Ctrl+D`
duplicate, `Delete` remove selection.

## Files

- `.llgui` (v2) — project file: the petramond-ui document verbatim plus editor
  settings (`sample_state`, zoom, preview scale, screen). See `src/project.rs`
  for the tagged sample-state JSON codec.
- **Export** writes the bare document to `assets/ui/documents/<kind>.gui.json`
  (the game hot-reloads it in debug builds).
- **Import Legacy** converts old layer-compositor `.llgui` v1 files
  (`../guis/*.llgui`) into a starting-point document: slot grids, shell
  buttons/inputs, file-image layers; anything untranslatable becomes a
  `TODO:` label.

Document images (`image`/`rotimage` nodes) are PNGs beside the project file /
exported document; missing ones simply don't draw (the validation panel warns).
The Image inspector's "Choose image…" copies a picked PNG next to the project,
and Export copies every referenced image next to the `.gui.json`. Image nodes
support fit modes (stretch / tile / 9-slice with insets).

## Samples

`samples/` holds one ready-to-open project per shipped UI type (File > Open
Sample). They are GENERATED — the shipped document verbatim plus catalog-seeded
sample state — by:

```sh
cargo run -- --make-samples
```

Re-run it after editing anything in `assets/ui/documents/` (a unit test fails
with that instruction when a sample goes stale). Hand edits to `samples/*.llgui`
are overwritten on regeneration.

## CLI

```sh
gui-builder --export <in.llgui> [out.gui.json]      # headless export
gui-builder --import-legacy <v1.llgui> <out.llgui>  # legacy conversion
gui-builder --screenshot <project.llgui> <out.png>  # render the preview raster
gui-builder --make-samples                          # regenerate samples/
```

## Notes

- This crate is deliberately excluded from the game workspace (own
  `Cargo.lock`); it depends on `petramond-ui` by path with the `raster` feature.
- The builder replicates the engine's slot-contract table in
  `src/contracts.rs` — keep it in sync with `src/gui/documents.rs` by hand
  (a unit test pins the expected values).
