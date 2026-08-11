//! Shapes shared by `telenotes-bot` and `telenotes-tui`: the `items` row
//! shape and the pure text helpers used to derive tags, filenames, and vault
//! paths from it.
//!
//! Deliberately data-and-pure-functions only, not a networking layer -- the
//! bot never calls Supabase's REST API and the TUI never touches storage
//! credentials, so there is no shared client to put here, only shared
//! shapes.

pub mod models;
pub mod text;

pub use models::{Backend, Item, Stats};
