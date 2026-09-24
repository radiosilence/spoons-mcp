//! A client for the API behind Wetherspoon's order-and-pay app.
//!
//! The API is unofficial and undocumented. It sits behind a proxy at `ca.jdw-apps.net`, and the bearer token the app
//! uses is built into the app's public JavaScript bundle, so [`Client::discover`] reads it from there rather than
//! anyone keeping a copy of it. Only the read routes are covered; the basket, order and payment routes are left alone.

mod client;
mod de;
mod models;

pub use client::{Client, Error, paths};
pub use models::*;
