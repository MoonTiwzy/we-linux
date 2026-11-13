//! The window of WE Linux: everything `app.slint` declares, as Rust types.
//!
//! The generated code is large and slow to compile, and it changes only when the
//! `.slint` file does. Kept in this crate, it is compiled once; a change to the
//! program's logic no longer rebuilds the interface.

slint::include_modules!();
