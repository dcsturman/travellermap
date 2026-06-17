//! Shared Traveller Map domain model and wire types.
//!
//! **Pure logic only** — no I/O, no async, no framework. This crate compiles
//! to both native (the `tmap-backend` server) and `wasm32-unknown-unknown`
//! (the `tmap-frontend` Leptos client), so it is the single source of truth
//! for the data model and the streaming wire format shared across the wire.
//!
//! Keep it that way: if something here wants to touch the filesystem, the
//! network, or `tokio`, it belongs in `tmap-backend` or `tmap-frontend`
//! instead. See CLAUDE.md "Mission" for why.
//!
//! Porting note: this is where the reference implementation's
//! `server/Astrometrics.cs`, `server/SecondSurvey.cs`, and `server/World.cs`
//! logic should land as it is reimplemented in Rust.

pub mod astrometrics;
pub mod dto;
pub mod metadata;
pub mod parse;
pub mod route;
pub mod searchlang;
pub mod sector_writer;
pub mod world_util;
