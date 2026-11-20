//! The GraphQL API over Wetherspoon's order-and-pay: pubs, their menus, a basket, quotes and payments.
//!
//! Hand-written over `spoons-api-client`. The API's records don't join up by themselves (a menu is per sales area,
//! a sales area is per table, a guest ale's colour is in a separate ale list), and the joining is what makes it
//! usable, so it lives here. Reads below a pub are deduplicated per request by [`loaders::PubLoader`], and menus are
//! cached per sales area per London day by the service, so a nested selection never repeats a fetch.

pub mod loaders;
pub mod mutation;
pub mod query;
#[cfg(test)]
mod tests;
pub mod types;

use std::sync::Arc;

use async_graphql::dataloader::DataLoader;
use async_graphql::{EmptySubscription, Request, Schema};
use spoons_api_client::Card;

use crate::service::Service;
use crate::session::Session;

pub type SpoonsSchema = Schema<query::QueryRoot, mutation::MutationRoot, EmptySubscription>;

/// Deep enough for `pubs { nodes { menu { sections { products { portions { price { formatted } } } } } } }`,
/// short of anything that walks for its own sake.
const MAX_DEPTH: usize = 12;

pub fn schema() -> SpoonsSchema {
    Schema::build(query::QueryRoot, mutation::MutationRoot, EmptySubscription)
        .limit_depth(MAX_DEPTH)
        .finish()
}

/// The schema as SDL, which `spoons_schema` returns.
pub fn sdl() -> String {
    schema().sdl()
}

/// Who is asking, from the request: where the receipt goes and the saved card, if any.
#[derive(Clone)]
pub struct Caller {
    pub email: Option<String>,
    /// A card configured in part is an error rather than quietly falling back to the payment page.
    pub card: Result<Option<Card>, String>,
}

impl Caller {
    /// Whose confirmation tokens these are: the receipt address, which the gateway sets per user.
    pub fn tenant(&self) -> &str {
        self.email.as_deref().unwrap_or("")
    }
}

/// Attaches what resolvers need: the shared service, this session's basket, the caller and a fresh loader.
pub fn request(
    query: &str,
    service: Arc<Service>,
    session: Arc<Session>,
    caller: Caller,
) -> Request {
    Request::new(query)
        .data(DataLoader::new(
            loaders::PubLoader::new(service.clone()),
            tokio::spawn,
        ))
        .data(service)
        .data(session)
        .data(caller)
}
