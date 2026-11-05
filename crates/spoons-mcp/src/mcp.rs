//! An MCP server over stdio for finding a pub, reading its menu and ordering to a table.
//!
//! Ordering is two steps: `quote` prices a basket at the till without starting anything, and `pay` prepares the card
//! payment and opens the payment page, where the card is entered and the bank approves it. Nothing here retries a
//! request that could place an order, and nothing cancels an order on its own: cancelling while the bank is
//! authorising a card can leave a charge with no order.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;

use anyhow::Result;
use rmcp::handler::server::wrapper::Parameters;

use rmcp::service::{RequestContext, RoleServer};
use rmcp::{ServerHandler, ServiceExt, tool, tool_handler, tool_router};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};
use spoons_api_client::{
    BasketLine, Card, CardError, CardOrder, Client, Error, Item, PAYMENT_PAGE, PaymentStatus,
    Payments, Product, VenueSummary,
};
use tokio::sync::OnceCell;
use tokio::time::{Instant, sleep};

use crate::orders::{Line, Order, Payment, Store, now};

const MAX_LINES: usize = 10;
const MAX_QUANTITY: u32 = 10;
const MAX_WAIT: u64 = 55;
const POLL: Duration = Duration::from_secs(5);
const MAX_MENU_RESULTS: usize = 50;
/// Over HTTP, where the receipt goes, per request: the gateway in front injects each user's own.
const EMAIL_HEADER: &str = "x-spoons-email";
/// Over HTTP, the card `pay` enters on the payment page: all four, or none, in which case the user pays on the page.
const CARD_HEADERS: [&str; 4] = [
    "x-spoons-card-number",
    "x-spoons-card-expiry",
    "x-spoons-card-cvc",
    "x-spoons-card-name",
];
/// How long `pay` watches a payment after entering the card, before leaving the rest to order_status.
const SETTLE: Duration = Duration::from_secs(30);

#[derive(Clone)]
pub struct Spoons {
    client: Client,
    payments: Payments,
    store: Arc<Store>,
    venues: Arc<OnceCell<Vec<VenueSummary>>>,
    /// Each sales area's products, fetched at most once per London day, so a busy day of quotes reads the menu a
    /// handful of times rather than on every call. Prices stay exact: a quote's total comes from the till.
    menus: Arc<Mutex<Menus>>,
    email: Option<String>,
}

type Products = Arc<Vec<(u64, Product)>>;
/// By venue and sales area, with the London day each was fetched.
type Menus = HashMap<(u64, u64), (jiff::civil::Date, Products)>;

fn london_today() -> jiff::civil::Date {
    let london = jiff::tz::TimeZone::get("Europe/London").unwrap_or(jiff::tz::TimeZone::UTC);
    jiff::Timestamp::now().to_zoned(london).date()
}

pub async fn serve(http: Option<&str>, state: Option<PathBuf>) -> Result<()> {
    let store = match state {
        Some(dir) => Store::at(dir)?,
        None => Store::open()?,
    };
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
        menus: Arc::default(),
        email: std::env::var("SPOONS_EMAIL")
            .ok()
            .filter(|email| !email.is_empty()),
    };
    match http {
        None => {
            server
                .serve(rmcp::transport::stdio())
                .await?
                .waiting()
                .await?;
        }
        Some(addr) => {
            use rmcp::transport::streamable_http_server::{
                StreamableHttpServerConfig, StreamableHttpService,
                session::local::LocalSessionManager,
            };
            // The Host allowlist guards a browser reaching a server on localhost; behind a proxy it only rejects
            // the internal Host the proxy forwards.
            let service = StreamableHttpService::new(
                move || Ok(server.clone()),
                Arc::new(LocalSessionManager::default()),
                StreamableHttpServerConfig::default().disable_allowed_hosts(),
            );
            let router = axum::Router::new().nest_service("/mcp", service);
            let listener = tokio::net::TcpListener::bind(addr).await?;
            tracing::info!("MCP over streamable HTTP on http://{addr}/mcp");
            axum::serve(listener, router).await?;
        }
    }
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
            "Held back as a repeat of a recent order to this table. Ask the user whether to send it anyway \
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

/// Only the payment page is opened: the url comes from a remote API, and `open` would hand anything else, such as a
/// file or an app, to the system.
fn open_in_browser(url: &str) -> bool {
    if !cfg!(target_os = "macos") || !url.starts_with(PAYMENT_PAGE) {
        return false;
    }
    std::process::Command::new("open")
        .arg(url)
        .status()
        .is_ok_and(|status| status.success())
}

impl Spoons {
    /// Where this request's receipt goes. Over HTTP only its `X-Spoons-Email` header counts, since the server is
    /// shared and the gateway in front injects each user's own; over stdio, `SPOONS_EMAIL`.
    fn email(&self, ctx: &RequestContext<RoleServer>) -> Option<String> {
        let header = |parts: &http::request::Parts| {
            parts
                .headers
                .get(EMAIL_HEADER)
                .and_then(|value| value.to_str().ok())
                .map(str::trim)
                .filter(|email| !email.is_empty())
                .map(str::to_owned)
        };
        match ctx.extensions.get::<http::request::Parts>() {
            Some(parts) => header(parts),
            None => self.email.clone(),
        }
    }

    /// The card to pay with, from this request's headers. Only over HTTP, where the gateway in front keeps it as the
    /// user's secret; a half-configured card is refused rather than quietly falling back to the payment page.
    fn card(&self, ctx: &RequestContext<RoleServer>) -> Result<Option<Card>, String> {
        let Some(parts) = ctx.extensions.get::<http::request::Parts>() else {
            return Ok(None);
        };
        let values = CARD_HEADERS.map(|name| {
            parts
                .headers
                .get(name)
                .and_then(|value| value.to_str().ok())
                .map(str::trim)
                .filter(|value| !value.is_empty())
        });
        match values {
            [None, None, None, None] => Ok(None),
            [Some(number), Some(expiry), Some(cvc), Some(name)] => {
                Card::new(number, expiry, cvc, name)
                    .map(Some)
                    .map_err(|error| format!("the saved card is unusable: {error}"))
            }
            _ => {
                Err("the saved card is missing some of its number, expiry, CVC and name".to_owned())
            }
        }
    }

    /// Enters the card on a pending payment's page, once ever, then watches the payment for a little while. Hands
    /// out the payment page instead when the card never got there.
    async fn enter_card(&self, order: &mut Order, card: &Card, retried: bool) -> String {
        let Payment::Prepared {
            transaction_id,
            url,
            card_entered,
            ..
        } = &mut order.payment
        else {
            unreachable!("entered a card for a quote with no payment prepared");
        };
        let (transaction_id, url) = (transaction_id.clone(), url.clone());
        // Recorded first, so a server that dies while entering it never enters it again.
        *card_entered = true;
        if let Err(error) = self.store.save(order) {
            tracing::warn!(%error, quote_id = order.quote_id, "couldn't record entering the card; leaving it to the user");
            if let Payment::Prepared { card_entered, .. } = &mut order.payment {
                *card_entered = false;
            }
            return self.hand_out(order, true, false, retried);
        }
        let note = match spoons_api_client::enter(&url, card).await {
            Ok(entered) => {
                tracing::info!(hops = ?entered.hops, quote_id = order.quote_id, "entered the card");
                None
            }
            Err(CardError::NotSent(reason)) => {
                tracing::warn!(reason, quote_id = order.quote_id, "couldn't enter the card");
                return self.hand_out(order, true, false, retried);
            }
            Err(error @ CardError::AfterSending(_)) => {
                tracing::warn!(%error, quote_id = order.quote_id, "entering the card went wrong");
                Some(error.to_string())
            }
        };
        // Pending stays in the wait: the payment API can take a moment to hear from the payment page.
        let deadline = Instant::now() + SETTLE;
        let mut status = None;
        loop {
            match self
                .payments
                .status(&order.franchise, order.venue_ref, &transaction_id)
                .await
            {
                Ok(now) => {
                    let waiting = matches!(
                        now,
                        PaymentStatus::Pending | PaymentStatus::InProgress | PaymentStatus::Paid
                    );
                    status = Some(now);
                    if !waiting {
                        break;
                    }
                }
                Err(error) => tracing::warn!(%error, "couldn't read the payment's status"),
            }
            if Instant::now() + POLL > deadline {
                break;
            }
            sleep(POLL).await;
        }
        let Some(status) = status else {
            return text(json!({
                "quote_id": order.quote_id,
                "card": "The saved card was entered, but the payment's status couldn't be read. Don't pay again; \
                         call order_status with wait_secs.",
            }));
        };
        if status == PaymentStatus::Pending && note.is_none() {
            // The page never took the card, so the user can still pay on it. It is the same payment, so nothing is
            // charged twice.
            let mut handed: Value =
                serde_json::from_str(&self.hand_out(order, true, false, retried))
                    .unwrap_or_default();
            handed["card"] = json!(
                "The saved card was entered but the payment page didn't take it; the user can pay on the page."
            );
            return text(handed);
        }
        let mut report = status_report(order, &status);
        report["card"] = json!(match (&note, &status) {
            (Some(note), _) => format!(
                "Entering the saved card went wrong ({note}). Don't pay again: call order_status, and if it stays \
                 unclear, ask the user to look for a charge or a receipt."
            ),
            (None, PaymentStatus::Placed { .. } | PaymentStatus::Paid) => "Paid with the saved card.".to_owned(),
            (None, PaymentStatus::InProgress) =>
                "The saved card was entered and is being authorised; the user may need to approve it in their \
                 banking app. Call order_status with wait_secs."
                    .to_owned(),
            (None, _) => "The saved card was entered; see payment for what happened.".to_owned(),
        });
        text(report)
    }

    /// Pays a pending payment with the saved card when there is one and it hasn't been tried, and otherwise hands
    /// out the payment page.
    async fn settle(
        &self,
        order: &mut Order,
        card: Option<&Card>,
        confirm_duplicate: bool,
        reopen: bool,
        retried: bool,
    ) -> String {
        if let (
            Some(card),
            Payment::Prepared {
                duplicate_order,
                card_entered: false,
                ..
            },
        ) = (card, &order.payment)
            && (!*duplicate_order || confirm_duplicate)
        {
            return self.enter_card(order, card, retried).await;
        }
        self.hand_out(order, confirm_duplicate, reopen, retried)
    }

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

    /// Every product on the orderable menus of a sales area, with the id of the category it is listed under; from
    /// today's copy when there is one.
    async fn products(
        &self,
        franchise: &str,
        venue: u64,
        sales_area: u64,
    ) -> Result<Products, String> {
        let today = london_today();
        let key = (venue, sales_area);
        if let Some((day, products)) = self.menus.lock().unwrap().get(&key)
            && *day == today
        {
            return Ok(products.clone());
        }
        let products = Arc::new(self.fetch_products(franchise, venue, sales_area).await?);
        self.menus
            .lock()
            .unwrap()
            .insert(key, (today, products.clone()));
        Ok(products)
    }

    /// Drops a sales area's cached menu, for when the till disagrees with it.
    fn forget_menu(&self, venue: u64, sales_area: u64) {
        self.menus.lock().unwrap().remove(&(venue, sales_area));
    }

    async fn fetch_products(
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

    /// Holds a quote for this call, or says another call has it, rather than waiting: the other may be a payment
    /// waiting up to a minute on the payment API.
    fn hold(&self, quote_id: &str) -> Result<std::fs::File, String> {
        self.store.try_lock(quote_id).map_err(fail)?.ok_or_else(|| {
            "another call is working on this quote; call order_status to see where it has got to".to_owned()
        })
    }

    /// Hands out the payment page once a payment is prepared and pending, unless it needs the user's say-so first.
    /// `retried` says an earlier attempt for this quote got no clear answer.
    fn hand_out(
        &self,
        order: &mut Order,
        confirm_duplicate: bool,
        reopen: bool,
        retried: bool,
    ) -> String {
        let Payment::Prepared {
            url,
            duplicate_order,
            url_opened,
            ..
        } = &mut order.payment
        else {
            unreachable!("handed out a quote with no payment prepared");
        };
        if *duplicate_order && !confirm_duplicate {
            let question = if retried {
                "The payment API says a matching order went to this table recently. That is most likely this \
                 quote's earlier attempt, which got no clear answer and can't be paid. Ask the user only whether \
                 they ordered the same thing elsewhere in the last few minutes; if not, call pay again with \
                 confirm_duplicate. If they did, call cancel_order."
            } else {
                "A matching order went to this table recently. Ask the user whether they want another. If yes, \
                 call pay again with confirm_duplicate; if no, call cancel_order."
            };
            return text(json!({ "quote_id": order.quote_id, "question": question }));
        }
        let opened = if !*url_opened || reopen {
            let opened = open_in_browser(url);
            *url_opened |= opened;
            opened
        } else {
            false
        };
        let url = url.clone();
        // Only records that the page was opened. Failing here must not report a failure while the page is open in
        // front of the user.
        if let Err(error) = self.store.save(order) {
            tracing::warn!(%error, quote_id = order.quote_id, "couldn't record that the payment page was opened");
        }
        text(json!({
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
        }))
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
        let forget = |error: String| {
            self.forget_menu(venue_ref, area);
            error
        };
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
                })
                .map_err(forget)?;
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
            .map_err(fail)
            .map_err(forget)?;
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

    /// Starts paying for a quote. With a saved card it enters the card itself and reports how the payment went;
    /// otherwise it opens the payment page, where the user enters their card. Either way the bank may ask the user to
    /// approve it in their banking app. Safe to call again: it never starts a second payment for the same quote, and
    /// never enters the card twice.
    #[tool(annotations(destructive_hint = true, idempotent_hint = true))]
    async fn pay(
        &self,
        ctx: RequestContext<RoleServer>,
        Parameters(request): Parameters<PayRequest>,
    ) -> Result<String, String> {
        let card = self.card(&ctx)?;
        let _lock = self.hold(&request.quote_id)?;
        let mut order = self.store.load(&request.quote_id).map_err(fail)?;
        // A payment whose request got no clear answer may exist, but its payment page only ever came back in that
        // answer, so nobody can pay it and it lapses. Preparing again is safe; the payment API flags the repeat.
        let retried = match &order.payment {
            Payment::Cancelled => return Err("this quote was cancelled; quote again".to_owned()),
            Payment::None => false,
            Payment::Preparing | Payment::Unconfirmed { .. } => true,
            Payment::Prepared { transaction_id, .. } => {
                let status = self
                    .payments
                    .status(&order.franchise, order.venue_ref, transaction_id)
                    .await
                    .map_err(fail)?;
                return match status {
                    PaymentStatus::Pending => Ok(self
                        .settle(
                            &mut order,
                            card.as_ref(),
                            request.confirm_duplicate,
                            request.reopen,
                            false,
                        )
                        .await),
                    PaymentStatus::Cancelled => {
                        order.payment = Payment::Cancelled;
                        self.store.save(&order).map_err(fail)?;
                        Err(
                            "the payment page lapsed or was backed out of, which cancelled it; quote again"
                                .to_owned(),
                        )
                    }
                    other => Ok(text(status_report(&order, &other))),
                };
            }
        };

        let email = self.email(&ctx).ok_or(
            "no receipt address: send X-Spoons-Email over HTTP, or set SPOONS_EMAIL for stdio",
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
                    email: &email,
                    token: &order.token,
                },
            )
            .await;
        match prepared {
            Ok(prepared) => {
                let transaction_id = prepared.transaction_id.clone();
                order.payment = Payment::Prepared {
                    transaction_id: prepared.transaction_id,
                    url: prepared.url,
                    duplicate_order: prepared.duplicate_order,
                    url_opened: false,
                    resubmitted: false,
                    card_entered: false,
                };
                if let Err(error) = self.store.save(&order) {
                    // Nobody has the payment page yet, so cancelling is safe, and a payment that isn't recorded
                    // couldn't be checked on or cancelled later.
                    tracing::error!(%error, transaction_id, "couldn't record a prepared payment; cancelling it");
                    let cancelled = self
                        .payments
                        .cancel(&order.franchise, order.venue_ref, &transaction_id)
                        .await;
                    return Err(format!(
                        "couldn't record the payment ({error}); cancelling transaction {transaction_id} {}",
                        match cancelled {
                            Ok(()) => "worked, so pay can be tried again".to_owned(),
                            Err(error) =>
                                format!("failed ({error}); it lapses by itself if nobody pays it"),
                        }
                    ));
                }
                Ok(self
                    .settle(
                        &mut order,
                        card.as_ref(),
                        request.confirm_duplicate,
                        false,
                        retried,
                    )
                    .await)
            }
            Err(Error::Unconfirmed { detail, .. }) => {
                // The detail can hold the payment page, which must not reach anyone: see above.
                tracing::warn!(
                    detail,
                    quote_id = order.quote_id,
                    "no clear answer preparing a payment"
                );
                order.payment = Payment::Unconfirmed { detail };
                self.store.save(&order).map_err(fail)?;
                Err(
                    "the payment API gave no clear answer. Nobody can pay a payment started that way, so it's safe \
                     to call pay again."
                        .to_owned(),
                )
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
        let _lock = self.hold(&request.quote_id)?;
        let mut order = self.store.load(&request.quote_id).map_err(fail)?;
        // A payment started without a clear answer has no payment page anyone holds, so it lapses by itself.
        if let Payment::Prepared { transaction_id, .. } = &order.payment {
            self.payments
                .cancel(&order.franchise, order.venue_ref, transaction_id)
                .await
                .map_err(fail)?;
        }
        order.payment = Payment::Cancelled;
        self.store.save(&order).map_err(fail)?;
        Ok(text(
            json!({ "quote_id": order.quote_id, "payment": "cancelled" }),
        ))
    }

    /// Sends an order held as a repeat of a recent one to the pub anyway. Only when the user has said they want it.
    /// Sends it at most once.
    #[tool(annotations(destructive_hint = true, idempotent_hint = false))]
    async fn send_duplicate(
        &self,
        Parameters(request): Parameters<QuoteId>,
    ) -> Result<String, String> {
        let _lock = self.hold(&request.quote_id)?;
        let mut order = self.store.load(&request.quote_id).map_err(fail)?;
        let Payment::Prepared {
            transaction_id,
            resubmitted,
            ..
        } = &mut order.payment
        else {
            return Err("this quote has no payment to send".to_owned());
        };
        if *resubmitted {
            return Err(
                "this order has already been sent once; call order_status to see where it has got to".to_owned(),
            );
        }
        // Recorded before sending, like a prepare: right after sending, the status can still read as a duplicate,
        // and a second send would order it twice.
        *resubmitted = true;
        let transaction_id = transaction_id.clone();
        self.store.save(&order).map_err(fail)?;
        match self
            .payments
            .resubmit(&order.franchise, order.venue_ref, &transaction_id)
            .await
        {
            Ok(()) => {}
            Err(Error::Unconfirmed { detail, .. }) => {
                tracing::warn!(
                    detail,
                    quote_id = order.quote_id,
                    "no clear answer sending a duplicate"
                );
                return Err(
                    "no clear answer, so it may have been sent. Call order_status rather than sending again."
                        .to_owned(),
                );
            }
            Err(error) => {
                if let Payment::Prepared { resubmitted, .. } = &mut order.payment {
                    *resubmitted = false;
                }
                self.store.save(&order).map_err(fail)?;
                return Err(fail(error));
            }
        }
        let status = self
            .payments
            .status(&order.franchise, order.venue_ref, &transaction_id)
            .await
            .map_err(fail)?;
        Ok(text(status_report(&order, &status)))
    }
}

#[tool_handler(
    name = "spoons",
    instructions = "Orders food and drink to a table at a Wetherspoon's pub. Find the pub (find_pubs), look up \
                    products (menu), price the basket (quote) and show the user the lines and total, then pay once \
                    they agree. With a saved card, pay enters it and reports the result; otherwise it gives a payment \
                    page where the user enters their card. The bank may ask the user to approve the payment in their \
                    banking app; follow with order_status.\n\n\
                    The server keeps no history between conversations, so take the pub, table and usual order from \
                    this one: for \"another\" or \"same again\", quote the last order's items for the same pub and \
                    table. Never guess a table: a drink is carried to whichever table is given, so ask when the user \
                    hasn't said one in this conversation, and check it again if they may have moved. Always show a \
                    quote's total and wait for the user's yes before pay. Never call send_duplicate or pay with \
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
