//! Ways into the graph.

use std::sync::Arc;
use std::time::Duration;

use async_graphql::{Context, ID, Object, Result};

use super::types::{
    Basket, LatLngInput, ListConnection, Now, Order, Pub, Quote, gql, page, service,
};
use crate::session::Session;

pub struct QueryRoot;

pub fn distance_m(from: (f64, f64), to: (f64, f64)) -> f64 {
    let (lat1, lon1) = (from.0.to_radians(), from.1.to_radians());
    let (lat2, lon2) = (to.0.to_radians(), to.1.to_radians());
    let a = ((lat2 - lat1) / 2.0).sin().powi(2)
        + lat1.cos() * lat2.cos() * ((lon2 - lon1) / 2.0).sin().powi(2);
    2.0 * 6_371_000.0 * a.sqrt().asin()
}

/// Reads. Each field says what it costs in requests to Wetherspoon's.
#[Object]
impl QueryRoot {
    /// The time in London. Free: no request.
    async fn now(&self) -> Now {
        Now
    }

    /// Pubs by name, town or postcode, or nearest first to a point, or both. Closed-for-good pubs are left out.
    /// The pub list is one request, read once and kept; everything here is free after that. Cursors are pub refs.
    async fn pubs(
        &self,
        ctx: &Context<'_>,
        #[graphql(desc = "Part of a pub's name, town or postcode; every word must match.")]
        query: Option<String>,
        #[graphql(desc = "Lists the nearest first, with `distanceMetres`.")] near: Option<
            LatLngInput,
        >,
        #[graphql(desc = "Page size, 5 when not given, at most 50.")] first: Option<i32>,
        after: Option<String>,
    ) -> Result<ListConnection<Pub>> {
        let venues = service(ctx).venues().await.map_err(gql)?;
        let point = near.map(|n| (n.lat, n.lng));
        let open = venues.iter().filter(|v| !v.is_closed);
        // Misheard names ("Broccoli Barge") and towns still find their pub, best match first; with `near` too,
        // equally good matches go nearest first.
        let (ranked, close): (Vec<(f32, &spoons_api_client::VenueSummary)>, bool) = match query
            .as_deref()
            .and_then(|q| crate::search::pubs(q, open.clone()))
        {
            Some(r) => (r.items, r.close),
            None => (open.map(|v| (0.0, v)).collect(), false),
        };
        let mut found: Vec<(f32, Pub)> = ranked
            .into_iter()
            .map(|(score, v)| {
                (
                    score,
                    Pub {
                        distance: point
                            .zip(v.address.location.as_ref())
                            .map(|(p, at)| distance_m(p, (at.latitude, at.longitude))),
                        venue: v.clone(),
                    },
                )
            })
            .collect();
        found.sort_by(|(sa, a), (sb, b)| {
            ((sb * 10.0).round()).total_cmp(&(sa * 10.0).round()).then(
                a.distance
                    .unwrap_or(f64::MAX)
                    .total_cmp(&b.distance.unwrap_or(f64::MAX)),
            )
        });
        let items = found
            .into_iter()
            .map(|(_, p)| (p.venue.venue_ref.to_string(), p))
            .collect();
        page(items, after, first, 5, 50, close)
    }

    /// One pub by its ref, or without one, the pub this conversation is ordering at ("the pub I'm in"). Free once
    /// the pub list is in.
    #[graphql(name = "pub")]
    async fn pub_(
        &self,
        ctx: &Context<'_>,
        #[graphql(
            name = "ref",
            desc = "From `pubs`; the conversation's pub when not given."
        )]
        venue_ref: Option<i64>,
    ) -> Result<Option<Pub>> {
        let venue_ref = match venue_ref {
            Some(named) => named,
            None => match ctx.data_unchecked::<Arc<Session>>().basket().pub_ref {
                Some(here) => here.venue_ref as i64,
                None => {
                    return Err(gql(
                        "no pub in this conversation yet: find it with `pubs` and pass its `ref`",
                    ));
                }
            },
        };
        let venues = service(ctx).venues().await.map_err(gql)?;
        Ok(venues
            .iter()
            .find(|v| v.venue_ref as i64 == venue_ref)
            .map(|v| Pub {
                venue: v.clone(),
                distance: None,
            }))
    }

    /// This conversation's basket. Free.
    async fn basket(
        &self,
        ctx: &Context<'_>,
        #[graphql(
            desc = "Wait up to this many seconds (at most 25) for the basket to change from this `version` before \
                    answering. For a view keeping itself current; the model has no reason to wait."
        )]
        changed_from: Option<u64>,
        #[graphql(default = 20)] wait_secs: u64,
    ) -> Basket {
        let session = ctx.data_unchecked::<Arc<Session>>();
        if let Some(from) = changed_from {
            let mut watch = session.watch();
            let wait = Duration::from_secs(wait_secs.min(25));
            let _ = tokio::time::timeout(wait, watch.wait_for(|v| *v != from)).await;
        }
        Basket {
            basket: session.basket(),
        }
    }

    /// A quote by id. Free: quotes are kept by this server.
    async fn quote(&self, ctx: &Context<'_>, id: ID) -> Result<Option<Quote>> {
        match service(ctx).load(&id) {
            Ok(order) => Ok(Some(Quote { order })),
            Err(error) if error.starts_with("no such quote") => Ok(None),
            Err(error) => Err(gql(error)),
        }
    }

    /// The quotes made in this conversation, newest first: what "same again" repeats. Free.
    async fn quotes(&self, ctx: &Context<'_>) -> Vec<Quote> {
        let session = ctx.data_unchecked::<Arc<Session>>();
        session
            .quotes()
            .iter()
            .rev()
            .filter_map(|id| service(ctx).load(id).ok())
            .map(|order| Quote { order })
            .collect()
    }

    /// A quote's order and where its payment has got to. One request to the payment API once paying has started,
    /// and one every five seconds while waiting.
    async fn order(
        &self,
        ctx: &Context<'_>,
        #[graphql(desc = "The quote id.")] quote_id: ID,
        #[graphql(
            desc = "Wait up to this many seconds (at most 55) for the payment to finish or need the user before \
                    answering."
        )]
        wait_secs: Option<u64>,
    ) -> Result<Order> {
        let service = service(ctx);
        let order = service.load(&quote_id).map_err(gql)?;
        let live = service
            .wait(&order, Duration::from_secs(wait_secs.unwrap_or(0)))
            .await
            .map_err(gql)?;
        Ok(Order {
            quote: Quote { order },
            live,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::distance_m;

    #[test]
    fn measures_distance_between_points() {
        let watch_house = (51.459945, -0.01164);
        let lewisham_station = (51.4657, -0.0142);
        let metres = distance_m(watch_house, lewisham_station);
        assert!((600.0..700.0).contains(&metres), "{metres}");
    }
}
