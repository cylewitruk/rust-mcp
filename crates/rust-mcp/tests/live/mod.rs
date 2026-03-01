//! Opt-in live integration tests against real upstreams and local registry.
//!
//! These tests hit real network endpoints and require local filesystem
//! fixtures (rustdoc JSON, cargo registry). They are gated behind the
//! `live-tests` Cargo feature and executed via `just test-live`.

#[path = "../common.rs"]
mod common;

mod crates_io;
mod discovery;
mod index;
