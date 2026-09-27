use crate::mathh::IVec3;
use crate::world::placement::authored::Turn;

use super::{Bounds, Cell, Template};

#[derive(Clone, Copy)]
pub struct Placement<'a> {
    template: &'a Template,
    origin: IVec3,
    turn: Turn,
    bounds: Bounds,
}

fn checked_add(a: IVec3, b: IVec3) -> Result<IVec3, String> {
    Ok(IVec3::new(
        a.x.checked_add(b.x).ok_or("placement x overflow")?,
        a.y.checked_add(b.y).ok_or("placement y overflow")?,
        a.z.checked_add(b.z).ok_or("placement z overflow")?,
    ))
}

impl<'a> Placement<'a> {
    pub(super) fn new(template: &'a Template, origin: IVec3, turn: Turn) -> Result<Self, String> {
        let local = template.bounds(turn);
        let bounds = Bounds {
            min: checked_add(origin, local.min)?,
            max: checked_add(origin, local.max)?,
        };
        Ok(Self {
            template,
            origin,
            turn,
            bounds,
        })
    }

    pub fn bounds(self) -> Bounds {
        self.bounds
    }

    pub fn origin(self) -> IVec3 {
        self.origin
    }

    pub fn turn(self) -> Turn {
        self.turn
    }

    pub fn visit(self, clip: Bounds, mut emit: impl FnMut(IVec3, &Cell)) {
        if !self.bounds.intersects(clip) {
            return;
        }
        for cell in &self.template.variants[self.turn.index()].cells {
            let pos = self.origin + cell.pos;
            if clip.contains(pos) {
                emit(pos, cell);
            }
        }
    }

    pub fn attach<'b>(
        self,
        socket: &str,
        child: &'b Template,
        plug: &str,
    ) -> Result<Placement<'b>, String> {
        let socket = self
            .template
            .connectors(self.turn)
            .iter()
            .find(|c| c.name == socket)
            .ok_or_else(|| format!("unknown connector '{socket}'"))?;
        for turn in Turn::ALL {
            let Some(plug) = child.connectors(turn).iter().find(|c| c.name == plug) else {
                continue;
            };
            if plug.kind != socket.kind || plug.facing.dir() != -socket.facing.dir() {
                continue;
            }
            let target = checked_add(checked_add(self.origin, socket.pos)?, socket.facing.dir())?;
            return child.place(checked_add(target, -plug.pos)?, turn);
        }
        Err(format!(
            "connector '{plug}' does not mate with '{}'",
            socket.name
        ))
    }
}
