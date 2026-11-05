//! A client for the API behind Wetherspoon's order-and-pay app.
//!
//! The API is unofficial and undocumented. It sits behind a proxy at `ca.jdw-apps.net`, and the bearer token the app
//! uses is built into the app's public JavaScript bundle, so [`Client::discover`] reads it from there rather than
//! anyone keeping a copy of it. Orders go through a separate payment API, reached with [`Client::payments`], and are paid on an Axepta page,
//! which [`enter`] can fill in with a card.

mod basket;
mod card;
mod client;
mod de;
mod models;
mod payments;

pub use basket::{BasketLine, LineError};
pub use card::{Card, CardError, Entered, PAYMENT_PAGE, enter};
pub use client::{Client, Error, paths};
pub use models::*;
pub use payments::{CardOrder, PaymentStatus, Payments, Prepared};
