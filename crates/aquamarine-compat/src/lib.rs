//! Cargo compatibility layer for teloxide's rustdoc sequence diagram.

pub use aquamarine_compat_macro::aquamarine;

#[doc(hidden)]
pub use merman_rustdoc::{__render_doc, merman as __merman};
