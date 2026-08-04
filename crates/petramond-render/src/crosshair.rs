pub(super) const MAX_CROSSHAIR_VERTICES: usize = 30;

const ARM: f32 = 7.0;
const HALF_THICKNESS: f32 = 0.5;

pub(super) struct CrosshairVertices {
    pub vertices: [[f32; 2]; MAX_CROSSHAIR_VERTICES],
    pub count: u32,
}

pub(super) fn crosshair_vertices(width: u32, height: u32) -> CrosshairVertices {
    if width == 0 || height == 0 {
        return CrosshairVertices {
            vertices: [[0.0; 2]; MAX_CROSSHAIR_VERTICES],
            count: 0,
        };
    }

    let cx = width as f32 * 0.5;
    let cy = height as f32 * 0.5;
    let mut out = CrosshairVertices {
        vertices: [[0.0; 2]; MAX_CROSSHAIR_VERTICES],
        count: 0,
    };

    push_rect(
        &mut out,
        width,
        height,
        cx - HALF_THICKNESS,
        cy - HALF_THICKNESS,
        cx + HALF_THICKNESS,
        cy + HALF_THICKNESS,
    );
    push_rect(
        &mut out,
        width,
        height,
        cx - ARM - HALF_THICKNESS,
        cy - HALF_THICKNESS,
        cx - HALF_THICKNESS,
        cy + HALF_THICKNESS,
    );
    push_rect(
        &mut out,
        width,
        height,
        cx + HALF_THICKNESS,
        cy - HALF_THICKNESS,
        cx + ARM + HALF_THICKNESS,
        cy + HALF_THICKNESS,
    );
    push_rect(
        &mut out,
        width,
        height,
        cx - HALF_THICKNESS,
        cy - ARM - HALF_THICKNESS,
        cx + HALF_THICKNESS,
        cy - HALF_THICKNESS,
    );
    push_rect(
        &mut out,
        width,
        height,
        cx - HALF_THICKNESS,
        cy + HALF_THICKNESS,
        cx + HALF_THICKNESS,
        cy + ARM + HALF_THICKNESS,
    );

    out
}

fn push_rect(
    out: &mut CrosshairVertices,
    width: u32,
    height: u32,
    x0: f32,
    y0: f32,
    x1: f32,
    y1: f32,
) {
    let p0 = ndc(width, height, x0, y0);
    let p1 = ndc(width, height, x1, y0);
    let p2 = ndc(width, height, x1, y1);
    let p3 = ndc(width, height, x0, y1);
    push_tri(out, p0, p1, p2);
    push_tri(out, p0, p2, p3);
}

fn push_tri(out: &mut CrosshairVertices, a: [f32; 2], b: [f32; 2], c: [f32; 2]) {
    let i = out.count as usize;
    debug_assert!(i + 3 <= MAX_CROSSHAIR_VERTICES);
    out.vertices[i] = a;
    out.vertices[i + 1] = b;
    out.vertices[i + 2] = c;
    out.count += 3;
}

fn ndc(width: u32, height: u32, x: f32, y: f32) -> [f32; 2] {
    [x / width as f32 * 2.0 - 1.0, 1.0 - y / height as f32 * 2.0]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crosshair_has_expected_vertex_count() {
        let verts = crosshair_vertices(800, 600);
        assert_eq!(verts.count, MAX_CROSSHAIR_VERTICES as u32);
    }

    #[test]
    fn crosshair_vertices_are_clip_space() {
        let verts = crosshair_vertices(320, 240);
        for p in &verts.vertices[..verts.count as usize] {
            assert!((-1.0..=1.0).contains(&p[0]), "x {}", p[0]);
            assert!((-1.0..=1.0).contains(&p[1]), "y {}", p[1]);
        }
    }
}
