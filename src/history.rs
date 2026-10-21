//! The strokes of the open document, with undo and redo.

use crate::ink::Stroke;
use eframe::egui::Pos2;

/// The strokes drawn on the open document, with undo/redo and a "saved" mark.
pub struct Drawing {
    strokes: Vec<Stroke>,
    undo: Vec<Edit>,
    redo: Vec<Edit>,
    /// Length of `undo` when the drawing was last saved (0 for a freshly opened document), or
    /// `None` once undo/redo can no longer get back to that state.
    saved_at: Option<usize>,
    /// An eraser gesture is going on: what it erases joins the last undo step.
    erasing: bool,
}

/// One undoable change.
enum Edit {
    /// A stroke appended at the end.
    Add(Stroke),
    /// Strokes removed by one eraser gesture, each with the index it had when it was removed.
    Erase(Vec<(usize, Stroke)>),
    /// Every stroke, removed by "Clear all".
    Clear(Vec<Stroke>),
}

impl Default for Drawing {
    fn default() -> Self {
        Drawing {
            strokes: Vec::new(),
            undo: Vec::new(),
            redo: Vec::new(),
            saved_at: Some(0),
            erasing: false,
        }
    }
}

impl Drawing {
    /// All strokes, oldest first.
    pub fn strokes(&self) -> &[Stroke] {
        &self.strokes
    }

    /// Adds a finished stroke.
    pub fn add(&mut self, stroke: Stroke) {
        self.strokes.push(stroke.clone());
        self.push(Edit::Add(stroke));
    }

    /// Removes every stroke on `page` whose ink comes within `radius` (pt) of `p`.
    /// Everything erased until [`Drawing::end_erase`] is undone in one step.
    pub fn erase(&mut self, page: usize, p: Pos2, radius: f32) {
        let mut removed = Vec::new();
        // Back to front, so removing a stroke does not shift the ones still to check.
        for i in (0..self.strokes.len()).rev() {
            if self.strokes[i].page == page && self.strokes[i].hit(p, radius) {
                removed.push((i, self.strokes.remove(i)));
            }
        }
        if removed.is_empty() {
            return;
        }
        match self.undo.last_mut() {
            Some(Edit::Erase(earlier)) if self.erasing => earlier.extend(removed),
            _ => self.push(Edit::Erase(removed)),
        }
        self.erasing = true;
    }

    /// Ends the current eraser gesture.
    pub fn end_erase(&mut self) {
        self.erasing = false;
    }

    /// Removes all strokes (undoable).
    pub fn clear(&mut self) {
        if !self.strokes.is_empty() {
            let all = std::mem::take(&mut self.strokes);
            self.push(Edit::Clear(all));
        }
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    /// Reverts the last change, if any.
    pub fn undo(&mut self) {
        self.erasing = false;
        let Some(edit) = self.undo.pop() else {
            return;
        };
        match &edit {
            Edit::Add(_) => {
                self.strokes.pop();
            }
            // Re-insert in reverse order of removal, so each index means what it meant then.
            Edit::Erase(removed) => {
                for (i, stroke) in removed.iter().rev() {
                    self.strokes.insert(*i, stroke.clone());
                }
            }
            Edit::Clear(all) => self.strokes = all.clone(),
        }
        self.redo.push(edit);
    }

    /// Applies the last undone change again, if any.
    pub fn redo(&mut self) {
        self.erasing = false;
        let Some(edit) = self.redo.pop() else {
            return;
        };
        match &edit {
            Edit::Add(stroke) => self.strokes.push(stroke.clone()),
            Edit::Erase(removed) => {
                for (i, _) in removed {
                    self.strokes.remove(*i);
                }
            }
            Edit::Clear(_) => self.strokes.clear(),
        }
        self.undo.push(edit);
    }

    /// Whether the strokes differ from the last save (or, before any save, from no strokes).
    pub fn is_dirty(&self) -> bool {
        self.saved_at != Some(self.undo.len())
    }

    /// Remembers the current strokes as saved.
    pub fn mark_saved(&mut self) {
        self.saved_at = Some(self.undo.len());
        // Later erasing must not change the step that is now marked as saved.
        self.erasing = false;
    }

    fn push(&mut self, edit: Edit) {
        // The new edit replaces the redo steps; if the saved state was among them, it is gone.
        if self.saved_at.is_some_and(|n| n > self.undo.len()) {
            self.saved_at = None;
        }
        self.redo.clear();
        self.undo.push(edit);
        self.erasing = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use eframe::egui::{Color32, pos2};

    /// A horizontal stroke at height `y` on `page`.
    fn line(page: usize, y: f32) -> Stroke {
        Stroke {
            page,
            points: vec![pos2(0.0, y), pos2(100.0, y)],
            color: Color32::BLACK,
            width: 1.0,
        }
    }

    fn ys(d: &Drawing) -> Vec<f32> {
        d.strokes().iter().map(|s| s.points[0].y).collect()
    }

    #[test]
    fn undo_and_redo_adds() {
        let mut d = Drawing::default();
        assert!(!d.can_undo() && !d.can_redo());
        d.add(line(0, 10.0));
        d.add(line(0, 20.0));
        d.undo();
        assert_eq!(ys(&d), [10.0]);
        assert!(d.can_undo() && d.can_redo());
        d.redo();
        assert_eq!(ys(&d), [10.0, 20.0]);
        d.undo();
        d.undo();
        assert_eq!(ys(&d), [] as [f32; 0]);
        d.undo(); // nothing left: no-op
        d.redo();
        assert_eq!(ys(&d), [10.0]);
        // A new stroke drops what could have been redone.
        d.add(line(0, 30.0));
        assert!(!d.can_redo());
        assert_eq!(ys(&d), [10.0, 30.0]);
    }

    #[test]
    fn one_eraser_gesture_is_one_undo_step() {
        let mut d = Drawing::default();
        for y in [10.0, 20.0, 30.0, 40.0] {
            d.add(line(0, y));
        }
        d.add(line(1, 20.0)); // other page: never erased below
        d.erase(0, pos2(50.0, 21.0), 2.0);
        d.erase(0, pos2(50.0, 25.0), 1.0); // touches nothing
        d.erase(0, pos2(50.0, 39.5), 2.0);
        d.end_erase();
        assert_eq!(ys(&d), [10.0, 30.0, 20.0]);
        d.undo();
        assert_eq!(ys(&d), [10.0, 20.0, 30.0, 40.0, 20.0]);
        d.redo();
        assert_eq!(ys(&d), [10.0, 30.0, 20.0]);
        d.undo();
        // A new gesture is a new step.
        d.erase(0, pos2(50.0, 10.0), 2.0);
        d.end_erase();
        d.erase(0, pos2(50.0, 30.0), 2.0);
        d.end_erase();
        assert_eq!(ys(&d), [20.0, 40.0, 20.0]);
        d.undo();
        assert_eq!(ys(&d), [20.0, 30.0, 40.0, 20.0]);
    }

    #[test]
    fn one_eraser_touch_removes_every_stroke_it_hits() {
        let mut d = Drawing::default();
        for y in [10.0, 12.0, 14.0, 50.0] {
            d.add(line(0, y));
        }
        d.erase(0, pos2(50.0, 12.0), 2.5);
        d.end_erase();
        assert_eq!(ys(&d), [50.0]);
        d.undo();
        assert_eq!(ys(&d), [10.0, 12.0, 14.0, 50.0]);
    }

    #[test]
    fn clear_is_undoable() {
        let mut d = Drawing::default();
        d.clear(); // nothing to clear: no undo step
        assert!(!d.can_undo());
        d.add(line(0, 10.0));
        d.add(line(2, 20.0));
        d.clear();
        assert!(d.strokes().is_empty());
        d.undo();
        assert_eq!(ys(&d), [10.0, 20.0]);
        d.redo();
        assert!(d.strokes().is_empty());
    }

    #[test]
    fn dirty_follows_the_saved_state() {
        let mut d = Drawing::default();
        assert!(!d.is_dirty());
        d.add(line(0, 10.0));
        assert!(d.is_dirty());
        d.undo();
        assert!(!d.is_dirty(), "back to the opened state");
        d.redo();
        d.mark_saved();
        assert!(!d.is_dirty());
        d.add(line(0, 20.0));
        assert!(d.is_dirty());
        d.undo();
        assert!(!d.is_dirty(), "back to the saved state");
        d.undo();
        assert!(d.is_dirty());
        d.redo();
        assert!(!d.is_dirty());
        // Undo past the save, then edit: the saved state can no longer be reached.
        d.undo();
        d.add(line(0, 30.0));
        assert!(d.is_dirty());
        d.undo();
        assert!(d.is_dirty());
    }
}
