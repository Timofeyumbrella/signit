//! pdf-draw: open a PDF, draw on its pages with the trackpad, and save a new PDF with the ink
//! baked into the page content as vector paths.
//!
//! The modules live in a library (not only in the binary) so the tests in `tests/` can use them.

pub mod app;
pub mod geometry;
pub mod history;
pub mod ink;
pub mod pdf;
pub mod save;
pub mod view;
