//! A client for the API behind Wetherspoon's order-and-pay app.
//!
//! The API is unofficial and undocumented. It sits behind a proxy at `ca.jdw-apps.net`, and the bearer token the app
//! uses is built into the app's public JavaScript bundle, so [`Client::discover`] reads it from there rather than
//! anyone keeping a copy of it. Orders go through a separate payment API, reached with [`Client::payments`].

mod basket;
mod client;
mod de;
mod models;
mod payments;

pub use basket::{BasketLine, LineError};
pub use client::{Client, Error, paths};
pub use models::*;
pub use payments::{CardOrder, PaymentStatus, Payments, Prepared};
