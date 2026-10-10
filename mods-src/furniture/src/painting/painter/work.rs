//! The flag on the table and the design being painted on it. The stack in the table's slot is
//! the truth; the design here is an edit of it, sent as a whole when the painter asks, so the
//! server never hears a stroke.

use mod_sdk::ItemStackData;

use crate::painting::design::{Design, Rgb, PAINT_KEY, WHITE};
use crate::painting::paintable::{Plan, Plans};

/// Strokes that can be taken back.
const UNDO_DEPTH: usize = 64;

/// Frames a sent design is waited for before the stack in the slot is believed instead.
const PATIENCE: u16 = 240;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Tool {
    Pencil,
    Fill,
}

pub(super) struct Work {
    /// The stack the design was last matched against.
    source: ItemStackData,
    plan: Plan,
    /// The design that stack shows.
    saved: Design,
    design: Design,
    undo: Vec<Design>,
    /// Designs sent and not yet seen on the stack, oldest first, and the frames left to wait.
    sent: Vec<Vec<u8>>,
    patience: u16,
    /// Where the stroke under way last was.
    stroke: Option<[i32; 2]>,
}

fn same_flag(a: &ItemStackData, b: &ItemStackData) -> bool {
    a.item == b.item && a.data == b.data
}

impl Work {
    fn open(
        stack: ItemStackData,
        plan: Plan,
        tile_pixels: impl FnOnce(&str) -> Option<Vec<u8>>,
    ) -> Work {
        let saved = plan.starting_design(&stack, tile_pixels);
        Work {
            source: stack,
            plan,
            design: saved.clone(),
            saved,
            undo: Vec::new(),
            sent: Vec::new(),
            patience: 0,
            stroke: None,
        }
    }

    /// Brings the work in line with the stack now in the table's slot; `true` if the design on
    /// show changed. Seeing a sent design arrive keeps whatever was painted since; any other new
    /// stack is another flag, and starts over from it.
    pub fn follow(
        work: &mut Option<Work>,
        plans: &Plans,
        stack: Option<ItemStackData>,
        tile_pixels: impl FnOnce(&str) -> Option<Vec<u8>>,
    ) -> bool {
        let paintable = stack.and_then(|stack| {
            let plan = plans.of(&stack.item)?.clone();
            Some((stack, plan))
        });
        let Some((stack, plan)) = paintable else {
            return work.take().is_some();
        };
        if let Some(current) = work {
            if same_flag(&current.source, &stack) {
                current.wait();
                return false;
            }
            if current.arrived(&stack, &plan) {
                return false;
            }
        }
        *work = Some(Work::open(stack, plan, tile_pixels));
        true
    }

    fn wait(&mut self) {
        self.patience = self.patience.saturating_sub(1);
        if self.patience == 0 {
            self.sent.clear();
        }
    }

    /// Whether `stack` is the flag wearing one of the sent designs; if so that design is now the
    /// saved one.
    fn arrived(&mut self, stack: &ItemStackData, plan: &Plan) -> bool {
        let paint = stack.data.iter().find(|(key, _)| key == PAINT_KEY);
        let Some((_, paint)) = paint.filter(|_| stack.item == self.plan.result) else {
            return false;
        };
        let Some(newest) = self.sent.iter().rposition(|sent| sent == paint) else {
            return false;
        };
        let Some(saved) = Design::from_paint(plan.canvas, paint) else {
            return false;
        };
        self.sent.drain(..=newest);
        self.saved = saved;
        self.source = stack.clone();
        self.plan = plan.clone();
        true
    }

    /// The design to send the server, if it differs from the last one it was sent or shows.
    /// `send` hands it over and says whether anyone heard.
    pub fn save(&mut self, send: impl FnOnce(Vec<u8>) -> bool) {
        let paint = self.design.to_paint();
        let promised = match self.sent.last() {
            Some(sent) => *sent == paint,
            None => self.design == self.saved,
        };
        if !promised && send(paint.clone()) {
            self.sent.push(paint);
            self.patience = PATIENCE;
        }
    }

    pub fn design(&self) -> &Design {
        &self.design
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    fn remember(&mut self) {
        if self.undo.len() == UNDO_DEPTH {
            self.undo.remove(0);
        }
        self.undo.push(self.design.clone());
    }

    pub fn press(&mut self, at: [i32; 2], tool: Tool, color: Rgb) {
        self.remember();
        match tool {
            Tool::Pencil => {
                self.design.set(at, color);
                self.stroke = Some(at);
            }
            Tool::Fill => self.design.fill(at, color),
        }
    }

    pub fn drag(&mut self, at: [i32; 2], color: Rgb) {
        if let Some(from) = self.stroke {
            self.design.line(from, at, color);
            self.stroke = Some(at);
        }
    }

    pub fn release(&mut self) {
        self.stroke = None;
    }

    pub fn undo(&mut self) {
        if let Some(design) = self.undo.pop() {
            self.design = design;
        }
    }

    pub fn clear(&mut self) {
        self.remember();
        self.design = Design::filled(self.plan.canvas, WHITE);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::painting::design::TINT_KEY;
    use crate::painting::paintable::tests::{plans, stack, EMBLEM, FLAG};

    const INK: Rgb = [20, 40, 200];

    fn no_tile(_: &str) -> Option<Vec<u8>> {
        None
    }

    fn follow(work: &mut Option<Work>, stack: Option<ItemStackData>) -> bool {
        Work::follow(work, &plans(), stack, no_tile)
    }

    /// Sends the work's design and returns what the server would then put in the slot.
    fn save(work: &mut Work) -> Option<ItemStackData> {
        let mut heard = None;
        work.save(|paint| {
            heard = Some(paint);
            true
        });
        let design = Design::from_paint(work.plan.canvas, &heard?)?;
        Some(work.plan.painted(&work.source, &design))
    }

    fn on_table(stack: ItemStackData) -> Option<Work> {
        let mut work = None;
        assert!(follow(&mut work, Some(stack)));
        work
    }

    #[test]
    fn nothing_is_sent_until_the_design_differs_from_the_flag() {
        let mut work = on_table(stack(FLAG, &[])).expect("a flag is paintable");
        assert!(save(&mut work).is_none());
        work.press([1, 1], Tool::Pencil, INK);
        work.undo();
        assert!(
            save(&mut work).is_none(),
            "painted and taken back is unchanged"
        );
        work.press([1, 1], Tool::Pencil, INK);
        assert!(save(&mut work).is_some());
        assert!(
            save(&mut work).is_none(),
            "the same design is not sent twice"
        );
    }

    #[test]
    fn strokes_painted_while_a_save_is_on_its_way_survive_its_arrival() {
        let mut work = on_table(stack(FLAG, &[]));
        let painter = work.as_mut().expect("a flag is paintable");
        painter.press([1, 1], Tool::Pencil, INK);
        let first = save(painter).expect("a changed design is sent");
        painter.press([5, 5], Tool::Pencil, INK);

        assert!(
            !follow(&mut work, Some(stack(FLAG, &[]))),
            "the old stack is still awaited"
        );
        assert!(
            !follow(&mut work, Some(first)),
            "the save arriving redraws nothing"
        );
        let painter = work.as_mut().expect("still the same flag");
        assert_eq!(painter.design().get([5, 5]), Some(INK));
        assert!(save(painter).is_some(), "the later stroke is still unsaved");
    }

    #[test]
    fn a_second_save_sent_before_the_first_lands_is_not_mistaken_for_another_flag() {
        let mut work = on_table(stack(FLAG, &[]));
        let painter = work.as_mut().expect("a flag is paintable");
        painter.press([1, 1], Tool::Pencil, INK);
        let first = save(painter).expect("sent");
        painter.press([5, 5], Tool::Pencil, INK);
        let second = save(painter).expect("sent");

        assert!(!follow(&mut work, Some(first)));
        assert!(!follow(&mut work, Some(second)));
        let painter = work.as_mut().expect("still the same flag");
        assert_eq!(painter.design().get([5, 5]), Some(INK));
        assert!(save(painter).is_none(), "everything painted is on the flag");
    }

    #[test]
    fn another_flag_in_the_slot_starts_over_from_that_flag() {
        let mut work = on_table(stack(FLAG, &[]));
        work.as_mut()
            .expect("a flag")
            .press([1, 1], Tool::Pencil, INK);
        let red = stack(FLAG, &[(TINT_KEY, vec![200, 0, 0])]);
        assert!(follow(&mut work, Some(red)));
        let painter = work.as_ref().expect("the dyed flag is paintable");
        assert_eq!(painter.design().get([1, 1]), Some([200, 0, 0]));
        assert!(!painter.can_undo());

        assert!(follow(&mut work, None), "an empty slot clears the canvas");
        assert!(work.is_none());
        assert!(!follow(&mut work, Some(stack("test:rock", &[]))));
    }

    #[test]
    fn a_save_nobody_answers_is_given_up_on_and_can_be_sent_again() {
        let mut work = on_table(stack(FLAG, &[]));
        let painter = work.as_mut().expect("a flag is paintable");
        painter.press([1, 1], Tool::Pencil, INK);
        assert!(save(painter).is_some());
        for _ in 0..PATIENCE {
            assert!(!follow(&mut work, Some(stack(FLAG, &[]))));
        }
        let painter = work.as_mut().expect("still the same flag");
        assert_eq!(
            painter.design().get([1, 1]),
            Some(INK),
            "the painting is kept"
        );
        assert!(save(painter).is_some());
    }

    #[test]
    fn an_emblem_flag_painted_becomes_a_plain_flag_under_the_same_brush() {
        let mut work = on_table(stack(EMBLEM, &[]));
        let painter = work.as_mut().expect("an emblem flag is paintable");
        assert!(
            save(painter).is_none(),
            "untouched, it stays the flag it was"
        );
        painter.press([1, 1], Tool::Pencil, INK);
        let painted = save(painter).expect("sent");
        assert_eq!(painted.item, FLAG);
        assert!(
            !follow(&mut work, Some(painted)),
            "the new item is the same work"
        );
        let painter = work.as_mut().expect("still painting");
        assert!(painter.can_undo());
        assert!(save(painter).is_none());
    }

    #[test]
    fn a_drag_draws_from_where_the_stroke_was_and_stops_when_released() {
        let mut work = on_table(stack(FLAG, &[])).expect("a flag is paintable");
        work.press([0, 0], Tool::Pencil, INK);
        work.drag([3, 0], INK);
        assert_eq!(work.design().get([2, 0]), Some(INK));
        work.release();
        work.drag([3, 5], INK);
        assert_eq!(work.design().get([3, 3]), Some(WHITE), "no stroke, no line");
        work.undo();
        assert_eq!(
            work.design().get([0, 0]),
            Some(WHITE),
            "one undo takes the stroke back"
        );
    }
}
