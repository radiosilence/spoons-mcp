//! Per-request loading of what hangs off a pub.
//!
//! The API has no batch routes, so a loader can't fold twenty pubs into one request. What it does is make each
//! pub's detail, wait times, ale list and tables load at most once per query however many fields ask for them,
//! with the pubs' requests in flight together rather than one after another.

use std::collections::HashMap;
use std::sync::Arc;

use async_graphql::dataloader::Loader;
use futures_util::future::join_all;
use spoons_api_client::{Ale, Table, Venue, WaitTimes};

use crate::service::Service;

pub struct PubLoader {
    service: Arc<Service>,
}

impl PubLoader {
    pub fn new(service: Arc<Service>) -> Self {
        Self { service }
    }
}

#[derive(Clone, Copy, Hash, PartialEq, Eq)]
pub struct VenueKey(pub u64);
#[derive(Clone, Copy, Hash, PartialEq, Eq)]
pub struct WaitKey(pub u64);
#[derive(Clone, Copy, Hash, PartialEq, Eq)]
pub struct AlesKey(pub u64);
#[derive(Clone, Copy, Hash, PartialEq, Eq)]
pub struct TablesKey(pub u64);

macro_rules! per_pub {
    ($key:ident, $value:ty, $fetch:ident) => {
        impl Loader<$key> for PubLoader {
            type Value = $value;
            type Error = Arc<String>;

            async fn load(&self, keys: &[$key]) -> Result<HashMap<$key, $value>, Self::Error> {
                let loaded = join_all(keys.iter().map(|key| async move {
                    let pub_ref = self.service.pub_ref(key.0).await?;
                    self.service
                        .$fetch(&pub_ref)
                        .await
                        .map(|value| (*key, value))
                }))
                .await;
                loaded
                    .into_iter()
                    .collect::<Result<HashMap<_, _>, String>>()
                    .map_err(Arc::new)
            }
        }
    };
}

per_pub!(VenueKey, Venue, venue);
per_pub!(WaitKey, WaitTimes, wait_times);
per_pub!(AlesKey, Vec<Ale>, ales);
per_pub!(TablesKey, Vec<Table>, tables);
