use super::corner_form::{donor_list, intersect_lists, turned_list, union_bounds, union_lists};
use super::*;

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
    Trapdoor,
    Custom(RawCustomShape),
    Named(String),
    Run(RawRun),
}

const MAX_AUTHORED_BOXES: usize = crate::world::shape_bake_validate::MAX_SHAPE_BOXES;

fn resolve_box_set(
    raw: &[RawBox],
    corners: bool,
) -> Result<(ShapeFamily, ShapeParams, String), String> {
    let boxes = resolve_box_list(raw)?;
    if corners && boxes.iter().any(|b| b.pose.is_some()) {
        return Err("'corners' cannot compose a shape with rotated boxes".into());
    }
    let key = format!("#boxes/{}", box_list_key(&boxes)) + if corners { "+corners" } else { "" };
    // Stair rule for box lists: outer = self INTERSECT quarter-turned self, inner = self UNION
    // quarter-turned self, one clockwise and one counter-clockwise per corner.
    // A kind that doesn't corner-join has a single form shared by all five slots, so indexing
    // stays uniform.
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
    let mut targets: [[&'static [crate::block::PosedBox]; 5]; 4] = [[&[]; 5]; 4];
    let mut bounds = [[Aabb {
        min: [0.0; 3],
        max: [0.0; 3],
    }; 5]; 4];
    for f in 0..if corners { 5 } else { 1 } {
        let mut set: &'static [BoxDef] = authored_forms[f];
        for (t, (((forms, collision), targets), bounds)) in forms
            .iter_mut()
            .zip(collision.iter_mut())
            .zip(targets.iter_mut())
            .zip(bounds.iter_mut())
            .enumerate()
        {
            if t > 0 {
                set = Box::leak(turned_list(set, 1).into_boxed_slice());
            }
            forms[f] = set;
            let c: Vec<Aabb> = set.iter().filter_map(BoxDef::collision_volume).collect();
            collision[f] = Box::leak(c.into_boxed_slice());
            let g: Vec<crate::block::PosedBox> = set.iter().map(BoxDef::target).collect();
            targets[f] = Box::leak(g.into_boxed_slice());
            bounds[f] = union_bounds(set);
        }
    }
    if !corners {
        for t in 0..4 {
            forms[t] = [forms[t][0]; 5];
            collision[t] = [collision[t][0]; 5];
            targets[t] = [targets[t][0]; 5];
            bounds[t] = [bounds[t][0]; 5];
        }
    }
    let params: &'static BoxSetParams = Box::leak(Box::new(BoxSetParams {
        forms,
        collision,
        targets,
        bounds,
        refine: if corners {
            BoxSetRefine::Corners
        } else {
            BoxSetRefine::None
        },
    }));
    Ok((ShapeFamily::BoxSet, ShapeParams::BoxSet(params), key))
}

fn resolve_box_list(raw: &[RawBox]) -> Result<Vec<BoxDef>, String> {
    if raw.is_empty() {
        return Err("a box list needs at least one box".into());
    }
    if raw.len() > MAX_AUTHORED_BOXES {
        return Err(format!(
            "a box list may hold at most {MAX_AUTHORED_BOXES} boxes, got {}",
            raw.len()
        ));
    }
    raw.iter().map(RawBox::resolve).collect()
}

fn box_list_key(boxes: &[BoxDef]) -> String {
    boxes
        .iter()
        .map(|b| {
            let t = |v: f32| format!("{}", (v * 16.0 * 1000.0).round() / 1000.0);
            let faces: String = b.faces.iter().map(|&f| if f { '1' } else { '0' }).collect();
            let tiles: String = b
                .tiles
                .iter()
                .map(|t| t.map_or(String::new(), |t| format!(".{}", t.index())))
                .collect();
            let uv: String =
                b.uv.iter()
                    .zip(b.uv_turns)
                    .map(|(r, turns)| {
                        format!(
                            ".{}{}",
                            r.map_or(String::new(), |r| format!("{r:?}")),
                            if turns != 0 {
                                format!("r{turns}")
                            } else {
                                String::new()
                            }
                        )
                    })
                    .collect();
            let pose = b.pose.map_or(String::new(), |p| {
                let q = p.rotation.to_array().map(t);
                let o = p.origin.to_array().map(t);
                format!("@{}|{}", q.join(","), o.join(","))
            });
            format!(
                "{},{},{}-{},{},{}:{faces}{}{}{}{}{tiles}{uv}{pose}",
                t(b.aabb.min[0]),
                t(b.aabb.min[1]),
                t(b.aabb.min[2]),
                t(b.aabb.max[0]),
                t(b.aabb.max[1]),
                t(b.aabb.max[2]),
                if b.collides { "c" } else { "" },
                if b.occludes { "o" } else { "" },
                if b.double_sided { "d" } else { "" },
                if b.casts_ao { "" } else { "n" }
            )
        })
        .collect::<Vec<_>>()
        .join("/")
}

/// The body of a `{"run": {...}}` shape: a box set whose form follows the
/// cell's place in a same-kind vertical run. The forms are authored STANDING
/// (rooted below, tapering upward, texel `y = 0` at the floor, like every
/// other box shape); a row rooted `"up"` gets the same forms mirrored about
/// the cell's mid-plane, so one authored set serves a stalagmite and its
/// stalactite.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RawRun {
    pub root: String,
    pub forms: RawRunForms,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RawRunForms {
    pub tip: Vec<RawBox>,
    pub frustum: Vec<RawBox>,
    pub middle: Vec<RawBox>,
    pub base: Vec<RawBox>,
    #[serde(default)]
    pub merge: Option<Vec<RawBox>>,
}

fn resolve_run(raw: &RawRun) -> Result<(ShapeFamily, ShapeParams, String), String> {
    let root = match raw.root.as_str() {
        "up" => RunRoot::Up,
        "down" => RunRoot::Down,
        other => return Err(format!("run root '{other}' must be 'up' or 'down'")),
    };
    let f = &raw.forms;
    let named: [(&str, &[RawBox]); 5] = [
        ("tip", &f.tip),
        ("frustum", &f.frustum),
        ("middle", &f.middle),
        ("base", &f.base),
        ("merge", f.merge.as_deref().unwrap_or(&f.tip)),
    ];
    let mut key = format!("#run/{}", root.name());
    let mut authored: [&'static [BoxDef]; 5] = [&[]; 5];
    for (slot, (name, list)) in authored.iter_mut().zip(named) {
        let boxes = resolve_box_list(list).map_err(|e| format!("run form '{name}': {e}"))?;
        let boxes = match root {
            RunRoot::Down => boxes,
            RunRoot::Up => boxes.iter().map(BoxDef::mirrored_y).collect(),
        };
        key.push_str(&format!("/{name}={}", box_list_key(&boxes)));
        *slot = Box::leak(boxes.into_boxed_slice());
    }
    let mut collision: [&'static [Aabb]; 5] = [&[]; 5];
    let mut targets: [&'static [crate::block::PosedBox]; 5] = [&[]; 5];
    let mut bounds = [Aabb {
        min: [0.0; 3],
        max: [0.0; 3],
    }; 5];
    for form in 0..5 {
        let set = authored[form];
        let c: Vec<Aabb> = set.iter().filter_map(BoxDef::collision_volume).collect();
        collision[form] = Box::leak(c.into_boxed_slice());
        let g: Vec<crate::block::PosedBox> = set.iter().map(BoxDef::target).collect();
        targets[form] = Box::leak(g.into_boxed_slice());
        bounds[form] = union_bounds(set);
    }
    let params: &'static BoxSetParams = Box::leak(Box::new(BoxSetParams {
        forms: [authored; 4],
        collision: [collision; 4],
        targets: [targets; 4],
        bounds: [bounds; 4],
        refine: BoxSetRefine::Run(RunParams { root }),
    }));
    Ok((ShapeFamily::BoxSet, ShapeParams::BoxSet(params), key))
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RawBox {
    #[serde(default)]
    pub from: Option<[f32; 3]>,
    #[serde(default)]
    pub to: Option<[f32; 3]>,
    #[serde(default)]
    pub rotation: Option<[f32; 3]>,
    #[serde(default)]
    pub origin: Option<[f32; 3]>,
    #[serde(default)]
    pub uv: Option<std::collections::BTreeMap<String, [u8; 4]>>,
    #[serde(default)]
    pub uv_rotation: Option<std::collections::BTreeMap<String, u16>>,
    #[serde(default)]
    pub faces: Option<Vec<String>>,
    #[serde(default)]
    pub tiles: Option<std::collections::BTreeMap<String, String>>,
    #[serde(default = "yes")]
    pub occludes: bool,
    #[serde(default = "yes")]
    pub collides: bool,
    #[serde(default)]
    pub double_sided: bool,
    #[serde(default = "yes")]
    pub casts_ao: bool,
}

fn yes() -> bool {
    true
}

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

const OVERHANG_TEXELS: f32 = 16.0;

impl RawBox {
    fn resolve(&self) -> Result<BoxDef, String> {
        let texel = |v: f32, name: &str| -> Result<f32, String> {
            if !v.is_finite() || !(-OVERHANG_TEXELS..=16.0 + OVERHANG_TEXELS).contains(&v) {
                return Err(format!(
                    "box {name} {v} out of range ({}..={} texels)",
                    -OVERHANG_TEXELS,
                    16.0 + OVERHANG_TEXELS
                ));
            }
            Ok(v / 16.0)
        };
        let from = self.from.unwrap_or([0.0, 0.0, 0.0]);
        let to = self.to.unwrap_or([16.0, 16.0, 16.0]);
        let mut min = [0.0f32; 3];
        let mut max = [0.0f32; 3];
        let mut flat_axes = 0;
        for a in 0..3 {
            min[a] = texel(from[a], "from")?;
            max[a] = texel(to[a], "to")?;
            if from[a] > to[a] {
                return Err(format!(
                    "box axis {a} is inverted ({} .. {}) — 'from' must not exceed 'to'",
                    from[a], to[a]
                ));
            }
            if from[a] == to[a] {
                flat_axes += 1;
            }
        }
        if flat_axes > 1 {
            return Err("a box may be flat on at most one axis (a plane, never a line)".into());
        }
        let pose = match self.rotation {
            Some(deg) if deg != [0.0; 3] => {
                if deg.iter().any(|d| !d.is_finite()) {
                    return Err("box rotation must be finite degrees".into());
                }
                let origin = match self.origin {
                    Some(o) => {
                        if o.iter().any(|v| !v.is_finite()) {
                            return Err("box origin must be finite texels".into());
                        }
                        o.map(|v| v / 16.0)
                    }
                    None => std::array::from_fn(|a| (min[a] + max[a]) * 0.5),
                };
                Some(crate::block::BoxPose::from_euler_degrees(deg, origin))
            }
            _ => {
                if self.origin.is_some() {
                    return Err("box 'origin' needs a 'rotation'".into());
                }
                None
            }
        };
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
        let mut uv = [None; 6];
        for (name, rect) in self.uv.iter().flatten() {
            if rect.iter().any(|&t| t > 16) {
                return Err(format!(
                    "box face uv '{name}' {rect:?} out of range (0..=16 texels)"
                ));
            }
            for &i in face_group(name)? {
                if !faces[i] {
                    return Err(format!(
                        "box face uv '{name}' names a face the box does not draw"
                    ));
                }
                uv[i] = Some(*rect);
            }
        }
        let mut uv_turns = [0u8; 6];
        for (name, deg) in self.uv_rotation.iter().flatten() {
            if deg % 90 != 0 || *deg >= 360 {
                return Err(format!(
                    "box face uv_rotation '{name}' {deg} must be 0, 90, 180 or 270"
                ));
            }
            for &i in face_group(name)? {
                if !faces[i] {
                    return Err(format!(
                        "box face uv_rotation '{name}' names a face the box does not draw"
                    ));
                }
                uv_turns[i] = (deg / 90) as u8;
            }
        }
        Ok(BoxDef {
            aabb: Aabb { min, max },
            faces,
            tiles,
            occludes: self.occludes,
            collides: self.collides,
            double_sided: self.double_sided,
            casts_ao: self.casts_ao,
            art_turns: [0; 6],
            uv,
            uv_turns,
            pose,
        })
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RawCustomShape {
    pub family: String,
    #[serde(default)]
    pub post_thickness: Option<u8>,
    #[serde(default)]
    pub post_offset: Option<u8>,
    #[serde(default)]
    pub connection_rule: Option<String>,
    #[serde(default)]
    pub item_form: Option<String>,
    #[serde(default)]
    pub inset: Option<u8>,
    #[serde(default)]
    pub plane_count: Option<u8>,
    #[serde(default)]
    pub drop: Option<u8>,
    #[serde(default)]
    pub thickness: Option<u8>,
    #[serde(default)]
    pub height: Option<u8>,
}

impl RawShape {
    pub fn resolve(&self, corners: bool) -> Result<(ShapeFamily, ShapeParams, String), String> {
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
            RawShape::Run(raw) => resolve_run(raw)?,
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
            RawShape::Trapdoor => (
                ShapeFamily::Trapdoor,
                ShapeParams::None,
                "petramond:trapdoor".into(),
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

    fn texel(&self, value: Option<u8>, default: u8, name: &str) -> Result<f32, String> {
        let v = value.unwrap_or(default);
        if v > 16 {
            return Err(format!("{name} {v} out of range (0..=16)"));
        }
        Ok(v as f32 / 16.0)
    }

    fn reject_fields(&self, fields: &[(&str, bool)]) -> Result<(), String> {
        if let Some((name, _)) = fields.iter().find(|(_, present)| *present) {
            return Err(format!("family '{}' takes no '{name}' field", self.family));
        }
        Ok(())
    }

    fn reject_connection_fields(&self) -> Result<(), String> {
        self.reject_fields(&[
            ("post_thickness", self.post_thickness.is_some()),
            ("post_offset", self.post_offset.is_some()),
            ("connection_rule", self.connection_rule.is_some()),
            ("item_form", self.item_form.is_some()),
        ])
    }

    fn reject_dimension_fields(&self) -> Result<(), String> {
        self.reject_fields(&[
            ("inset", self.inset.is_some()),
            ("plane_count", self.plane_count.is_some()),
            ("drop", self.drop.is_some()),
            ("thickness", self.thickness.is_some()),
            ("height", self.height.is_some()),
        ])
    }

    fn resolve_cross(&self) -> Result<(ShapeFamily, ShapeParams, String), String> {
        self.reject_connection_fields()?;
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

    fn resolve_crop(&self) -> Result<(ShapeFamily, ShapeParams, String), String> {
        self.reject_connection_fields()?;
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

    fn resolve_wall_panel(&self) -> Result<(ShapeFamily, ShapeParams, String), String> {
        self.reject_connection_fields()?;
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
        if item_form == ItemForm::Segment && family != ShapeFamily::Fence {
            return Err("item_form 'segment' requires the 'fence' family".into());
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn raw(text: &str) -> RawBox {
        serde_json::from_str(text).expect("box parses")
    }

    #[test]
    fn a_rotated_box_reserves_its_clipped_posed_bounds() {
        let plane = raw(
            r#"{"from":[0,8,-3.3137],"to":[16,8,19.3137],"rotation":[45,0,0],"origin":[8,8,8],"faces":["up"]}"#,
        )
        .resolve()
        .expect("resolves");
        assert!(plane.pose.is_some());
        assert!(plane.is_flat_plane());
        assert_eq!(
            plane.collision_volume(),
            None,
            "a flat plane never collides"
        );
        let b = plane.posed_bounds().clipped_to_cell().expect("in the cell");
        assert!(
            b.min[1] < 0.01 && b.max[1] > 0.99,
            "the slope spans the cell: {b:?}"
        );

        let slab = raw(r#"{"from":[0,7,0],"to":[16,9,16],"rotation":[0,0,90]}"#)
            .resolve()
            .expect("resolves");
        let c = slab.collision_volume().expect("a solid box collides");
        assert!((c.min[0] - 7.0 / 16.0).abs() < 1e-4 && (c.max[0] - 9.0 / 16.0).abs() < 1e-4);
        assert!(
            c.min[1] < 1e-4 && c.max[1] > 1.0 - 1e-4,
            "clipped to the cell: {c:?}"
        );
    }

    fn run_shape(root: &str) -> RawShape {
        serde_json::from_str(&format!(
            r#"{{"run":{{"root":"{root}","forms":{{
                "tip":[{{"from":[6,0,6],"to":[10,10,10]}},{{"from":[7,10,7],"to":[9,16,9],"faces":["sides","up"]}}],
                "frustum":[{{"from":[4,0,4],"to":[12,16,12]}}],
                "middle":[{{"from":[3,0,3],"to":[13,16,13]}}],
                "base":[{{"from":[2,0,2],"to":[14,4,14],"tiles":{{"down":"stone"}}}},{{"from":[3,4,3],"to":[13,16,13]}}]
            }}}}}}"#
        ))
        .expect("run parses")
    }

    #[test]
    fn a_hanging_run_is_the_standing_run_mirrored() {
        let (_, down, key_down) = run_shape("down").resolve(false).expect("standing resolves");
        let (_, up, key_up) = run_shape("up").resolve(false).expect("hanging resolves");
        assert_ne!(key_down, key_up, "the root is kind identity");
        let (down, up) = (down.box_set().unwrap(), up.box_set().unwrap());
        assert_eq!(down.run().unwrap().root, RunRoot::Down);
        assert_eq!(up.run().unwrap().root, RunRoot::Up);
        for form in 0..5u8 {
            let (a, b) = (down.boxes(0, form), up.boxes(0, form));
            assert_eq!(a.len(), b.len());
            for (d, u) in a.iter().zip(b) {
                assert_eq!(u.aabb.min[1], 1.0 - d.aabb.max[1]);
                assert_eq!(u.aabb.max[1], 1.0 - d.aabb.min[1]);
                assert_eq!(u.aabb.min[0], d.aabb.min[0]);
                assert_eq!(u.faces[2], d.faces[3], "up/down faces swap");
                assert_eq!(u.faces[3], d.faces[2]);
                assert_eq!(u.tiles[2], d.tiles[3], "a face's tile travels with it");
            }
            assert_eq!(down.boxes(1, form), down.boxes(0, form));
        }
        assert_eq!(down.boxes(0, RUN_MERGE), down.boxes(0, RUN_TIP));
        assert_eq!(down.collision(0, RUN_BASE).len(), 2);
        assert!(
            down.bounds(0, RUN_TIP).max[1] > 0.99,
            "the tip reaches the cell top"
        );
    }

    #[test]
    fn run_authoring_errors_are_load_errors() {
        assert!(
            serde_json::from_str::<RawShape>(r#"{"run":{"root":"north","forms":{"tip":[{}],"frustum":[{}],"middle":[{}],"base":[{}]}}}"#)
                .unwrap()
                .resolve(false)
                .is_err(),
            "a horizontal root"
        );
        assert!(
            serde_json::from_str::<RawShape>(r#"{"run":{"root":"up","forms":{"tip":[],"frustum":[{}],"middle":[{}],"base":[{}]}}}"#)
                .unwrap()
                .resolve(false)
                .is_err(),
            "an empty form"
        );
        assert!(
            serde_json::from_str::<RawShape>(
                r#"{"run":{"root":"up","forms":{"tip":[{}],"middle":[{}],"base":[{}]}}}"#
            )
            .is_err(),
            "a missing form"
        );
        assert!(run_shape("down").resolve(true).is_err(), "corners on a run");
    }

    #[test]
    fn rotated_box_authoring_errors_are_load_errors() {
        let posed = raw(r#"{"to":[16,8,16],"rotation":[0,45,0]}"#);
        assert!(
            resolve_box_set(&[posed], true).is_err(),
            "corners over a posed box"
        );
        assert!(raw(r#"{"uv_rotation":{"up":45}}"#).resolve().is_err());
        assert!(raw(r#"{"origin":[8,8,8]}"#).resolve().is_err());
        assert!(raw(r#"{"from":[0,8,8],"to":[16,8,8]}"#).resolve().is_err());
        assert!(
            raw(r#"{"from":[0,8,0],"to":[16,8,16]}"#).resolve().is_ok(),
            "one flat axis"
        );
        assert!(
            raw(r#"{"from":[0,0,-40]}"#).resolve().is_err(),
            "past the overhang room"
        );
    }
}
