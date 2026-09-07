# Plant texture sources

`hemp.aseprite` exports to `assets/textures/hemp.png` at 16×16 with binary
transparency. Keep the desaturated palette with slightly blue shadows and
neutral bright tips: the atlas row's `grass` tint supplies the biome hue, the
cool shadow bias (as on short grass) keeps depth, and the bright tips keep
hemp readable up close.

Wild hemp and mature cultivated hemp share identical pixels. When revising the
mature plant, update farming's `hemp_3.aseprite` and its PNG too (the
cultivated stages live under `mods-src/farming/pack/textures/sources/plants/`).
