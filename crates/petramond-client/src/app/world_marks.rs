//! Every client mod's world marks, composed each frame into the renderer's
//! [`WorldMarks`]: lines as they are, a point's body and label painted in
//! window pixels around it. The renderer draws them on the window only, so
//! they never reach a frame capture.

use mod_api::{ClientSprite, ClientWorldMark};
use petramond_math::world_pos::WorldPos;
use petramond_render::{ClientOverlayImage, WorldMark, WorldMarks};
use petramond_ui::RectI;

/// The backing a label sits on, so it reads over any sky or terrain.
const LABEL_BACKING: [f32; 4] = [0.0, 0.0, 0.0, 0.5];

pub(super) fn compose(marks: &mut WorldMarks, game: &crate::game::Game, gui_scale: i32) {
    marks.clear();
    let theme = petramond::gui::doc_theme::theme();
    game.for_each_client_world_mark(|mark, image| match mark {
        ClientWorldMark::Line {
            from,
            to,
            color,
            width,
            occluded,
        } => marks.items.push(WorldMark::Line {
            from: WorldPos::from_array(*from),
            to: WorldPos::from_array(*to),
            color: unit(*color),
            width: *width,
            occluded: f32::from(*occluded) / 255.0,
        }),
        ClientWorldMark::Point {
            pos,
            sprite,
            size,
            color,
            label,
            occluded,
        } => {
            let at = WorldPos::from_array(*pos);
            let occluded = f32::from(*occluded) / 255.0;
            let tint = unit(*color);
            let size = *size;
            if let (Some(ClientSprite::Image { .. }), Some(image), true) =
                (sprite, image, size > 0.0)
            {
                let w = size * f32::from(image.width) / f32::from(image.height);
                marks.items.push(WorldMark::Image {
                    at,
                    occluded,
                    tint,
                    image: ClientOverlayImage {
                        key: image.key.clone(),
                        size: (image.width, image.height),
                        rgba: image.rgba.clone(),
                        revision: image.revision,
                        recent_blits: image.recent_blits.clone(),
                        rect: [-w * 0.5, -size * 0.5, w, size],
                        uv: [0.0, 0.0, 1.0, 1.0],
                    },
                });
            }
            marks.paint(at, occluded, |list| {
                let mut painter = petramond_ui::Painter {
                    list,
                    scale: 1,
                    font: theme.ui_font(),
                };
                if size > 0.0 {
                    match sprite {
                        None => painter.solid(centred(size, size), tint, None),
                        Some(ClientSprite::Theme { part }) => {
                            let face = theme
                                .part(part)
                                .and_then(|p| p.face(petramond_ui::FaceState::Default));
                            if let Some(face) = face {
                                let [.., fw, fh] = face.rect;
                                let w = size * fw as f32 / fh.max(1) as f32;
                                let src = petramond_ui::SpriteSrc {
                                    tex: petramond_ui::TexId::ThemePage(face.page),
                                    rect: face.rect,
                                    tex_size: theme.page_size(face.page),
                                };
                                painter.sprite(
                                    &src,
                                    centred(w, size),
                                    petramond_ui::Fit::Stretch,
                                    petramond_ui::PaintStyle {
                                        color: tint,
                                        clip: None,
                                    },
                                );
                            }
                        }
                        Some(ClientSprite::Image { .. }) => {}
                    }
                }
                if let Some(label) = label {
                    paint_label(&mut painter, label, size, gui_scale.max(1), tint);
                }
            });
        }
    });
}

/// One line under the body (or centred on the point when there is none), in
/// the theme font at the window's GUI scale, on a translucent backing.
fn paint_label(
    painter: &mut petramond_ui::Painter<'_>,
    label: &str,
    body: f32,
    k: i32,
    color: [f32; 4],
) {
    let font = painter.font;
    let (w, h) = (font.width(label) * k, font.line_h() * k);
    let pad = k;
    let top = if body > 0.0 {
        (body * 0.5).ceil() as i32 + pad
    } else {
        -(h / 2) - pad
    };
    let text = RectI {
        x: -(w / 2),
        y: top + pad,
        w,
        h,
    };
    painter.solid(
        RectI {
            x: text.x - pad,
            y: top,
            w: w + 2 * pad,
            h: h + 2 * pad,
        },
        LABEL_BACKING,
        None,
    );
    painter.ellipsized_at(label, text, k, color, None);
}

/// A `w` × `h` pixel rect centred on the point.
fn centred(w: f32, h: f32) -> RectI {
    let (w, h) = (w.round().max(1.0) as i32, h.round().max(1.0) as i32);
    RectI {
        x: -(w / 2),
        y: -(h / 2),
        w,
        h,
    }
}

fn unit(color: [u8; 4]) -> [f32; 4] {
    color.map(|c| f32::from(c) / 255.0)
}
