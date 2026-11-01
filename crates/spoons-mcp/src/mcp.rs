//! An MCP server over stdio for finding a pub, reading its menu and ordering to a table.
//!
//! Ordering is two steps: `quote` prices a basket at the till without starting anything, and `pay` prepares the card
//! payment and opens the payment page, where the card is entered and the bank approves it. Nothing here retries a
//! request that could place an order, and nothing cancels an order on its own: cancelling while the bank is
//! authorising a card can leave a charge with no order.

use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use rmcp::handler::server::{router::tool::ToolRouter, wrapper::Parameters};
use rmcp::{ServerHandler, ServiceExt, tool, tool_handler, tool_router};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};
use spoons_api_client::{
    BasketLine, CardOrder, Client, Error, Item, PaymentStatus, Payments, Product, VenueSummary,
};
use tokio::sync::OnceCell;
use tokio::time::{Instant, sleep};

use crate::orders::{Line, Order, Payment, Store, now};

const MAX_LINES: usize = 10;
const MAX_QUANTITY: u32 = 10;
const MAX_WAIT: u64 = 55;
const POLL: Duration = Duration::from_secs(5);
const MAX_MENU_RESULTS: usize = 50;

#[derive(Clone)]
pub struct Spoons {
    client: Client,
    payments: Payments,
    store: Arc<Store>,
    venues: Arc<OnceCell<Vec<VenueSummary>>>,
    email: Option<String>,
    tool_router: ToolRouter<Self>,
}

pub async fn serve() -> Result<()> {
    let store = Store::open()?;
    if let Err(error) = store.prune() {
        tracing::warn!(%error, "couldn't prune old quotes");
    }
    let client = Client::discover()
        .await?
        .with_interval(Duration::from_millis(100));
    let payments = client.payments(&store.device_id()?)?;
    let server = Spoons {
        client,
        payments,
        store: Arc::new(store),
        venues: Arc::new(OnceCell::new()),
        email: std::env::var("SPOONS_EMAIL")
            .ok()
            .filter(|email| !email.is_empty()),
        tool_router: Spoons::tool_router(),
    };
    server
        .serve(rmcp::transport::stdio())
        .await?
        .waiting()
        .await?;
    Ok(())
}

#[derive(Deserialize, JsonSchema)]
pub struct FindPubs {
    /// Part of a pub's name, town or postcode.
    query: Option<String>,
    /// With longitude, lists the nearest pubs first.
    latitude: Option<f64>,
    longitude: Option<f64>,
    /// How many to list; 5 when not given.
    limit: Option<usize>,
}

#[derive(Deserialize, JsonSchema)]
pub struct MenuQuery {
    venue_ref: u64,
    /// The table being ordered to. Parts of a pub such as a garden can have their own menu and prices.
    table: Option<u32>,
    /// Part of a product's name, such as `guinness`; everything when not given.
    search: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
pub struct QuoteRequest {
    venue_ref: u64,
    table: u32,
    items: Vec<QuoteItem>,
}

#[derive(Deserialize, JsonSchema)]
pub struct QuoteItem {
    product_id: u64,
    /// A size as the menu labels it, such as `Pint` or `Half pint`; the default size when not given.
    portion: Option<String>,
    /// From 1 to 10.
    quantity: u32,
}

#[derive(Deserialize, JsonSchema)]
pub struct PayRequest {
    quote_id: String,
    /// The user has said they want this order although a matching one went to the same table recently.
    #[serde(default)]
    confirm_duplicate: bool,
    /// Prepare a payment again after an earlier attempt got no clear answer. Only when the user has checked that no
    /// order went through (no receipt email, no charge).
    #[serde(default)]
    force: bool,
    /// Open the payment page in the browser again.
    #[serde(default)]
    reopen: bool,
}

#[derive(Deserialize, JsonSchema)]
pub struct StatusRequest {
    quote_id: String,
    /// Wait up to this many seconds (at most 55) for the payment to finish before answering.
    wait_secs: Option<u64>,
}

#[derive(Deserialize, JsonSchema)]
pub struct QuoteId {
    quote_id: String,
}

fn text(value: Value) -> String {
    serde_json::to_string_pretty(&value).unwrap_or_default()
}

fn fail(error: impl std::fmt::Display) -> String {
    error.to_string()
}

fn distance_m(from: (f64, f64), to: (f64, f64)) -> f64 {
    let (lat1, lon1) = (from.0.to_radians(), from.1.to_radians());
    let (lat2, lon2) = (to.0.to_radians(), to.1.to_radians());
    let a = ((lat2 - lat1) / 2.0).sin().powi(2)
        + lat1.cos() * lat2.cos() * ((lon2 - lon1) / 2.0).sin().powi(2);
    2.0 * 6_371_000.0 * a.sqrt().asin()
}

/// What each payment state means for the user, and what to do next.
fn explain(status: &PaymentStatus) -> &'static str {
    match status {
        PaymentStatus::Placed { .. } => "Paid for and sent to the pub.",
        PaymentStatus::Pending => {
            "Waiting for the card to be entered on the payment page. Call order_status again to keep waiting."
        }
        PaymentStatus::InProgress => {
            "The card is being authorised; the user may need to approve it in their banking app. Call order_status again."
        }
        PaymentStatus::Paid => "Paid for, and being sent to the pub. Call order_status again.",
        PaymentStatus::Duplicate => {
            "Paid for, but held as a repeat of a recent order to this table. Ask the user whether to send it anyway \
             (send_duplicate) or cancel it (cancel_order)."
        }
        PaymentStatus::Cancelled => "Cancelled. Quote again to order.",
        PaymentStatus::Other { .. } => {
            "In a state this server doesn't know; the details are included."
        }
    }
}

fn status_report(order: &Order, status: &PaymentStatus) -> Value {
    json!({
        "quote_id": order.quote_id,
        "pub": order.venue_name,
        "table": order.table,
        "total": order.total,
        "payment": status,
        "meaning": explain(status),
    })
}

fn open_in_browser(url: &str) -> bool {
    if !cfg!(target_os = "macos") {
        return false;
    }
    std::process::Command::new("open")
        .arg(url)
        .status()
        .is_ok_and(|status| status.success())
}

impl Spoons {
    async fn venues(&self) -> Result<&[VenueSummary], String> {
        self.venues
            .get_or_try_init(|| self.client.venues())
            .await
            .map(Vec::as_slice)
            .map_err(fail)
    }

    async fn summary(&self, venue_ref: u64) -> Result<VenueSummary, String> {
        self.venues()
            .await?
            .iter()
            .find(|venue| venue.venue_ref == venue_ref)
            .cloned()
            .ok_or_else(|| format!("no pub with venue_ref {venue_ref}"))
    }

    /// Every product on the orderable menus of a sales area, with the id of the category it is listed under.
    async fn products(
        &self,
        franchise: &str,
        venue: u64,
        sales_area: u64,
    ) -> Result<Vec<(u64, Product)>, String> {
        let mut products = Vec::new();
        let menus = self
            .client
            .menus(franchise, venue, sales_area)
            .await
            .map_err(fail)?;
        for summary in menus
            .iter()
            .filter(|menu| menu.can_order && !menu.is_hidden)
        {
            let menu = self
                .client
                .menu(franchise, venue, sales_area, &summary.id)
                .await
                .map_err(fail)?;
            for category in menu.categories.into_iter().filter(|c| !c.hidden) {
                for group in category.item_groups {
                    for item in group.items {
                        if let Item::Product(product) = item
                            && !products
                                .iter()
                                .any(|(_, p): &(u64, Product)| p.id == product.id)
                        {
                            products.push((category.id, *product));
                        }
                    }
                }
            }
        }
        Ok(products)
    }

    async fn sales_area(
        &self,
        franchise: &str,
        venue: u64,
        table: Option<u32>,
    ) -> Result<u64, String> {
        if let Some(number) = table {
            let tables = self.client.tables(franchise, venue).await.map_err(fail)?;
            let table = tables
                .iter()
                .find(|table| table.number == number)
                .ok_or_else(|| format!("there is no table {number} at this pub"))?;
            if !table.available {
                return Err(format!("table {number} isn't taking orders right now"));
            }
            return Ok(table.sales_area_id);
        }
        let venue = self.client.venue(franchise, venue).await.map_err(fail)?;
        venue
            .sales_areas
            .first()
            .map(|area| area.id)
            .ok_or_else(|| "this pub has no menus to order from".to_owned())
    }

    /// Hands out the payment page once a payment is prepared and pending, unless it needs the user's say-so first.
    fn hand_out(
        &self,
        order: &mut Order,
        confirm_duplicate: bool,
        reopen: bool,
    ) -> Result<String, String> {
        let Payment::Prepared {
            url,
            duplicate_order,
            url_opened,
            ..
        } = &mut order.payment
        else {
            return Err("this quote has no payment prepared".to_owned());
        };
        if *duplicate_order && !confirm_duplicate {
            return Ok(text(json!({
                "quote_id": order.quote_id,
                "question": "A matching order went to this table recently. Ask the user whether they want another. \
                             If yes, call pay again with confirm_duplicate; if no, call cancel_order.",
            })));
        }
        let opened = if !*url_opened || reopen {
            let opened = open_in_browser(url);
            *url_opened |= opened;
            opened
        } else {
            false
        };
        let url = url.clone();
        self.store.save(order).map_err(fail)?;
        Ok(text(json!({
            "quote_id": order.quote_id,
            "pub": order.venue_name,
            "table": order.table,
            "total": order.total,
            "payment_page": url,
            "opened_in_browser": opened,
            "next": if opened {
                "The payment page is open in the browser. Once the user has entered their card and approved it in \
                 their banking app, call order_status with wait_secs."
            } else {
                "Give the user the payment page link. Once they have entered their card and approved it in their \
                 banking app, call order_status with wait_secs."
            },
        })))
    }
}

#[tool_router]
impl Spoons {
    /// Finds Wetherspoon's pubs by name, town or postcode, or the nearest to a point. Returns each pub's venue_ref,
    /// which the other tools take.
    #[tool(annotations(read_only_hint = true))]
    async fn find_pubs(&self, Parameters(query): Parameters<FindPubs>) -> Result<String, String> {
        let venues = self.venues().await?;
        let needle = query.query.as_deref().map(str::to_lowercase);
        let point = query.latitude.zip(query.longitude);
        let mut found: Vec<(Option<f64>, &VenueSummary)> = venues
            .iter()
            .filter(|venue| !venue.is_closed)
            .filter(|venue| {
                needle.as_ref().is_none_or(|needle| {
                    let address = &venue.address;
                    [
                        Some(&venue.name),
                        address.town.as_ref(),
                        address.postcode.as_ref(),
                    ]
                    .into_iter()
                    .flatten()
                    .any(|field| field.to_lowercase().contains(needle))
                })
            })
            .map(|venue| {
                let distance = point
                    .zip(venue.address.location.as_ref())
                    .map(|(point, at)| distance_m(point, (at.latitude, at.longitude)));
                (distance, venue)
            })
            .collect();
        if point.is_some() {
            found.sort_by(|a, b| a.0.unwrap_or(f64::MAX).total_cmp(&b.0.unwrap_or(f64::MAX)));
        }
        let pubs: Vec<Value> = found
            .into_iter()
            .take(query.limit.unwrap_or(5).clamp(1, 50))
            .map(|(distance, venue)| {
                json!({
                    "venue_ref": venue.venue_ref,
                    "name": venue.name,
                    "town": venue.address.town,
                    "postcode": venue.address.postcode,
                    "status": venue.status,
                    "distance_m": distance.map(|m| m.round()),
                })
            })
            .collect();
        Ok(text(json!(pubs)))
    }

    /// Lists what a pub sells, live from its menus, with each product's sizes and prices. Products marked not
    /// orderable can't be quoted yet.
    #[tool(annotations(read_only_hint = true))]
    async fn menu(&self, Parameters(query): Parameters<MenuQuery>) -> Result<String, String> {
        let venue = self.summary(query.venue_ref).await?;
        let area = self
            .sales_area(&venue.franchise, venue.venue_ref, query.table)
            .await?;
        let needle = query.search.as_deref().map(str::to_lowercase);
        let products = self
            .products(&venue.franchise, venue.venue_ref, area)
            .await?;
        let matching: Vec<&Product> = products
            .iter()
            .map(|(_, product)| product)
            .filter(|product| {
                needle
                    .as_ref()
                    .is_none_or(|needle| product.name.to_lowercase().contains(needle))
            })
            .collect();
        let listed: Vec<Value> = matching
            .iter()
            .take(MAX_MENU_RESULTS)
            .map(|product| {
                let unorderable = BasketLine::new(0, product, None, 1).err();
                json!({
                    "product_id": product.id,
                    "name": product.name,
                    "description": product.description,
                    "portions": product.portions().map(|(label, price)| json!({ "label": label, "price": price.value })).collect::<Vec<_>>(),
                    "orderable": unorderable.is_none(),
                    "why_not": unorderable.map(|error| error.to_string()),
                })
            })
            .collect();
        Ok(text(json!({
            "pub": venue.name,
            "products": listed,
            "more": matching.len().saturating_sub(MAX_MENU_RESULTS),
        })))
    }

    /// Prices a basket for a table at the pub's till, without ordering anything. Show the user the lines and total,
    /// then call pay with the quote_id once they agree.
    #[tool(annotations(read_only_hint = true))]
    async fn quote(&self, Parameters(request): Parameters<QuoteRequest>) -> Result<String, String> {
        if request.items.is_empty() || request.items.len() > MAX_LINES {
            return Err(format!("a quote takes 1 to {MAX_LINES} items"));
        }
        if let Some(item) = request
            .items
            .iter()
            .find(|item| !(1..=MAX_QUANTITY).contains(&item.quantity))
        {
            return Err(format!(
                "quantity {} for product {} isn't between 1 and {MAX_QUANTITY}",
                item.quantity, item.product_id
            ));
        }
        let venue = self.summary(request.venue_ref).await?;
        let (franchise, venue_ref) = (venue.franchise.as_str(), venue.venue_ref);
        let area = self
            .sales_area(franchise, venue_ref, Some(request.table))
            .await?;
        let products = self.products(franchise, venue_ref, area).await?;
        let mut basket = Vec::new();
        let mut lines = Vec::new();
        for item in &request.items {
            let (category, product) = products
                .iter()
                .find(|(_, product)| product.id == item.product_id)
                .ok_or_else(|| {
                    format!(
                        "product {} isn't on the menu for table {}",
                        item.product_id, request.table
                    )
                })?;
            let line = BasketLine::new(*category, product, item.portion.as_deref(), item.quantity)
                .map_err(fail)?;
            lines.push(Line {
                product_id: product.id,
                name: product.name.clone(),
                portion: line.portion.clone(),
                quantity: item.quantity,
                price: None,
            });
            basket.push(line);
        }
        let validation = self
            .client
            .validate_basket(
                franchise,
                venue_ref,
                area,
                &basket,
                &self.store.device_id().map_err(fail)?,
            )
            .await
            .map_err(fail)?;
        if validation.items.len() == lines.len() {
            for (line, priced) in lines.iter_mut().zip(&validation.items) {
                if priced.id == line.product_id {
                    line.price = Some(priced.price.value);
                }
            }
        }
        let order = Order {
            quote_id: uuid::Uuid::new_v4().simple().to_string()[..12].to_owned(),
            created_at: now(),
            franchise: franchise.to_owned(),
            venue_ref,
            venue_name: venue.name.clone(),
            sales_area: area,
            table: request.table,
            lines,
            total: validation.total.value,
            currency: validation
                .total
                .currency
                .clone()
                .unwrap_or_else(|| "GBP".to_owned()),
            token: validation.token,
            payment: Payment::None,
        };
        self.store.save(&order).map_err(fail)?;
        Ok(text(json!({
            "quote_id": order.quote_id,
            "pub": order.venue_name,
            "table": order.table,
            "lines": order.lines,
            "total": order.total,
            "currency": order.currency,
        })))
    }

    /// Starts paying for a quote and opens the payment page in the browser, where the user enters their card and
    /// approves it in their banking app. Safe to call again: it never starts a second payment for the same quote.
    #[tool(annotations(destructive_hint = true, idempotent_hint = true))]
    async fn pay(&self, Parameters(request): Parameters<PayRequest>) -> Result<String, String> {
        let _lock = self.store.lock(&request.quote_id).map_err(fail)?;
        let mut order = self.store.load(&request.quote_id).map_err(fail)?;
        match &order.payment {
            Payment::Cancelled => return Err("this quote was cancelled; quote again".to_owned()),
            Payment::Preparing | Payment::Unconfirmed { .. } if !request.force => {
                return Err(
                    "an earlier attempt to pay got no clear answer, so an order may exist. Ask the user to check for \
                     a receipt email or a charge; only if there is none, call pay with force."
                        .to_owned(),
                );
            }
            Payment::Prepared { transaction_id, .. } => {
                let status = self
                    .payments
                    .status(&order.franchise, order.venue_ref, transaction_id)
                    .await
                    .map_err(fail)?;
                return match status {
                    PaymentStatus::Pending => {
                        self.hand_out(&mut order, request.confirm_duplicate, request.reopen)
                    }
                    PaymentStatus::Cancelled => {
                        order.payment = Payment::Cancelled;
                        self.store.save(&order).map_err(fail)?;
                        Err("the payment page lapsed or was backed out of, which cancelled it; quote again".to_owned())
                    }
                    other => Ok(text(status_report(&order, &other))),
                };
            }
            _ => {}
        }

        let email = self.email.as_deref().ok_or(
            "set SPOONS_EMAIL in this server's environment to the address the receipt should go to",
        )?;
        order.payment = Payment::Preparing;
        self.store.save(&order).map_err(fail)?;
        let prepared = self
            .payments
            .prepare_card(
                &order.franchise,
                order.venue_ref,
                &CardOrder {
                    table: order.table,
                    currency: &order.currency,
                    amount: order.total,
                    email,
                    token: &order.token,
                },
            )
            .await;
        match prepared {
            Ok(prepared) => {
                order.payment = Payment::Prepared {
                    transaction_id: prepared.transaction_id,
                    url: prepared.url,
                    duplicate_order: prepared.duplicate_order,
                    url_opened: false,
                };
                self.store.save(&order).map_err(fail)?;
                self.hand_out(&mut order, request.confirm_duplicate, false)
            }
            Err(Error::Unconfirmed { detail, .. }) => {
                order.payment = Payment::Unconfirmed {
                    detail: detail.clone(),
                };
                self.store.save(&order).map_err(fail)?;
                Err(format!(
                    "the payment API gave no clear answer ({detail}), so an order may exist. Don't pay again: ask the \
                     user to check for a receipt email or a charge."
                ))
            }
            Err(error) => {
                order.payment = Payment::None;
                self.store.save(&order).map_err(fail)?;
                Err(fail(error))
            }
        }
    }

    /// Where a quote's payment has got to. With wait_secs, waits for it to finish or need the user's attention.
    #[tool(annotations(read_only_hint = true))]
    async fn order_status(
        &self,
        Parameters(request): Parameters<StatusRequest>,
    ) -> Result<String, String> {
        let order = self.store.load(&request.quote_id).map_err(fail)?;
        let Payment::Prepared { transaction_id, .. } = &order.payment else {
            return Ok(text(json!({
                "quote_id": order.quote_id,
                "payment": order.payment,
            })));
        };
        let deadline =
            Instant::now() + Duration::from_secs(request.wait_secs.unwrap_or(0).min(MAX_WAIT));
        loop {
            let status = self
                .payments
                .status(&order.franchise, order.venue_ref, transaction_id)
                .await
                .map_err(fail)?;
            let waiting = matches!(
                status,
                PaymentStatus::Pending | PaymentStatus::InProgress | PaymentStatus::Paid
            );
            if !waiting || Instant::now() + POLL > deadline {
                return Ok(text(status_report(&order, &status)));
            }
            sleep(POLL).await;
        }
    }

    /// Cancels a quote's payment. Only possible before a card is entered, or when the order is held as a duplicate.
    #[tool(annotations(destructive_hint = true, idempotent_hint = true))]
    async fn cancel_order(
        &self,
        Parameters(request): Parameters<QuoteId>,
    ) -> Result<String, String> {
        let _lock = self.store.lock(&request.quote_id).map_err(fail)?;
        let mut order = self.store.load(&request.quote_id).map_err(fail)?;
        match &order.payment {
            Payment::None | Payment::Cancelled => {}
            Payment::Preparing | Payment::Unconfirmed { .. } => {
                return Err(
                    "an earlier attempt to pay got no clear answer, so there is no payment to cancel by id. If a \
                     receipt email arrives, the order went through."
                        .to_owned(),
                );
            }
            Payment::Prepared { transaction_id, .. } => {
                self.payments
                    .cancel(&order.franchise, order.venue_ref, transaction_id)
                    .await
                    .map_err(fail)?;
            }
        }
        order.payment = Payment::Cancelled;
        self.store.save(&order).map_err(fail)?;
        Ok(text(
            json!({ "quote_id": order.quote_id, "payment": "cancelled" }),
        ))
    }

    /// Sends an order held as a repeat of a recent one to the pub anyway. Only when the user has said they want it.
    #[tool(annotations(destructive_hint = true, idempotent_hint = false))]
    async fn send_duplicate(
        &self,
        Parameters(request): Parameters<QuoteId>,
    ) -> Result<String, String> {
        let _lock = self.store.lock(&request.quote_id).map_err(fail)?;
        let order = self.store.load(&request.quote_id).map_err(fail)?;
        let Payment::Prepared { transaction_id, .. } = &order.payment else {
            return Err("this quote has no payment to send".to_owned());
        };
        match self
            .payments
            .resubmit(&order.franchise, order.venue_ref, transaction_id)
            .await
        {
            Ok(()) => {}
            Err(Error::Unconfirmed { detail, .. }) => {
                return Err(format!(
                    "no clear answer ({detail}), so it may have been sent. Call order_status rather than sending again."
                ));
            }
            Err(error) => return Err(fail(error)),
        }
        let status = self
            .payments
            .status(&order.franchise, order.venue_ref, transaction_id)
            .await
            .map_err(fail)?;
        Ok(text(status_report(&order, &status)))
    }
}

#[tool_handler(
    name = "spoons",
    instructions = "Orders food and drink to a table at a Wetherspoon's pub. Find the pub (find_pubs), look up \
                    products (menu), price the basket (quote) and show the user the lines and total, then pay once \
                    they agree. pay opens the payment page; the user enters their card and approves it in their \
                    banking app; follow with order_status. Never call pay with force, send_duplicate or \
                    confirm_duplicate without the user's say-so."
)]
impl ServerHandler for Spoons {}

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
