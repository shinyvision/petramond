//! The `blocks.json` shape loader: the raw authored forms (`RawShape`,
//! `RawBox`, `RawCustomShape`) and their resolution into registry params.
//!
//! Kept apart from the vocabulary it produces, the way `block/load.rs` is kept
//! apart from `block.rs`.

use super::corner_form::{donor_list, intersect_lists, turned_list, union_bounds, union_lists};
use super::*;

/// The `shape` field of a `blocks.json` row, before resolution to a
/// [`BlockShapeKind`]. A bare family name (`"cube"`, `"stair"`, …), or an
/// externally-tagged parameterized form (`{"lowered_cube": 15}`,
/// `{"model": "petramond:bed"}`). Resolved by [`resolve`](Self::resolve) at
/// load. A parameterized kind adds a `{"custom": {...}}` variant here.
/// Serialize is kept (derived) for `RawBlockDef`'s derive; deserialize is manual
/// so a bare namespaced string (`"mymod:gate"`) resolves to [`RawShape::Named`],
/// the custom-shape reference, alongside the enum forms.
#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RawShape {
    Cube,
    Boxes(Vec<RawBox>),
    Cross,
    Crop,
    Torch,
    Stair,
    Slab,
    Pane,
    Fence,
    Ladder,
    Model(BlockModelKind),
    Door,
    /// A mod-parameterized connection shape: `{"custom": {"family":
    /// "fence", "post_thickness": 6, …}}`.
    Custom(RawCustomShape),
    /// A custom shape referenced by name (`"shape": "mymod:gate"`),
    /// declared in the pack's `shapes.json`.
    Named(String),
}

/// The most boxes one authored shape may list. Shares the guest-bake cap: a
/// static shape and a WASM-baked one land in the same mesher and the same
/// per-cell budget.
const MAX_AUTHORED_BOXES: usize = crate::world::shape_bake_validate::MAX_SHAPE_BOXES;

/// Resolve `{"boxes": [...]}` to its family, leaked params, and canonical key.
/// The key spells the whole authored list (and the corners flag), so two rows
/// with identical boxes share ONE shape kind (every plain cactus is one row in
/// the table) and two that differ never collide.
fn resolve_box_set(
    raw: &[RawBox],
    corners: bool,
) -> Result<(ShapeFamily, ShapeParams, String), String> {
    if raw.is_empty() {
        return Err("a 'boxes' shape needs at least one box".into());
    }
    if raw.len() > MAX_AUTHORED_BOXES {
        return Err(format!(
            "a 'boxes' shape may list at most {MAX_AUTHORED_BOXES} boxes, got {}",
            raw.len()
        ));
    }
    let boxes: Vec<BoxDef> = raw.iter().map(RawBox::resolve).collect::<Result<_, _>>()?;
    let key = format!(
        "#boxes/{}",
        boxes
            .iter()
            .map(|b| {
                let t = |v: f32| (v * 16.0).round() as u8;
                let faces: String = b.faces.iter().map(|&f| if f { '1' } else { '0' }).collect();
                // Tiles are part of the shape's identity: two rows whose boxes
                // agree but whose face art does not are different kinds.
                let tiles: String = b
                    .tiles
                    .iter()
                    .map(|t| t.map_or(String::new(), |t| format!(".{}", t.index())))
                    .collect();
                format!(
                    "{},{},{}-{},{},{}:{faces}{}{}{}{tiles}",
                    t(b.aabb.min[0]),
                    t(b.aabb.min[1]),
                    t(b.aabb.min[2]),
                    t(b.aabb.max[0]),
                    t(b.aabb.max[1]),
                    t(b.aabb.max[2]),
                    if b.collides { "c" } else { "" },
                    if b.occludes { "o" } else { "" },
                    if b.double_sided { "d" } else { "" }
                )
            })
            .collect::<Vec<_>>()
            .join("/")
    ) + if corners { "+corners" } else { "" };
    // The AUTHORED-space forms, the stair rule lifted to box lists: straight,
    // outer = self INTERSECT quarter-turned self (the matter both orientations
    // agree on), inner = self UNION quarter-turned self — one clockwise and
    // one counter-clockwise of each corner. A kind that does not corner-join
    // has exactly ONE form; the five slots stay so indexing is uniform, but
    // they share one list rather than five identical copies of it.
    let authored: &'static [BoxDef] = Box::leak(boxes.clone().into_boxed_slice());
    let mut authored_forms: [&'static [BoxDef]; 5] = [authored; 5];
    if corners {
        let cw = donor_list(&boxes, 1);
        let ccw = donor_list(&boxes, 3);
        let composed = [
            intersect_lists(&boxes, &cw),
            intersect_lists(&boxes, &ccw),
            union_lists(&boxes, &cw),
            union_lists(&boxes, &ccw),
        ];
        for (slot, list) in authored_forms[1..].iter_mut().zip(composed) {
            if list.len() > MAX_AUTHORED_BOXES {
                return Err(format!(
                    "a corner form of this shape needs {} boxes (max {MAX_AUTHORED_BOXES})",
                    list.len()
                ));
            }
            *slot = Box::leak(list.into_boxed_slice());
        }
    }
    // Every (turn, form) variant is resolved HERE, at load: composed in
    // authored space, then the whole list is turned. Nothing composes or
    // rotates per cell, per frame or per collision query, and
    // `collision_boxes` can still hand out a `&'static`. Only the DISTINCT
    // forms are turned and leaked — a plain box set pays four lists, not
    // twenty identical ones.
    let mut forms: [[&'static [BoxDef]; 5]; 4] = [[&[]; 5]; 4];
    let mut collision: [[&'static [Aabb]; 5]; 4] = [[&[]; 5]; 4];
    // Every slot is written below; this is only the array's initial value.
    let mut bounds = [[Aabb {
        min: [0.0; 3],
        max: [0.0; 3],
    }; 5]; 4];
    for f in 0..if corners { 5 } else { 1 } {
        let mut set: &'static [BoxDef] = authored_forms[f];
        for (t, ((forms, collision), bounds)) in forms
            .iter_mut()
            .zip(collision.iter_mut())
            .zip(bounds.iter_mut())
            .enumerate()
        {
            if t > 0 {
                set = Box::leak(turned_list(set, 1).into_boxed_slice());
            }
            forms[f] = set;
            let c: Vec<Aabb> = set.iter().filter(|b| b.collides).map(|b| b.aabb).collect();
            collision[f] = Box::leak(c.into_boxed_slice());
            bounds[f] = union_bounds(set);
        }
    }
    if !corners {
        for t in 0..4 {
            forms[t] = [forms[t][0]; 5];
            collision[t] = [collision[t][0]; 5];
            bounds[t] = [bounds[t][0]; 5];
        }
    }
    let params: &'static BoxSetParams = Box::leak(Box::new(BoxSetParams {
        forms,
        collision,
        bounds,
        corner_joins: corners,
    }));
    Ok((ShapeFamily::BoxSet, ShapeParams::BoxSet(params), key))
}

/// Whether a family resolves to a box set — the shape-kind row's
/// [`resolves_to_boxes`](ShapeKindDef::resolves_to_boxes), reachable at LOAD
/// time (before the kind table is installed) so the loader can mirror it onto
/// the dense block flags.
pub fn family_resolves_to_boxes(family: ShapeFamily) -> bool {
    families::resolves_to_boxes(family)
}

/// One box of a `{"boxes": [...]}` shape, as authored. Extents are TEXELS
/// (`0..=16`); `from` defaults to the cell origin and `to` to the far corner,
/// so a plain full cube is `{}` and farmland is `{"to": [16, 15, 16]}`.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RawBox {
    #[serde(default)]
    pub from: Option<[u8; 3]>,
    #[serde(default)]
    pub to: Option<[u8; 3]>,
    /// Which faces this box draws: any of `up`, `down`, `sides`, `all`, or the
    /// individual `+x`/`-x`/`+y`/`-y`/`+z`/`-z`. Absent = all six.
    #[serde(default)]
    pub faces: Option<Vec<String>>,
    /// Per-face tile overrides, keyed by the same face names as `faces`
    /// (`{"up": "mymod:shelf_top"}`). Absent faces keep the row's
    /// `[top, bottom, side]`. Naming a face the box does not draw is a load
    /// error — it is always a typo, never a no-op worth shipping.
    #[serde(default)]
    pub tiles: Option<std::collections::BTreeMap<String, String>>,
    /// Whether the box is matter — shadows and blocks light (default yes).
    #[serde(default = "yes")]
    pub occludes: bool,
    /// Whether the box obstructs movement (default yes).
    #[serde(default = "yes")]
    pub collides: bool,
    /// Draw the box's faces from both sides (default no) — for a CUTOUT face
    /// whose art must stay whole from every angle.
    #[serde(default)]
    pub double_sided: bool,
}

fn yes() -> bool {
    true
}

/// The canonical face indices a box's face name covers (`+X, -X, +Y, -Y, +Z,
/// -Z` order). One vocabulary for both `faces` and `tiles`, so a name that
/// selects faces to draw selects the same faces to texture.
fn face_group(name: &str) -> Result<&'static [usize], String> {
    Ok(match name {
        "all" => &[0, 1, 2, 3, 4, 5],
        "sides" => &[0, 1, 4, 5],
        "up" | "+y" => &[2],
        "down" | "-y" => &[3],
        "+x" => &[0],
        "-x" => &[1],
        "+z" => &[4],
        "-z" => &[5],
        other => {
            return Err(format!(
                "unknown box face '{other}' (expected all, sides, up, down, \
                 or +x/-x/+y/-y/+z/-z)"
            ))
        }
    })
}

impl RawBox {
    /// Resolve to the engine form, validating extents and face names.
    fn resolve(&self) -> Result<BoxDef, String> {
        let texel = |v: u8, name: &str| -> Result<f32, String> {
            if v > 16 {
                return Err(format!("box {name} {v} out of range (0..=16 texels)"));
            }
            Ok(v as f32 / 16.0)
        };
        let from = self.from.unwrap_or([0, 0, 0]);
        let to = self.to.unwrap_or([16, 16, 16]);
        let mut min = [0.0f32; 3];
        let mut max = [0.0f32; 3];
        for a in 0..3 {
            min[a] = texel(from[a], "from")?;
            max[a] = texel(to[a], "to")?;
            if from[a] >= to[a] {
                return Err(format!(
                    "box axis {a} is empty ({} .. {}) — 'from' must be below 'to'",
                    from[a], to[a]
                ));
            }
        }
        // Canonical face order: +X, -X, +Y, -Y, +Z, -Z.
        let mut faces = [self.faces.is_none(); 6];
        for name in self.faces.iter().flatten() {
            for &i in face_group(name)? {
                faces[i] = true;
            }
        }
        let mut tiles = [None; 6];
        for (name, tile) in self.tiles.iter().flatten() {
            let resolved =
                Tile::from_name(tile).ok_or_else(|| format!("unknown box face tile '{tile}'"))?;
            for &i in face_group(name)? {
                if !faces[i] {
                    return Err(format!(
                        "box face tile '{name}' names a face the box does not draw"
                    ));
                }
                tiles[i] = Some(resolved);
            }
        }
        Ok(BoxDef {
            aabb: Aabb { min, max },
            faces,
            tiles,
            occludes: self.occludes,
            collides: self.collides,
            double_sided: self.double_sided,
            // Authored geometry: every face's art is in the shape's own frame.
            // Only a corner form's inherited faces ever offset this.
            art_turns: [0; 6],
        })
    }
}

/// The body of a `{"custom": {…}}` shape: a parameterized member of an existing
/// family (no WASM). Dimensions are in texels (`0..=16`).
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RawCustomShape {
    /// The family to parameterize: `"fence"` or `"pane"`.
    pub family: String,
    /// Post thickness in texels (fence default 4, pane default 2).
    #[serde(default)]
    pub post_thickness: Option<u8>,
    /// Post low-edge offset in texels; centred when omitted.
    #[serde(default)]
    pub post_offset: Option<u8>,
    /// `"opaque_or_same_family"` | `"solid_or_same_family"` | `"same_family_only"`
    /// | `"never"`. Defaults per family (fence opaque, pane solid).
    #[serde(default)]
    pub connection_rule: Option<String>,
    /// `"segment"` | `"sprite"` | `"cube"`. Defaults per family.
    #[serde(default)]
    pub item_form: Option<String>,
    /// Cross/crop billboard-plane inset from the cell edge, texels (cross
    /// default 0 = full-cell; crop default 2).
    #[serde(default)]
    pub inset: Option<u8>,
    /// Cross plane count — the diagonal cross is two planes; only `2` is valid.
    #[serde(default)]
    pub plane_count: Option<u8>,
    /// Crop lattice vertical drop, texels (default 1).
    #[serde(default)]
    pub drop: Option<u8>,
    /// Wall-panel thickness, texels (default 1 — the ladder slab).
    #[serde(default)]
    pub thickness: Option<u8>,
    /// Wall-panel / crop visible height, texels (default 16 = full).
    #[serde(default)]
    pub height: Option<u8>,
}

impl RawShape {
    /// Resolve this raw shape to its `(family, params, canonical key)`.
    /// `corners` is the row's corner-joining flag; only a box set consumes
    /// it, so any other shape refuses it.
    pub fn resolve(
        &self,
        corners: bool,
    ) -> Result<(ShapeFamily, ShapeParams, String), String> {
        if corners && !matches!(self, RawShape::Boxes(_)) {
            return Err("'corners' requires a '{\"boxes\": [...]}' shape".into());
        }
        Ok(match self {
            RawShape::Cube => (
                ShapeFamily::Cube,
                ShapeParams::None,
                "petramond:cube".into(),
            ),
            RawShape::Boxes(raw) => resolve_box_set(raw, corners)?,
            RawShape::Cross => (
                ShapeFamily::Cross,
                ShapeParams::None,
                "petramond:cross".into(),
            ),
            RawShape::Crop => (
                ShapeFamily::Crop,
                ShapeParams::None,
                "petramond:crop".into(),
            ),
            RawShape::Torch => (
                ShapeFamily::Torch,
                ShapeParams::None,
                "petramond:torch".into(),
            ),
            RawShape::Stair => (
                ShapeFamily::Stair,
                ShapeParams::None,
                "petramond:stair".into(),
            ),
            RawShape::Slab => (
                ShapeFamily::Slab,
                ShapeParams::None,
                "petramond:slab".into(),
            ),
            RawShape::Pane => (
                ShapeFamily::Pane,
                ShapeParams::Connection(&ENGINE_PANE_PARAMS),
                "petramond:pane".into(),
            ),
            RawShape::Fence => (
                ShapeFamily::Fence,
                ShapeParams::Connection(&ENGINE_FENCE_PARAMS),
                "petramond:fence".into(),
            ),
            RawShape::Ladder => (
                ShapeFamily::Ladder,
                ShapeParams::None,
                "petramond:ladder".into(),
            ),
            RawShape::Model(kind) => (
                ShapeFamily::Model,
                ShapeParams::Model { kind: *kind },
                format!("petramond:model/{}", crate::block_model::def(*kind).key),
            ),
            RawShape::Door => (
                ShapeFamily::Door,
                ShapeParams::None,
                "petramond:door".into(),
            ),
            RawShape::Custom(c) => c.resolve()?,
            RawShape::Named(key) => {
                let def = custom::by_key(key).ok_or_else(|| {
                    format!("unknown custom shape '{key}' (declare it in the pack's shapes.json)")
                })?;
                (ShapeFamily::Custom, ShapeParams::Custom(def), key.clone())
            }
        })
    }
}

impl RawCustomShape {
    fn resolve(&self) -> Result<(ShapeFamily, ShapeParams, String), String> {
        match self.family.as_str() {
            "fence" | "pane" => self.resolve_connection(),
            "cross" => self.resolve_cross(),
            "crop" => self.resolve_crop(),
            "wall_panel" => self.resolve_wall_panel(),
            other => Err(format!(
                "unknown custom shape family '{other}' \
                 (expected 'fence', 'pane', 'cross', 'crop', or 'wall_panel')"
            )),
        }
    }

    /// A texel dimension (`0..=16`) as a cell fraction, or its default.
    fn texel(&self, value: Option<u8>, default: u8, name: &str) -> Result<f32, String> {
        let v = value.unwrap_or(default);
        if v > 16 {
            return Err(format!("{name} {v} out of range (0..=16)"));
        }
        Ok(v as f32 / 16.0)
    }

    /// Error on any of the listed `(name, present)` fields that is set. Each
    /// family lists the parameters OUTSIDE its own vocabulary, so a misplaced
    /// field (a `height` on a cross, an `inset` on a wall panel) is a load error
    /// rather than a value the resolver silently drops.
    fn reject_fields(&self, fields: &[(&str, bool)]) -> Result<(), String> {
        if let Some((name, _)) = fields.iter().find(|(_, present)| *present) {
            return Err(format!("family '{}' takes no '{name}' field", self.family));
        }
        Ok(())
    }

    /// Reject the connection-only fields on a dimension family (a stray
    /// `post_thickness` or `item_form` on a crop is almost certainly a mistake).
    fn reject_connection_fields(&self) -> Result<(), String> {
        self.reject_fields(&[
            ("post_thickness", self.post_thickness.is_some()),
            ("post_offset", self.post_offset.is_some()),
            ("connection_rule", self.connection_rule.is_some()),
            ("item_form", self.item_form.is_some()),
        ])
    }

    /// Reject the dimension fields on a connection family (fence/pane take only
    /// the post/rule/item vocabulary).
    fn reject_dimension_fields(&self) -> Result<(), String> {
        self.reject_fields(&[
            ("inset", self.inset.is_some()),
            ("plane_count", self.plane_count.is_some()),
            ("drop", self.drop.is_some()),
            ("thickness", self.thickness.is_some()),
            ("height", self.height.is_some()),
        ])
    }

    /// `cross`: a two-plane diagonal billboard, `inset` texels in from the edges.
    fn resolve_cross(&self) -> Result<(ShapeFamily, ShapeParams, String), String> {
        self.reject_connection_fields()?;
        // Cross reads only `inset` + `plane_count`.
        self.reject_fields(&[
            ("drop", self.drop.is_some()),
            ("thickness", self.thickness.is_some()),
            ("height", self.height.is_some()),
        ])?;
        if let Some(pc) = self.plane_count {
            if pc != 2 {
                return Err(format!("cross plane_count {pc} unsupported (only 2)"));
            }
        }
        let inset = self.texel(self.inset, 0, "inset")?;
        if inset >= 0.5 {
            return Err("cross inset must be under 8 texels".into());
        }
        let params = Box::leak(Box::new(DimensionParams {
            inset,
            drop: 0.0,
            thickness: 0.0,
            height: 1.0,
        }));
        let key = format!("#custom/cross/inset{}", self.inset.unwrap_or(0));
        Ok((ShapeFamily::Cross, ShapeParams::Dimensions(params), key))
    }

    /// `crop`: a four-plane lattice, `inset` in from the edges and `drop` texels
    /// toward the floor (the engine crop is inset 2, drop 1).
    fn resolve_crop(&self) -> Result<(ShapeFamily, ShapeParams, String), String> {
        self.reject_connection_fields()?;
        // Crop reads only `inset` + `drop`.
        self.reject_fields(&[
            ("plane_count", self.plane_count.is_some()),
            ("thickness", self.thickness.is_some()),
            ("height", self.height.is_some()),
        ])?;
        let inset = self.texel(self.inset, 2, "inset")?;
        let drop = self.texel(self.drop, 1, "drop")?;
        if inset >= 0.5 {
            return Err("crop inset must be under 8 texels".into());
        }
        let params = Box::leak(Box::new(DimensionParams {
            inset,
            drop,
            thickness: 0.0,
            height: 1.0,
        }));
        let key = format!(
            "#custom/crop/inset{}/drop{}",
            self.inset.unwrap_or(2),
            self.drop.unwrap_or(1)
        );
        Ok((ShapeFamily::Crop, ShapeParams::Dimensions(params), key))
    }

    /// `wall_panel`: the ladder family with a retuned slab `thickness` and
    /// `height` (the engine ladder is thickness 1, height 16). Facing is per-cell
    /// block state, as for the ladder.
    fn resolve_wall_panel(&self) -> Result<(ShapeFamily, ShapeParams, String), String> {
        self.reject_connection_fields()?;
        // Wall panel reads only `thickness` + `height`.
        self.reject_fields(&[
            ("inset", self.inset.is_some()),
            ("plane_count", self.plane_count.is_some()),
            ("drop", self.drop.is_some()),
        ])?;
        let thickness = self.texel(self.thickness, 1, "thickness")?;
        let height = self.texel(self.height, 16, "height")?;
        if thickness == 0.0 {
            return Err("wall_panel thickness must be at least 1 texel".into());
        }
        if height == 0.0 {
            return Err("wall_panel height must be at least 1 texel".into());
        }
        let params = Box::leak(Box::new(DimensionParams {
            inset: 0.0,
            drop: 0.0,
            thickness,
            height,
        }));
        let key = format!(
            "#custom/wall_panel/th{}/h{}",
            self.thickness.unwrap_or(1),
            self.height.unwrap_or(16)
        );
        Ok((ShapeFamily::Ladder, ShapeParams::Dimensions(params), key))
    }

    /// `fence` / `pane`: the parameterized connection families.
    fn resolve_connection(&self) -> Result<(ShapeFamily, ShapeParams, String), String> {
        self.reject_dimension_fields()?;
        let family = match self.family.as_str() {
            "fence" => ShapeFamily::Fence,
            "pane" => ShapeFamily::Pane,
            other => {
                return Err(format!(
                    "unknown custom shape family '{other}' (expected 'fence' or 'pane')"
                ))
            }
        };
        let default_thickness = if family == ShapeFamily::Fence { 4 } else { 2 };
        let thickness = self.post_thickness.unwrap_or(default_thickness);
        if !(1..=16).contains(&thickness) {
            return Err(format!("post_thickness {thickness} out of range (1..=16)"));
        }
        let offset = self.post_offset.unwrap_or((16 - thickness) / 2);
        if offset as u16 + thickness as u16 > 16 {
            return Err(format!(
                "post_offset {offset} + post_thickness {thickness} exceeds 16"
            ));
        }
        let post_lo = offset as f32 / 16.0;
        let post_hi = (offset + thickness) as f32 / 16.0;
        let rule = match self.connection_rule.as_deref() {
            None if family == ShapeFamily::Fence => ConnectionRule::OpaqueOrSame,
            None => ConnectionRule::SolidOrSame,
            Some("opaque_or_same_family") => ConnectionRule::OpaqueOrSame,
            Some("solid_or_same_family") => ConnectionRule::SolidOrSame,
            Some("same_family_only") => ConnectionRule::SameOnly,
            Some("never") => ConnectionRule::Never,
            Some(other) => return Err(format!("unknown connection_rule '{other}'")),
        };
        let item_form = match self.item_form.as_deref() {
            None if family == ShapeFamily::Fence => ItemForm::Segment,
            None => ItemForm::Sprite,
            Some("segment") => ItemForm::Segment,
            Some("sprite") => ItemForm::Sprite,
            Some("cube") => ItemForm::Cube,
            Some(other) => return Err(format!("unknown item_form '{other}'")),
        };
        // Only the fence family builds a no-neighbour item segment (posts +
        // rails); a pane/bar with `item_form: "segment"` has no such geometry.
        if item_form == ItemForm::Segment && family != ShapeFamily::Fence {
            return Err("item_form 'segment' requires the 'fence' family".into());
        }
        // A mod's custom shape leaks its box table + params once (deduped by the
        // interner key, so identical customs share one).
        let boxes: &'static [connect::Shape; 16] =
            Box::leak(Box::new(connect::make_shapes(post_lo, post_hi)));
        let params: &'static ConnectionParams = Box::leak(Box::new(ConnectionParams {
            post_lo,
            post_hi,
            rule,
            item_form,
            boxes,
        }));
        let key = format!(
            "#custom/{}/off{offset}/th{thickness}/{rule:?}/{item_form:?}",
            self.family
        );
        Ok((family, ShapeParams::Connection(params), key))
    }
}
