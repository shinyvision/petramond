use super::material::{Axis, Dir};

/// A rotated local grid over a `w` × `d` footprint whose +local-z points `fwd`: builders lay a
/// hut or tower out once, in local coordinates, and it lands correctly facing any direction.
#[derive(Clone, Copy, Debug)]
pub struct Frame {
    origin: [i32; 2],
    fwd: Dir,
    shift: [i32; 2],
}

impl Frame {
    /// A frame whose footprint's minimum world corner is `min`.
    pub fn new(min: [i32; 2], w: i32, d: i32, fwd: Dir) -> Frame {
        let probe = Frame {
            origin: [0, 0],
            fwd,
            shift: [0, 0],
        };
        let corners =
            [(0, 0), (w - 1, 0), (0, d - 1), (w - 1, d - 1)].map(|(x, z)| probe.raw(x, z));
        let mx = corners.iter().map(|c| c[0]).min().unwrap();
        let mz = corners.iter().map(|c| c[1]).min().unwrap();
        Frame {
            origin: min,
            fwd,
            shift: [-mx, -mz],
        }
    }

    /// World footprint size of a `w` × `d` local footprint facing `fwd`.
    pub fn world_size(w: i32, d: i32, fwd: Dir) -> [i32; 2] {
        match fwd.axis() {
            Axis::Z => [w, d],
            _ => [d, w],
        }
    }

    fn raw(&self, lx: i32, lz: i32) -> [i32; 2] {
        let f = self.fwd.offset();
        let r = self.right().offset();
        [lx * r[0] + lz * f[0], lx * r[1] + lz * f[1]]
    }

    pub fn at(&self, lx: i32, lz: i32) -> [i32; 2] {
        let [x, z] = self.raw(lx, lz);
        [
            self.origin[0] + self.shift[0] + x,
            self.origin[1] + self.shift[1] + z,
        ]
    }

    pub fn fwd(&self) -> Dir {
        self.fwd
    }

    pub fn back(&self) -> Dir {
        self.fwd.opposite()
    }

    pub fn right(&self) -> Dir {
        self.fwd.turn(3)
    }

    pub fn left(&self) -> Dir {
        self.fwd.turn(1)
    }

    /// World axis of the local x direction, for logs laid along a wall.
    pub fn x_axis(&self) -> Axis {
        self.right().axis()
    }

    pub fn z_axis(&self) -> Axis {
        self.fwd.axis()
    }
}
