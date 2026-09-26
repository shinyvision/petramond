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

## Asset roots

The builder reads game assets through the same layering the game uses: the
base `assets/` directory, then pack roots above it. A point file — the theme
(`ui/theme/theme.json`), a document image (`ui/documents/<name>`) — comes from
the highest-priority layer holding it; catalogs (`items.json` tags,
`ui/bindings.json` binding docs) merge every layer, so a pack can document the
state keys of its own GUI kinds.

- **Base**: `--assets <dir>`, else the nearest `assets/` above the working
  directory, else above the executable. Nothing is compiled in, so a release
  build unpacked beside the game's `assets/` works as-is.
- **Packs**: every `--pack <dir>` (repeatable, highest priority last), then
  the project's own `editor.asset_roots` list (pack roots relative to the
  `.llgui`), then the pack the project is saved inside (the nearest ancestor
  holding a `pack.json`) on top.

With no theme in any layer the preview falls back to petramond-ui's
placeholder kit (the toolbar shows which theme loaded).

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
- Old layer-compositor `.llgui` v1 files are detected and refused with a
  message; the importer that converted them was retired once every shipped
  GUI had been migrated.

Document images (`image`/`rotimage` nodes, and image-backed `button` faces)
are PNGs beside the project file / exported document; missing ones simply
don't draw (the validation panel warns). The Image inspector's "Choose image…"
copies a picked PNG next to the project, and Export copies every referenced
image next to the `.gui.json`. Image nodes support fit modes (stretch / cover /
tile / 9-slice with insets) and an optional sprite-sheet grid (`frames`
cols×rows) with an `fps` animation rate — one frame draws and sizes the node,
`bind.frame` (a numeric state key, offered in the binding picker on framed
nodes) picks the frame authoritatively and `fps` cycles otherwise. A button's
"custom image" toggle swaps its theme chrome for a document image with the
same optional frames/fps; an image-backed button carries no text, icon, or
children, so those controls hide (and clear) while a custom image is set.
Generated samples preserve shipped document image paths and preview them
through the shipped `assets/ui/documents/` location when those files are not
beside the sample project.

## Samples

`samples/` holds one ready-to-open project per shipped UI type (File > Open
Sample). They are GENERATED — the shipped document verbatim plus catalog-seeded
sample state — by:

```sh
cargo run -- --make-samples
```

Re-run it after editing anything in `assets/ui/documents/` (a unit test fails
with that instruction when a sample goes stale or orphaned). Samples whose
document no longer ships are deleted, so the sample list is exactly the
shipped set. Hand edits to `samples/*.llgui` are overwritten on regeneration.

## CLI

```sh
gui-builder --export <in.llgui> [out.gui.json]      # headless export
gui-builder --screenshot <project.llgui> <out.png>  # render the preview raster
gui-builder --make-samples                          # regenerate samples/
```

`--assets <dir>` and `--pack <dir>` go before any of these (see Asset roots).

## Notes

- This crate is deliberately excluded from the game workspace (own
  `Cargo.lock`); it depends on `petramond-ui` by path with the `raster` feature.
- Slot contracts and every load-time document rule come from
  `petramond_ui::contract`, the same functions the game's loader calls; the
  builder keeps no copy of any engine table.
