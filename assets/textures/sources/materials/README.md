# Material texture sources

Editable Aseprite sources for the core material tiles. Each basename exports
to the matching `assets/textures/<basename>.png` at 16×16. Open the
`.aseprite` source when editing: Aseprite reads consecutively numbered PNG
exports as an animation sequence.

- Keep the grass top and the grass side overlay neutral grayscale for biome
  tinting. The side overlay keeps its transparency; every other export is
  opaque. Grass-side and snowy-side artwork share the authored dirt base.
- Lit furnace pixels differ from the unlit face only inside the lower opening;
  preserve the shared shell.
- Preserve names, dimensions, alpha contracts and species-specific grain
  direction (jungle planks run vertically).
- Stone's alternatives (`variation: "cell"` in the atlas) share one palette.
  Check every ordered pair's opposing edges, self-pairs included, and mixed
  surfaces at gameplay distance: matching one edge across alternatives does
  not make opposite edges join. Avoid isolated darkest flecks, border
  outlines, and average-brightness differences that reveal block cells.
- Check tiled repetition and the native renderer after any change.
