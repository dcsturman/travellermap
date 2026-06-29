//! Web-free map rendering for the Traveller map.
//!
//! The ported `RenderContext`/`Stylesheet` scene logic, expressed entirely
//! against [`canvas::Canvas`] so it compiles both to wasm (the Leptos frontend's
//! `Canvas2d`) and natively (the backend's SVG canvas). Holds no I/O and no
//! `web-sys`; it consumes `tmap_core` DTOs and emits draw calls.
//!
//! Entry point: [`render::draw_scene`]. Backends provide a [`canvas::Canvas`]
//! impl and the frame dimensions.

pub mod canvas;
pub mod glyph;
pub mod render;
