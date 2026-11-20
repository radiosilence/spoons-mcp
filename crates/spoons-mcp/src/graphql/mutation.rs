//! Changes: the basket, the table, quotes, and the three things that move money or settle an order.
//!
//! Basket changes are this conversation's own and reach nothing outside the server. `quote` prices at the till and
//! changes nothing at the pub. `pay`, `cancelOrder` and `sendDuplicate` are PREVIEW then CONFIRM: PREVIEW returns
//! what would happen and a token, and CONFIRM needs that token with the same arguments, which only the user's
//! explicit yes should send.

use std::sync::Arc;

use async_graphql::{Context, ID, Object, Result};

use super::Caller;
use super::types::{
    BasketResult, ConfirmAction, Money, Order, OrderActionResult, PayResult, Quote, QuoteItemInput,
    QuoteResult, order_status, service,
};
use crate::catalogue::Catalogue;
use crate::orders::Payment;
use crate::service::{MAX_LINES, MAX_QUANTITY, PubRef, Service, Wanted};
use crate::session::{Basket, Item, Session, line_id};

pub struct MutationRoot;

fn session<'a>(ctx: &Context<'a>) -> &'a Arc<Session> {
    ctx.data_unchecked::<Arc<Session>>()
}

fn caller<'a>(ctx: &Context<'a>) -> &'a Caller {
    ctx.data_unchecked::<Caller>()
}

fn parse_id(id: &ID) -> Result<u64, String> {
    id.parse()
        .map_err(|_| format!("{} isn't a product id", id.as_str()))
}

fn parse_table(table: i32) -> Result<u32, String> {
    u32::try_from(table)
        .ok()
        .filter(|t| *t > 0)
        .ok_or_else(|| format!("{table} isn't a table number"))
}

fn parse_quantity(quantity: i32) -> Result<u32, String> {
    u32::try_from(quantity)
        .ok()
        .filter(|q| (1..=MAX_QUANTITY).contains(q))
        .ok_or_else(|| format!("a quantity is 1 to {MAX_QUANTITY}"))
}

fn basket_result(session: &Session, outcome: Result<(), String>) -> BasketResult {
    let (success, error) = match outcome {
        Ok(()) => (true, None),
        Err(error) => (false, Some(error)),
    };
    BasketResult {
        success,
        error,
        basket: session.basket(),
    }
}

/// Puts a product into an item, priced from `catalogue`.
fn item(
    catalogue: &Catalogue,
    product_id: u64,
    portion: Option<&str>,
    quantity: u32,
) -> Result<Item, String> {
    let entry = catalogue
        .get(product_id)
        .ok_or_else(|| format!("product {product_id} isn't on this menu"))?;
    if let Some(why) = &entry.why_not {
        return Err(why.clone());
    }
    let size = match portion {
        Some(wanted) => entry
            .sizes
            .iter()
            .find(|s| s.label.eq_ignore_ascii_case(wanted) || s.name.eq_ignore_ascii_case(wanted))
            .ok_or_else(|| {
                format!(
                    "{} doesn't come as {wanted}; it comes as {}",
                    entry.product.name,
                    entry
                        .sizes
                        .iter()
                        .map(|s| s.name.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            })?,
        None => entry
            .default_size()
            .ok_or_else(|| format!("{} has no sizes to order", entry.product.name))?,
    };
    Ok(Item {
        product_id,
        name: entry.product.name.clone(),
        portion: size.label.clone(),
        quantity,
        unit_price: Some(size.price),
        kind: entry.kind,
        note: None,
    })
}

/// Re-prices every line against a sales area's menu, noting lines that aren't on it.
fn reprice(basket: &mut Basket, catalogue: &Catalogue, table: Option<u32>) {
    for line in &mut basket.items {
        match item(
            catalogue,
            line.product_id,
            Some(&line.portion),
            line.quantity,
        ) {
            Ok(fresh) => {
                line.unit_price = fresh.unit_price;
                line.note = None;
            }
            Err(error) => {
                line.unit_price = None;
                line.note = Some(match table {
                    Some(t) => format!("not orderable at table {t}: {error}"),
                    None => error,
                });
            }
        }
    }
}

/// The pub a change is for: the one named, which must match the basket's, or the basket's.
async fn which_pub(
    service: &Service,
    basket: &Basket,
    named: Option<i64>,
) -> Result<PubRef, String> {
    match (named, &basket.pub_ref) {
        (Some(named), Some(current))
            if named as u64 != current.venue_ref && !basket.items.is_empty() =>
        {
            Err(format!(
                "the basket is for {} (ref {}); clear it with basketClear before ordering from another pub",
                current.name, current.venue_ref
            ))
        }
        (Some(named), _) => service.pub_ref(named as u64).await,
        (None, Some(current)) => Ok(current.clone()),
        (None, None) => Err("which pub? Pass `pub` (its ref from `pubs`)".to_owned()),
    }
}

/// The catalogue the basket is priced from: its table's sales area, or the pub's first before there is a table.
async fn catalogue_for(
    service: &Service,
    pub_ref: &PubRef,
    table: Option<u32>,
) -> Result<(u64, Arc<Catalogue>), String> {
    let area = service.sales_area(pub_ref, table).await?;
    Ok((area, service.catalogue(pub_ref, area).await?))
}

fn same_pub(basket: &Basket, pub_ref: &PubRef) -> bool {
    basket
        .pub_ref
        .as_ref()
        .is_some_and(|p| p.venue_ref == pub_ref.venue_ref)
}

/// Changes. The basket's are this conversation's own; `pay`, `cancelOrder` and `sendDuplicate` are PREVIEW then
/// CONFIRM, and CONFIRM only on the user's explicit yes.
#[Object]
impl MutationRoot {
    /// Adds a product to this conversation's basket, priced from the basket's table's menu. Adding the same size
    /// again raises its quantity. The first add needs `pub`; `table` sets the table too, but only pass one the user
    /// gave. Free once the menu is read (see `Pub.menu` for that cost).
    async fn basket_add(
        &self,
        ctx: &Context<'_>,
        #[graphql(
            name = "pub",
            desc = "The pub's ref. Needed for the first item; must match after that."
        )]
        pub_: Option<i64>,
        #[graphql(desc = "The user's table number, if they've said it.")] table: Option<i32>,
        product_id: ID,
        #[graphql(
            desc = "A size's label or name, such as `Pint`; the default size when not given."
        )]
        portion: Option<String>,
        #[graphql(default = 1)] quantity: i32,
    ) -> BasketResult {
        let service = service(ctx);
        let session = session(ctx);
        let outcome = async {
            let product_id = parse_id(&product_id)?;
            let quantity = parse_quantity(quantity)?;
            let table = table.map(parse_table).transpose()?;
            let current = session.basket();
            let pub_ref = which_pub(service, &current, pub_).await?;
            let keep_table = if same_pub(&current, &pub_ref) {
                current.table
            } else {
                None
            };
            let table = table.or(keep_table);
            let (area, catalogue) = catalogue_for(service, &pub_ref, table).await?;
            let new = item(&catalogue, product_id, portion.as_deref(), quantity)?;
            session.change(|basket| {
                if !same_pub(basket, &pub_ref) {
                    *basket = Basket {
                        version: basket.version,
                        ..Basket::default()
                    };
                }
                basket.pub_ref = Some(pub_ref.clone());
                if basket.table != table || basket.sales_area != Some(area) {
                    basket.table = table;
                    basket.sales_area = Some(area);
                    reprice(basket, &catalogue, table);
                }
                let lines = basket.items.len();
                match basket.items.iter_mut().find(|i| i.id() == new.id()) {
                    Some(existing) => {
                        let total = existing.quantity + new.quantity;
                        if total > MAX_QUANTITY {
                            return Err(format!("at most {MAX_QUANTITY} of one thing in an order"));
                        }
                        existing.quantity = total;
                    }
                    None if lines >= MAX_LINES => {
                        return Err(format!(
                            "an order takes at most {MAX_LINES} different lines"
                        ));
                    }
                    None => basket.items.push(new),
                }
                Ok(())
            })
        }
        .await;
        basket_result(session, outcome)
    }

    /// Takes `quantity` off a basket line, or the whole line when not given. Free.
    async fn basket_remove(
        &self,
        ctx: &Context<'_>,
        #[graphql(desc = "The line's `id` from the basket.")] line_id: ID,
        quantity: Option<i32>,
    ) -> BasketResult {
        let session = session(ctx);
        let outcome = session.change(|basket| {
            let position = basket
                .items
                .iter()
                .position(|i| i.id() == line_id.as_str())
                .ok_or_else(|| format!("no line {} in the basket", line_id.as_str()))?;
            match quantity {
                Some(q) if q < 0 => return Err("a quantity to remove is positive".to_owned()),
                Some(q) if (q as u32) < basket.items[position].quantity => {
                    basket.items[position].quantity -= q as u32;
                }
                _ => {
                    basket.items.remove(position);
                }
            }
            Ok(())
        });
        basket_result(session, outcome)
    }

    /// Sets a line's quantity; 0 removes it. Name the line by `lineId`, or by `productId` and `portion` to add
    /// it at that quantity if it isn't there. Free once the menu is read.
    async fn basket_set(
        &self,
        ctx: &Context<'_>,
        line_id: Option<ID>,
        product_id: Option<ID>,
        portion: Option<String>,
        quantity: i32,
    ) -> BasketResult {
        let service = service(ctx);
        let session = session(ctx);
        let outcome = async {
            if quantity < 0 || quantity as u32 > MAX_QUANTITY {
                return Err(format!("a quantity is 0 to {MAX_QUANTITY}"));
            }
            let current = session.basket();
            let target = match (&line_id, &product_id) {
                (Some(id), _) => id.to_string(),
                (None, Some(product)) => {
                    let product = parse_id(product)?;
                    let pub_ref = which_pub(service, &current, None).await?;
                    let (_, catalogue) = catalogue_for(service, &pub_ref, current.table).await?;
                    let found = item(&catalogue, product, portion.as_deref(), 1)?;
                    if !current.items.iter().any(|i| i.id() == found.id()) && quantity > 0 {
                        let mut found = found;
                        found.quantity = quantity as u32;
                        return session.change(|basket| {
                            if basket.items.len() >= MAX_LINES {
                                return Err(format!(
                                    "an order takes at most {MAX_LINES} different lines"
                                ));
                            }
                            basket.items.push(found);
                            Ok(())
                        });
                    }
                    line_id_of(&found)
                }
                (None, None) => {
                    return Err("name the line: `lineId`, or `productId` and `portion`".to_owned());
                }
            };
            session.change(|basket| {
                let position = basket
                    .items
                    .iter()
                    .position(|i| i.id() == target)
                    .ok_or_else(|| format!("no line {target} in the basket"))?;
                if quantity == 0 {
                    basket.items.remove(position);
                } else {
                    basket.items[position].quantity = quantity as u32;
                }
                Ok(())
            })
        }
        .await;
        basket_result(session, outcome)
    }

    /// Empties the basket, keeping its pub and table. Free.
    async fn basket_clear(&self, ctx: &Context<'_>) -> BasketResult {
        let session = session(ctx);
        let outcome = session.change(|basket| {
            basket.items.clear();
            Ok(())
        });
        basket_result(session, outcome)
    }

    /// Fills the basket with a quote's lines, at the quote's pub and table: "same again". Replaces what was in it.
    /// Free once the menu is read.
    async fn basket_from_quote(&self, ctx: &Context<'_>, quote_id: ID) -> BasketResult {
        let service = service(ctx);
        let session = session(ctx);
        let outcome = async {
            let order = service.load(&quote_id)?;
            let pub_ref = service.pub_ref(order.venue_ref).await?;
            let (area, catalogue) = catalogue_for(service, &pub_ref, Some(order.table)).await?;
            session.change(|basket| {
                *basket = Basket {
                    pub_ref: Some(pub_ref.clone()),
                    table: Some(order.table),
                    sales_area: Some(area),
                    items: order
                        .lines
                        .iter()
                        .map(|l| Item {
                            product_id: l.product_id,
                            name: l.name.clone(),
                            portion: l.portion.clone(),
                            quantity: l.quantity,
                            unit_price: l.menu_price,
                            kind: catalogue
                                .get(l.product_id)
                                .map_or(crate::catalogue::Kind::Other, |e| e.kind),
                            note: None,
                        })
                        .collect(),
                    version: basket.version,
                };
                reprice(basket, &catalogue, Some(order.table));
                Ok(())
            })
        }
        .await;
        basket_result(session, outcome)
    }

    /// Sets the table the basket goes to, checking it exists and is taking orders, and re-prices the basket from
    /// that table's menu (a garden can have its own prices). Only a table the user gave: never guess one. One
    /// request for the table list, plus the menu if that sales area's isn't read yet.
    async fn set_table(
        &self,
        ctx: &Context<'_>,
        table: i32,
        #[graphql(
            name = "pub",
            desc = "The pub's ref, when the basket doesn't have one yet."
        )]
        pub_: Option<i64>,
    ) -> BasketResult {
        let service = service(ctx);
        let session = session(ctx);
        let outcome = async {
            let table = parse_table(table)?;
            let current = session.basket();
            let pub_ref = which_pub(service, &current, pub_).await?;
            let (area, catalogue) = catalogue_for(service, &pub_ref, Some(table)).await?;
            session.change(|basket| {
                if !same_pub(basket, &pub_ref) {
                    basket.items.clear();
                }
                basket.pub_ref = Some(pub_ref.clone());
                basket.table = Some(table);
                basket.sales_area = Some(area);
                reprice(basket, &catalogue, Some(table));
                Ok(())
            })
        }
        .await;
        basket_result(session, outcome)
    }

    /// Prices an order at the pub's till for a table, from `items` or else from the basket. Changes nothing at
    /// the pub. Show the user the lines, total and table, then on their yes preview and confirm `pay`. A table
    /// is required: ask the user rather than guess. Requests: the table list, the menu if not read today, and the
    /// till.
    async fn quote(
        &self,
        ctx: &Context<'_>,
        #[graphql(name = "pub", desc = "The pub's ref; the basket's when not given.")] pub_: Option<
            i64,
        >,
        #[graphql(desc = "The user's table; the basket's when not given.")] table: Option<i32>,
        #[graphql(desc = "What to quote; the basket when not given.")] items: Option<
            Vec<QuoteItemInput>,
        >,
    ) -> QuoteResult {
        let service = service(ctx);
        let session = session(ctx);
        let outcome = async {
            let basket = session.basket();
            let pub_ref = match (pub_, &basket.pub_ref) {
                (Some(named), _) => service.pub_ref(named as u64).await?,
                (None, Some(current)) => current.clone(),
                (None, None) => return Err("which pub? Pass `pub` (its ref from `pubs`)".to_owned()),
            };
            let table = match table {
                Some(t) => parse_table(t)?,
                None if same_pub(&basket, &pub_ref) => basket.table.ok_or(
                    "no table: ask the user which table number they're at (it's on the table), then setTable",
                )?,
                None => return Err("no table: ask the user which table number they're at".to_owned()),
            };
            let wanted: Vec<Wanted> = match items {
                Some(items) => items
                    .iter()
                    .map(|i| {
                        Ok(Wanted {
                            product_id: parse_id(&i.product_id)?,
                            portion: i.portion.clone(),
                            quantity: parse_quantity(i.quantity)?,
                        })
                    })
                    .collect::<Result<_, String>>()?,
                None if same_pub(&basket, &pub_ref) && !basket.items.is_empty() => basket
                    .items
                    .iter()
                    .map(|i| Wanted {
                        product_id: i.product_id,
                        portion: Some(i.portion.clone()),
                        quantity: i.quantity,
                    })
                    .collect(),
                None => return Err("the basket is empty: add something, or pass `items`".to_owned()),
            };
            let device = service.device_id()?;
            let order = service.quote(&pub_ref, table, &wanted, &device).await?;
            session.remember_quote(&order.quote_id);
            Ok(order)
        }
        .await;
        match outcome {
            Ok(order) => QuoteResult {
                success: true,
                error: None,
                quote: Some(Quote { order }),
            },
            Err(error) => QuoteResult {
                success: false,
                error: Some(error),
                quote: None,
            },
        }
    }

    /// Pays for a quote: PREVIEW, show the user the preview, and only on their explicit yes CONFIRM with the
    /// token. With a saved card CONFIRM enters it and reports how the payment went; otherwise it returns the
    /// payment page, where the user enters their card. The bank may ask the user to approve in their banking app:
    /// follow with `order(quoteId, waitSecs)`. Never starts a second payment for one quote, and never enters the
    /// card twice. `confirmDuplicate` only after the user has said they want a repeat of a recent order.
    #[allow(clippy::too_many_arguments)]
    async fn pay(
        &self,
        ctx: &Context<'_>,
        quote_id: ID,
        action: ConfirmAction,
        confirmation_token: Option<String>,
        #[graphql(default = false)] confirm_duplicate: bool,
        #[graphql(
            desc = "Open the payment page in the browser again (stdio on macOS only).",
            default = false
        )]
        reopen: bool,
    ) -> PayResult {
        let service = service(ctx);
        let caller = caller(ctx);
        let session = session(ctx);
        let failed = |action, error: String| PayResult {
            success: false,
            action,
            preview: None,
            confirmation_token: None,
            quote: None,
            status: None,
            payment_page: None,
            opened_in_browser: false,
            card: None,
            question: None,
            next: None,
            error: Some(error),
        };
        let order = match service.load(&quote_id) {
            Ok(order) => order,
            Err(error) => return failed(action, error),
        };
        let parts = [
            "pay",
            quote_id.as_str(),
            if confirm_duplicate { "duplicate" } else { "" },
        ];
        match action {
            ConfirmAction::Preview => {
                if matches!(order.payment, Payment::Cancelled) {
                    return failed(action, "this quote was cancelled; quote again".to_owned());
                }
                let quote = Quote { order };
                let card = match &caller.card {
                    Ok(Some(_)) => "with the saved card",
                    Ok(None) => "on a payment page the user opens",
                    Err(error) => return failed(action, error.clone()),
                };
                let receipt = caller
                    .email
                    .as_deref()
                    .map(|e| format!(" Receipt to {e}."))
                    .unwrap_or_default();
                let repeat = if confirm_duplicate {
                    " The user has said they want this although a matching order went to this table recently."
                } else {
                    ""
                };
                let started = if matches!(quote.order.payment, Payment::Prepared { .. }) {
                    " Paying has already started for this quote: CONFIRM picks it up where it is and never pays twice."
                } else {
                    ""
                };
                let preview = format!(
                    "Pay {} {card} for {}{receipt}{repeat}{started}",
                    Money::new(quote.order.total, &quote.order.currency).formatted,
                    quote.sentence(),
                );
                PayResult {
                    success: true,
                    action,
                    preview: Some(preview),
                    confirmation_token: Some(service.tokens.issue(caller.tenant(), &parts)),
                    quote: Some(quote),
                    status: None,
                    payment_page: None,
                    opened_in_browser: false,
                    card: None,
                    question: None,
                    next: Some(
                        "Show the user this preview and wait for their explicit yes before CONFIRM.".to_owned(),
                    ),
                    error: None,
                }
            }
            ConfirmAction::Confirm => {
                if let Err(error) =
                    service
                        .tokens
                        .spend(caller.tenant(), confirmation_token.as_deref(), &parts)
                {
                    return failed(action, error.to_owned());
                }
                let card = match &caller.card {
                    Ok(card) => card.clone(),
                    Err(error) => return failed(action, error.clone()),
                };
                match service
                    .pay(
                        &quote_id,
                        caller.email.clone(),
                        card,
                        confirm_duplicate,
                        reopen,
                    )
                    .await
                {
                    Ok((order, report)) => {
                        let status = order_status(&order.payment, report.status.as_ref());
                        // What's in the basket is what isn't ordered yet.
                        if report.question.is_none() && same_quote_basket(session, &order) {
                            let _ = session.change(|basket| {
                                basket.items.clear();
                                Ok(())
                            });
                        }
                        PayResult {
                            success: true,
                            action,
                            preview: None,
                            confirmation_token: None,
                            quote: Some(Quote { order }),
                            status: Some(status),
                            payment_page: report.payment_page,
                            opened_in_browser: report.opened_in_browser,
                            card: report.card,
                            question: report.question,
                            next: Some(report.next),
                            error: None,
                        }
                    }
                    Err(error) => failed(action, error),
                }
            }
        }
    }

    /// Cancels a quote's payment: PREVIEW, then CONFIRM on the user's yes. Only possible before a card is entered,
    /// or when the order is held as a duplicate; cancelling while a card is being authorised could leave a charge
    /// with no order, so it is refused then.
    async fn cancel_order(
        &self,
        ctx: &Context<'_>,
        quote_id: ID,
        action: ConfirmAction,
        confirmation_token: Option<String>,
    ) -> OrderActionResult {
        let service = service(ctx);
        let caller = caller(ctx);
        let parts = ["cancel", quote_id.as_str()];
        order_action(
            service,
            caller,
            &quote_id,
            action,
            confirmation_token,
            &parts,
            |quote| format!("Cancel the order of {}", quote.sentence()),
            |service, quote_id| async move {
                let order = service.cancel(&quote_id).await?;
                Ok(Order {
                    quote: Quote { order },
                    live: Some(spoons_api_client::PaymentStatus::Cancelled),
                })
            },
        )
        .await
    }

    /// Sends an order held as a repeat of a recent one (status DUPLICATE) to the pub anyway: PREVIEW, then CONFIRM
    /// only when the user has said they want it. Sends it at most once.
    async fn send_duplicate(
        &self,
        ctx: &Context<'_>,
        quote_id: ID,
        action: ConfirmAction,
        confirmation_token: Option<String>,
    ) -> OrderActionResult {
        let service = service(ctx);
        let caller = caller(ctx);
        let parts = ["duplicate", quote_id.as_str()];
        order_action(service, caller, &quote_id, action, confirmation_token, &parts, |quote| {
            format!(
                "Send this order to the pub although a matching one went to the same table recently: {}",
                quote.sentence()
            )
        }, |service, quote_id| async move {
            let (order, status) = service.send_duplicate(&quote_id).await?;
            Ok(Order {
                quote: Quote { order },
                live: Some(status),
            })
        })
        .await
    }
}

fn line_id_of(item: &Item) -> String {
    line_id(item.product_id, &item.portion)
}

/// The basket holds what this quote ordered, at its pub and table.
fn same_quote_basket(session: &Session, order: &crate::orders::Order) -> bool {
    let basket = session.basket();
    basket.table == Some(order.table)
        && basket
            .pub_ref
            .as_ref()
            .is_some_and(|p| p.venue_ref == order.venue_ref)
        && basket.items.len() == order.lines.len()
        && basket.items.iter().all(|i| {
            order.lines.iter().any(|l| {
                l.product_id == i.product_id && l.portion == i.portion && l.quantity == i.quantity
            })
        })
}

#[allow(clippy::too_many_arguments)]
async fn order_action<F, Fut>(
    service: &Arc<Service>,
    caller: &Caller,
    quote_id: &ID,
    action: ConfirmAction,
    token: Option<String>,
    parts: &[&str],
    describe: impl FnOnce(&Quote) -> String,
    act: F,
) -> OrderActionResult
where
    F: FnOnce(Arc<Service>, String) -> Fut,
    Fut: std::future::Future<Output = Result<Order, String>>,
{
    let failed = |error: String| OrderActionResult {
        success: false,
        action,
        preview: None,
        confirmation_token: None,
        order: None,
        error: Some(error),
    };
    let order = match service.load(quote_id) {
        Ok(order) => order,
        Err(error) => return failed(error),
    };
    match action {
        ConfirmAction::Preview => {
            let live = match service.status(&order).await {
                Ok(live) => live,
                Err(error) => return failed(error),
            };
            let quote = Quote { order };
            let preview = describe(&quote);
            OrderActionResult {
                success: true,
                action,
                preview: Some(preview),
                confirmation_token: Some(service.tokens.issue(caller.tenant(), parts)),
                order: Some(Order { quote, live }),
                error: None,
            }
        }
        ConfirmAction::Confirm => {
            if let Err(error) = service
                .tokens
                .spend(caller.tenant(), token.as_deref(), parts)
            {
                return failed(error.to_owned());
            }
            match act(service.clone(), quote_id.to_string()).await {
                Ok(order) => OrderActionResult {
                    success: true,
                    action,
                    preview: None,
                    confirmation_token: None,
                    order: Some(order),
                    error: None,
                },
                Err(error) => failed(error),
            }
        }
    }
}
