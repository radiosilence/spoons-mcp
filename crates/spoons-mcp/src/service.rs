//! Everything the GraphQL API does that reaches Wetherspoon's: the pub list, menus, tables, quotes and payments.
//!
//! Shared by every MCP session in the process. Nothing here retries a request that could place an order, and
//! nothing cancels an order on its own: cancelling while the bank is authorising a card can leave a charge with no
//! order. Quotes and payments live in [`Store`] on disk, so they outlive a session and are shared by processes.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use spoons_api_client::{
    Ale, BasketLine, Card, CardError, CardOrder, Client, Error, PAYMENT_PAGE, PaymentStatus,
    Payments, Table, Venue, VenueSummary, WaitTimes,
};
use tokio::sync::OnceCell;
use tokio::time::{Instant, sleep};

use crate::catalogue::Catalogue;
use crate::orders::{Line, Order, Payment, Store, now};

pub const MAX_LINES: usize = 10;
pub const MAX_QUANTITY: u32 = 10;
pub const MAX_WAIT: u64 = 55;
const POLL: Duration = Duration::from_secs(5);
/// How long `pay` watches a payment after entering the card, before leaving the rest to `order`.
const SETTLE: Duration = Duration::from_secs(30);

/// A venue ref and one of its sales areas.
type AreaKey = (u64, u64);

pub struct Service {
    client: Client,
    payments: Payments,
    pub store: Arc<Store>,
    venues: OnceCell<Vec<VenueSummary>>,
    /// Each sales area's menu, fetched at most once per London day, so a busy day of quotes reads the menu a
    /// handful of times rather than on every call. Prices stay exact: a quote's total comes from the till.
    menus: Mutex<HashMap<AreaKey, (jiff::civil::Date, Arc<Catalogue>)>>,
    /// One fetch of a sales area's menu at a time, so a query that asks for it twice reads it once.
    fetching: Mutex<HashMap<AreaKey, Arc<tokio::sync::Mutex<()>>>>,
    pub tokens: crate::tokens::TokenStore,
    /// Wait between polls of a payment; shortened in tests.
    poll: Duration,
    settle: Duration,
}

/// Where a pub's orders go: its franchise's routes and its venue ref.
#[derive(Clone, Debug)]
pub struct PubRef {
    pub franchise: String,
    pub venue_ref: u64,
    pub name: String,
    /// `GBP`, or `EUR` at the six pubs in the Republic of Ireland.
    pub currency: &'static str,
}

pub fn london_today() -> jiff::civil::Date {
    jiff::Timestamp::now().to_zoned(london()).date()
}

pub fn london() -> jiff::tz::TimeZone {
    jiff::tz::TimeZone::get("Europe/London").unwrap_or(jiff::tz::TimeZone::UTC)
}

pub fn fail(error: impl std::fmt::Display) -> String {
    error.to_string()
}

/// One item to quote.
#[derive(Clone, Debug)]
pub struct Wanted {
    pub product_id: u64,
    pub portion: Option<String>,
    pub quantity: u32,
}

/// What `pay` did, for the caller to report.
#[derive(Clone, Debug, Default)]
pub struct PayReport {
    pub status: Option<PaymentStatus>,
    pub payment_page: Option<String>,
    pub opened_in_browser: bool,
    /// A question the user has to answer before anything else happens.
    pub question: Option<String>,
    /// How entering the saved card went.
    pub card: Option<String>,
    pub next: String,
}

/// What each payment state means for the user, and what to do next.
pub fn explain(status: &PaymentStatus) -> &'static str {
    match status {
        PaymentStatus::Placed { .. } => "Paid for and sent to the pub.",
        PaymentStatus::Pending => {
            "Waiting for the card to be entered on the payment page. Query `order` again with `waitSecs` to keep waiting."
        }
        PaymentStatus::InProgress => {
            "The card is being authorised; the user may need to approve it in their banking app. Query `order` again with `waitSecs`."
        }
        PaymentStatus::Paid => {
            "Paid for, and being sent to the pub. Query `order` again with `waitSecs`."
        }
        PaymentStatus::Duplicate => {
            "Held back as a repeat of a recent order to this table. Ask the user whether to send it anyway \
             (`sendDuplicate`) or cancel it (`cancelOrder`)."
        }
        PaymentStatus::Cancelled => "Cancelled. Quote again to order.",
        PaymentStatus::Other { .. } => {
            "In a state this server doesn't know; the details are included."
        }
    }
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

impl Service {
    pub fn new(client: Client, payments: Payments, store: Store) -> Self {
        Self {
            client,
            payments,
            store: Arc::new(store),
            venues: OnceCell::new(),
            menus: Mutex::default(),
            fetching: Mutex::default(),
            tokens: crate::tokens::TokenStore::default(),
            poll: POLL,
            settle: SETTLE,
        }
    }

    /// Polls faster, for tests against a mock API.
    #[cfg(test)]
    pub fn with_poll(mut self, poll: Duration, settle: Duration) -> Self {
        self.poll = poll;
        self.settle = settle;
        self
    }

    pub async fn venues(&self) -> Result<&[VenueSummary], String> {
        self.venues
            .get_or_try_init(|| self.client.venues())
            .await
            .map(Vec::as_slice)
            .map_err(fail)
    }

    pub async fn summary(&self, venue_ref: u64) -> Result<VenueSummary, String> {
        self.venues()
            .await?
            .iter()
            .find(|venue| venue.venue_ref == venue_ref)
            .cloned()
            .ok_or_else(|| format!("no pub with ref {venue_ref}"))
    }

    pub async fn pub_ref(&self, venue_ref: u64) -> Result<PubRef, String> {
        let venue = self.summary(venue_ref).await?;
        Ok(PubRef {
            currency: crate::graphql::types::currency_of(&venue.address.country.code),
            franchise: venue.franchise,
            venue_ref: venue.venue_ref,
            name: venue.name,
        })
    }

    pub async fn venue(&self, pub_ref: &PubRef) -> Result<Venue, String> {
        self.client
            .venue(&pub_ref.franchise, pub_ref.venue_ref)
            .await
            .map_err(fail)
    }

    pub async fn wait_times(&self, pub_ref: &PubRef) -> Result<WaitTimes, String> {
        self.client
            .wait_times(&pub_ref.franchise, pub_ref.venue_ref)
            .await
            .map_err(fail)
    }

    pub async fn ales(&self, pub_ref: &PubRef) -> Result<Vec<Ale>, String> {
        self.client
            .ales(&pub_ref.franchise, pub_ref.venue_ref)
            .await
            .map_err(fail)
    }

    pub async fn tables(&self, pub_ref: &PubRef) -> Result<Vec<Table>, String> {
        self.client
            .tables(&pub_ref.franchise, pub_ref.venue_ref)
            .await
            .map_err(fail)
    }

    /// The sales area a table orders from, refusing a table that doesn't exist or isn't taking orders; the pub's
    /// first area when no table is given, for reading a menu before the user has sat down.
    pub async fn sales_area(&self, pub_ref: &PubRef, table: Option<u32>) -> Result<u64, String> {
        if let Some(number) = table {
            let tables = self.tables(pub_ref).await?;
            let table = tables
                .iter()
                .find(|table| table.number == number)
                .ok_or_else(|| format!("there is no table {number} at {}", pub_ref.name))?;
            if !table.available {
                return Err(format!("table {number} isn't taking orders right now"));
            }
            return Ok(table.sales_area_id);
        }
        let venue = self.venue(pub_ref).await?;
        venue
            .sales_areas
            .first()
            .map(|area| area.id)
            .ok_or_else(|| "this pub has no menus to order from".to_owned())
    }

    /// Every product on the orderable menus of a sales area; from today's copy when there is one.
    pub async fn catalogue(
        &self,
        pub_ref: &PubRef,
        sales_area: u64,
    ) -> Result<Arc<Catalogue>, String> {
        let key = (pub_ref.venue_ref, sales_area);
        let cached = || {
            let today = london_today();
            self.menus
                .lock()
                .unwrap()
                .get(&key)
                .filter(|(day, _)| *day == today)
                .map(|(_, catalogue)| catalogue.clone())
        };
        if let Some(catalogue) = cached() {
            return Ok(catalogue);
        }
        let gate = self
            .fetching
            .lock()
            .unwrap()
            .entry(key)
            .or_default()
            .clone();
        let _held = gate.lock().await;
        if let Some(catalogue) = cached() {
            return Ok(catalogue);
        }
        let catalogue = Arc::new(self.fetch_catalogue(pub_ref, sales_area).await?);
        self.menus
            .lock()
            .unwrap()
            .insert(key, (london_today(), catalogue.clone()));
        Ok(catalogue)
    }

    /// Drops a sales area's cached menu, for when the till disagrees with it.
    pub fn forget_menu(&self, venue: u64, sales_area: u64) {
        self.menus.lock().unwrap().remove(&(venue, sales_area));
    }

    async fn fetch_catalogue(
        &self,
        pub_ref: &PubRef,
        sales_area: u64,
    ) -> Result<Catalogue, String> {
        let (franchise, venue) = (pub_ref.franchise.as_str(), pub_ref.venue_ref);
        let summaries = self
            .client
            .menus(franchise, venue, sales_area)
            .await
            .map_err(fail)?;
        let mut menus = Vec::new();
        for summary in summaries.iter().filter(|m| m.can_order && !m.is_hidden) {
            menus.push(
                self.client
                    .menu(franchise, venue, sales_area, &summary.id)
                    .await
                    .map_err(fail)?,
            );
        }
        // The ale list only adds colours and styles, so a menu is still worth having without it.
        let ales = match self.client.ales(franchise, venue).await {
            Ok(ales) => ales,
            Err(error) => {
                tracing::warn!(%error, venue, "couldn't read the ale list");
                Vec::new()
            }
        };
        Ok(Catalogue::build(&menus, &ales))
    }

    /// Prices items at the till for a table and keeps the quote. Changes nothing at the pub.
    pub async fn quote(
        &self,
        pub_ref: &PubRef,
        table: u32,
        items: &[Wanted],
        device_id: &str,
    ) -> Result<Order, String> {
        if items.is_empty() || items.len() > MAX_LINES {
            return Err(format!("a quote takes 1 to {MAX_LINES} lines"));
        }
        if let Some(item) = items
            .iter()
            .find(|i| !(1..=MAX_QUANTITY).contains(&i.quantity))
        {
            return Err(format!(
                "quantity {} for product {} isn't between 1 and {MAX_QUANTITY}",
                item.quantity, item.product_id
            ));
        }
        let (franchise, venue_ref) = (pub_ref.franchise.as_str(), pub_ref.venue_ref);
        let area = self.sales_area(pub_ref, Some(table)).await?;
        let catalogue = self.catalogue(pub_ref, area).await?;
        let forget = |error: String| {
            self.forget_menu(venue_ref, area);
            error
        };
        let mut basket = Vec::new();
        let mut lines = Vec::new();
        for item in items {
            let entry = catalogue
                .get(item.product_id)
                .ok_or_else(|| {
                    format!(
                        "product {} isn't on the menu for table {table}",
                        item.product_id
                    )
                })
                .map_err(forget)?;
            let line = BasketLine::new(
                entry.category_id,
                &entry.product,
                item.portion.as_deref(),
                item.quantity,
            )
            .map_err(fail)?;
            let menu_price = entry
                .sizes
                .iter()
                .find(|s| s.label == line.portion)
                .map(|s| s.price);
            lines.push(Line {
                product_id: entry.product.id,
                name: entry.product.name.clone(),
                portion: line.portion.clone(),
                quantity: item.quantity,
                price: None,
                menu_price,
            });
            basket.push(line);
        }
        let validation = self
            .client
            .validate_basket(franchise, venue_ref, area, &basket, device_id)
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
            venue_name: pub_ref.name.clone(),
            sales_area: area,
            table,
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
        Ok(order)
    }

    pub fn device_id(&self) -> Result<String, String> {
        self.store.device_id().map_err(fail)
    }

    pub fn load(&self, quote_id: &str) -> Result<Order, String> {
        self.store.load(quote_id).map_err(fail)
    }

    /// Holds a quote for this call, or says another call has it, rather than waiting: the other may be a payment
    /// waiting up to a minute on the payment API.
    fn hold(&self, quote_id: &str) -> Result<std::fs::File, String> {
        self.store.try_lock(quote_id).map_err(fail)?.ok_or_else(|| {
            "another call is working on this quote; query `order` to see where it has got to"
                .to_owned()
        })
    }

    pub async fn status(&self, order: &Order) -> Result<Option<PaymentStatus>, String> {
        let Payment::Prepared { transaction_id, .. } = &order.payment else {
            return Ok(None);
        };
        self.payments
            .status(&order.franchise, order.venue_ref, transaction_id)
            .await
            .map(Some)
            .map_err(fail)
    }

    /// Where a payment has got to, waiting up to `wait` for it to finish or need the user.
    pub async fn wait(
        &self,
        order: &Order,
        wait: Duration,
    ) -> Result<Option<PaymentStatus>, String> {
        let deadline = Instant::now() + wait.min(Duration::from_secs(MAX_WAIT));
        loop {
            let Some(status) = self.status(order).await? else {
                return Ok(None);
            };
            let waiting = matches!(
                status,
                PaymentStatus::Pending | PaymentStatus::InProgress | PaymentStatus::Paid
            );
            if !waiting || Instant::now() + self.poll > deadline {
                return Ok(Some(status));
            }
            sleep(self.poll).await;
        }
    }

    /// Starts paying for a quote. With a saved card it enters the card itself and reports how the payment went;
    /// otherwise it hands out the payment page. Safe to call again: it never starts a second payment for the same
    /// quote, and never enters the card twice.
    pub async fn pay(
        &self,
        quote_id: &str,
        email: Option<String>,
        card: Option<Card>,
        confirm_duplicate: bool,
        reopen: bool,
    ) -> Result<(Order, PayReport), String> {
        let _lock = self.hold(quote_id)?;
        let mut order = self.load(quote_id)?;
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
                    PaymentStatus::Pending => {
                        let report = self
                            .settle(&mut order, card.as_ref(), confirm_duplicate, reopen, false)
                            .await;
                        Ok((order, report))
                    }
                    PaymentStatus::Cancelled => {
                        order.payment = Payment::Cancelled;
                        self.store.save(&order).map_err(fail)?;
                        Err("the payment page lapsed or was backed out of, which cancelled it; quote again"
                            .to_owned())
                    }
                    other => {
                        let next = explain(&other).to_owned();
                        Ok((
                            order,
                            PayReport {
                                status: Some(other),
                                next,
                                ..PayReport::default()
                            },
                        ))
                    }
                };
            }
        };

        let email = email.ok_or(
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
                let report = self
                    .settle(&mut order, card.as_ref(), confirm_duplicate, false, retried)
                    .await;
                Ok((order, report))
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
                Err("the payment API gave no clear answer. Nobody can pay a payment started that way, so it's \
                     safe to preview and confirm `pay` again."
                    .to_owned())
            }
            Err(error) => {
                order.payment = Payment::None;
                self.store.save(&order).map_err(fail)?;
                Err(fail(error))
            }
        }
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
    ) -> PayReport {
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

    /// Enters the card on a pending payment's page, once ever, then watches the payment for a little while. Hands
    /// out the payment page instead when the card never got there.
    async fn enter_card(&self, order: &mut Order, card: &Card, retried: bool) -> PayReport {
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
        let deadline = Instant::now() + self.settle;
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
            if Instant::now() + self.poll > deadline {
                break;
            }
            sleep(self.poll).await;
        }
        let Some(status) = status else {
            return PayReport {
                card: Some(
                    "The saved card was entered, but the payment's status couldn't be read. Don't pay again."
                        .to_owned(),
                ),
                next: "Query `order` with `waitSecs`.".to_owned(),
                ..PayReport::default()
            };
        };
        if status == PaymentStatus::Pending && note.is_none() {
            // The page never took the card, so the user can still pay on it. It is the same payment, so nothing is
            // charged twice.
            let mut report = self.hand_out(order, true, false, retried);
            report.card = Some(
                "The saved card was entered but the payment page didn't take it; the user can pay on the page."
                    .to_owned(),
            );
            return report;
        }
        let card = match (&note, &status) {
            (Some(note), _) => format!(
                "Entering the saved card went wrong ({note}). Don't pay again: query `order`, and if it stays \
                 unclear, ask the user to look for a charge or a receipt."
            ),
            (None, PaymentStatus::Placed { .. } | PaymentStatus::Paid) => {
                "Paid with the saved card.".to_owned()
            }
            (None, PaymentStatus::InProgress) => {
                "The saved card was entered and is being authorised; the user may need to approve it in their \
                 banking app."
                    .to_owned()
            }
            (None, _) => "The saved card was entered; see the status for what happened.".to_owned(),
        };
        PayReport {
            next: explain(&status).to_owned(),
            status: Some(status),
            card: Some(card),
            ..PayReport::default()
        }
    }

    /// Hands out the payment page once a payment is prepared and pending, unless it needs the user's say-so first.
    /// `retried` says an earlier attempt for this quote got no clear answer.
    fn hand_out(
        &self,
        order: &mut Order,
        confirm_duplicate: bool,
        reopen: bool,
        retried: bool,
    ) -> PayReport {
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
                 they ordered the same thing elsewhere in the last few minutes; if not, preview and confirm `pay` \
                 again with `confirmDuplicate: true`. If they did, cancel it with `cancelOrder`."
            } else {
                "A matching order went to this table recently. Ask the user whether they want another. If yes, \
                 preview and confirm `pay` again with `confirmDuplicate: true`; if no, `cancelOrder`."
            };
            return PayReport {
                question: Some(question.to_owned()),
                next: "Ask the user; do nothing until they answer.".to_owned(),
                ..PayReport::default()
            };
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
        PayReport {
            status: Some(PaymentStatus::Pending),
            payment_page: Some(url),
            opened_in_browser: opened,
            next: if opened {
                "The payment page is open in the browser. Once the user has entered their card and approved it in \
                 their banking app, query `order` with `waitSecs`."
            } else {
                "Give the user the payment page link. Once they have entered their card and approved it in their \
                 banking app, query `order` with `waitSecs`."
            }
            .to_owned(),
            ..PayReport::default()
        }
    }

    /// Cancels a quote's payment. Only possible before a card is entered, or when the order is held as a duplicate.
    pub async fn cancel(&self, quote_id: &str) -> Result<Order, String> {
        let _lock = self.hold(quote_id)?;
        let mut order = self.load(quote_id)?;
        // A payment started without a clear answer has no payment page anyone holds, so it lapses by itself.
        if let Payment::Prepared { transaction_id, .. } = &order.payment {
            self.payments
                .cancel(&order.franchise, order.venue_ref, transaction_id)
                .await
                .map_err(fail)?;
        }
        order.payment = Payment::Cancelled;
        self.store.save(&order).map_err(fail)?;
        Ok(order)
    }

    /// Sends an order held as a repeat of a recent one to the pub anyway. Sends it at most once.
    pub async fn send_duplicate(&self, quote_id: &str) -> Result<(Order, PaymentStatus), String> {
        let _lock = self.hold(quote_id)?;
        let mut order = self.load(quote_id)?;
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
                "this order has already been sent once; query `order` to see where it has got to"
                    .to_owned(),
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
                    "no clear answer, so it may have been sent. Query `order` rather than sending again.".to_owned(),
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
        Ok((order, status))
    }
}
