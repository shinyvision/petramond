//! Undo/redo: a snapshot ring of Document clones. Continuous gestures (canvas
//! drags, inspector drag-values) coalesce into one entry: the gesture pushes
//! the pre-gesture snapshot once and further edits ride on it until release.

use llama_ui::Document;

const DEPTH: usize = 256;

#[derive(Default)]
pub struct History {
    undo: Vec<Document>,
    redo: Vec<Document>,
    gesture: bool,
}

impl History {
    pub fn new() -> History {
        History::default()
    }

    /// Record a discrete edit: call with the document state *before* mutating.
    /// Inside a gesture this is a no-op (the gesture already snapshotted).
    pub fn record(&mut self, before: &Document) {
        if !self.gesture {
            self.push(before.clone());
        }
    }

    /// Start a continuous gesture (drag). Snapshots once; edits until
    /// `end_gesture` coalesce into this single entry.
    pub fn begin_gesture(&mut self, before: &Document) {
        if !self.gesture {
            self.push(before.clone());
            self.gesture = true;
        }
    }

    /// Finish the gesture. If it made no net change, the snapshot is dropped.
    pub fn end_gesture(&mut self, current: &Document) {
        if self.gesture {
            self.gesture = false;
            if self.undo.last() == Some(current) {
                self.undo.pop();
            }
        }
    }

    pub fn undo(&mut self, current: &mut Document) -> bool {
        let Some(prev) = self.undo.pop() else { return false };
        self.redo.push(std::mem::replace(current, prev));
        true
    }

    pub fn redo(&mut self, current: &mut Document) -> bool {
        let Some(next) = self.redo.pop() else { return false };
        self.undo.push(std::mem::replace(current, next));
        true
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    pub fn clear(&mut self) {
        self.undo.clear();
        self.redo.clear();
        self.gesture = false;
    }

    fn push(&mut self, d: Document) {
        self.redo.clear();
        self.undo.push(d);
        if self.undo.len() > DEPTH {
            self.undo.remove(0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use llama_ui::{DocClass, Node, NodeKind, FORMAT_VERSION};

    fn doc(label: &str) -> Document {
        Document {
            format: FORMAT_VERSION,
            kind: "llama:test".into(),
            class: DocClass::Screen,
            root: Node::leaf(NodeKind::Label { text: Some(label.into()), wrap: false, scale: 1 }),
        }
    }

    fn set(d: &mut Document, label: &str) {
        d.root.kind = NodeKind::Label { text: Some(label.into()), wrap: false, scale: 1 };
    }

    #[test]
    fn undo_redo_round_trips() {
        let mut h = History::new();
        let mut d = doc("a");
        h.record(&d);
        set(&mut d, "b");
        h.record(&d);
        set(&mut d, "c");

        assert!(h.undo(&mut d));
        assert_eq!(d, doc("b"));
        assert!(h.undo(&mut d));
        assert_eq!(d, doc("a"));
        assert!(!h.undo(&mut d), "stack exhausted");
        assert!(h.redo(&mut d));
        assert_eq!(d, doc("b"));
        assert!(h.redo(&mut d));
        assert_eq!(d, doc("c"));
        assert!(!h.redo(&mut d));
    }

    #[test]
    fn new_edit_clears_redo() {
        let mut h = History::new();
        let mut d = doc("a");
        h.record(&d);
        set(&mut d, "b");
        h.undo(&mut d);
        h.record(&d);
        set(&mut d, "z");
        assert!(!h.can_redo(), "diverging edit invalidates redo");
        h.undo(&mut d);
        assert_eq!(d, doc("a"));
    }

    #[test]
    fn gestures_coalesce_into_one_entry() {
        let mut h = History::new();
        let mut d = doc("start");
        h.begin_gesture(&d);
        for i in 0..5 {
            set(&mut d, &format!("drag{i}"));
            h.record(&d); // mid-gesture records are no-ops
        }
        h.end_gesture(&d);
        assert!(h.undo(&mut d), "one entry for the whole drag");
        assert_eq!(d, doc("start"));
        assert!(!h.can_undo());
    }

    #[test]
    fn no_op_gesture_leaves_no_entry() {
        let mut h = History::new();
        let mut d = doc("same");
        h.begin_gesture(&d);
        h.end_gesture(&d);
        assert!(!h.can_undo());
        assert!(!h.undo(&mut d));
    }

    #[test]
    fn ring_caps_at_depth() {
        let mut h = History::new();
        let mut d = doc("0");
        for i in 1..400 {
            h.record(&d);
            set(&mut d, &i.to_string());
        }
        let mut undone = 0;
        while h.undo(&mut d) {
            undone += 1;
        }
        assert_eq!(undone, 256);
    }
}
