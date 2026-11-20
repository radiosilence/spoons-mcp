//! The graph's types: pubs, menus, products, the basket, quotes and orders.

use std::sync::Arc;

use async_graphql::connection::{Connection, Edge};
use async_graphql::dataloader::DataLoader;
use async_graphql::{ComplexObject, Context, Enum, ID, InputObject, Object, Result, SimpleObject};
use spoons_api_client::{Ale as ApiAle, PaymentStatus, VenueSummary};

use super::loaders::{AlesKey, PubLoader, TablesKey, VenueKey, WaitKey};
use crate::catalogue::{Catalogue, ColourSource, Entry, Kind};
use crate::orders::{Order as Stored, Payment};
use crate::service::{PubRef, Service, explain, london};
use crate::session::Basket as SessionBasket;

pub fn service<'a>(ctx: &Context<'a>) -> &'a Arc<Service> {
    ctx.data_unchecked::<Arc<Service>>()
}

pub fn loader<'a>(ctx: &Context<'a>) -> &'a DataLoader<PubLoader> {
    ctx.data_unchecked::<DataLoader<PubLoader>>()
}

pub fn gql(error: impl std::fmt::Display) -> async_graphql::Error {
    async_graphql::Error::new(error.to_string())
}

/// An amount of money.
#[derive(SimpleObject, Clone, Debug)]
pub struct Money {
    pub amount: f64,
    /// ISO 4217: `GBP`, or `EUR` at the six pubs in the Republic of Ireland.
    pub currency: String,
    /// Ready to show: `£4.49`.
    pub formatted: String,
}

impl Money {
    pub fn new(amount: f64, currency: &str) -> Self {
        let symbol = match currency {
            "GBP" => "£",
            "EUR" => "€",
            _ => "",
        };
        let formatted = if symbol.is_empty() {
            format!("{amount:.2} {currency}")
        } else {
            format!("{symbol}{amount:.2}")
        };
        Self {
            amount: (amount * 100.0).round() / 100.0,
            currency: currency.to_owned(),
            formatted,
        }
    }
}

pub fn currency_of(country_code: &str) -> &'static str {
    if country_code.eq_ignore_ascii_case("IE") {
        "EUR"
    } else {
        "GBP"
    }
}

/// A point, in degrees.
#[derive(SimpleObject, Clone, Debug)]
pub struct LatLng {
    pub lat: f64,
    pub lng: f64,
}

/// A point, in degrees: where the user is, for the nearest pubs.
#[derive(InputObject, Clone, Copy, Debug)]
pub struct LatLngInput {
    pub lat: f64,
    pub lng: f64,
}

/// Paging over a list already in hand: the cursor is an item's id, so the page after it is the same page whatever
/// else changed. Free: it slices what one call returned.
#[derive(SimpleObject, Clone, Debug)]
pub struct CountFields {
    /// How many match, before paging.
    pub total_count: u64,
    /// Nothing matched every word of the search, so these are the nearest, best first. Say so rather than
    /// presenting them as what was asked for.
    pub close_matches: bool,
}

pub type ListConnection<T> = Connection<String, T, CountFields>;

/// A page of `items` after the one whose key is `after`.
pub fn page<T: async_graphql::OutputType>(
    items: Vec<(String, T)>,
    after: Option<String>,
    first: Option<i32>,
    default: usize,
    most: usize,
    close_matches: bool,
) -> Result<ListConnection<T>> {
    let total = items.len();
    let start = match after {
        Some(after) => {
            items
                .iter()
                .position(|(key, _)| *key == after)
                .ok_or_else(|| {
                    gql(format!(
                        "no item {after} in this list; start again without `after`"
                    ))
                })?
                + 1
        }
        None => 0,
    };
    let take = first
        .map(|first| usize::try_from(first.max(1)).unwrap_or(1))
        .unwrap_or(default)
        .min(most);
    let end = (start + take).min(total);
    let mut connection = Connection::with_additional_fields(
        start > 0,
        end < total,
        CountFields {
            total_count: total as u64,
            close_matches,
        },
    );
    connection.edges.extend(
        items
            .into_iter()
            .skip(start)
            .take(end - start)
            .map(|(key, item)| Edge::new(key, item)),
    );
    Ok(connection)
}

/// The time in London.
pub struct Now;

/// The time in London.
#[Object]
impl Now {
    /// RFC 3339 with the offset: `2026-11-20T18:04:00+00:00`.
    async fn iso(&self) -> String {
        jiff::Timestamp::now()
            .to_zoned_datetime(london())
            .strftime("%Y-%m-%dT%H:%M:%S%:z")
            .to_string()
    }
    /// `18:04`.
    async fn time(&self) -> String {
        jiff::Timestamp::now()
            .to_zoned_datetime(london())
            .strftime("%H:%M")
            .to_string()
    }
    /// `2026-11-20`.
    async fn date(&self) -> String {
        jiff::Timestamp::now()
            .to_zoned_datetime(london())
            .date()
            .to_string()
    }
    /// `Friday`.
    async fn weekday(&self) -> String {
        jiff::Timestamp::now()
            .to_zoned_datetime(london())
            .strftime("%A")
            .to_string()
    }
}

/// A Wetherspoon's pub.
///
/// Its name, address, status and location come from the pub list, read once and kept, so they cost nothing.
/// Everything else says what it costs.
#[derive(Clone)]
pub struct Pub {
    pub venue: VenueSummary,
    pub distance: Option<f64>,
}

#[derive(SimpleObject, Clone)]
pub struct SalesArea {
    pub id: i64,
    /// Usually `Bar`; a garden or a hotel lounge can be a sales area with its own menu and prices.
    pub name: String,
}

#[derive(SimpleObject, Clone)]
pub struct Table {
    pub number: i32,
    pub name: Option<String>,
    /// Taking orders now.
    pub available: bool,
    /// The sales area it orders from, whose menu and prices apply.
    pub sales_area: i64,
}

/// Minutes an order is taking now.
#[derive(SimpleObject, Clone)]
pub struct WaitTimes {
    pub food_minutes: Option<i32>,
    pub drink_minutes: Option<i32>,
    /// The pub's local time, as the API gives it.
    pub updated_at: Option<String>,
}

#[derive(SimpleObject, Clone)]
pub struct OpeningHours {
    /// `08:00`.
    pub opens: Option<String>,
    /// `00:00`, which is midnight.
    pub closes: Option<String>,
    pub closed: bool,
}

impl Pub {
    pub fn pub_ref(&self) -> PubRef {
        PubRef {
            franchise: self.venue.franchise.clone(),
            venue_ref: self.venue.venue_ref,
            name: self.venue.name.clone(),
            currency: self.currency_code(),
        }
    }

    pub fn currency_code(&self) -> &'static str {
        currency_of(&self.venue.address.country.code)
    }

    /// The pub's details, or `None` when they couldn't be read: the fields built on them are then null, and the
    /// rest of a list still answers.
    async fn detail(&self, ctx: &Context<'_>) -> Option<spoons_api_client::Venue> {
        loader(ctx)
            .load_one(VenueKey(self.venue.venue_ref))
            .await
            .ok()
            .flatten()
    }
}

/// Today's hours from a venue's `openingTimes`: by date if today has an entry, else by weekday.
fn today_hours(venue: &spoons_api_client::Venue) -> Option<OpeningHours> {
    let times = venue.opening_times.as_ref()?;
    let now = jiff::Timestamp::now().to_zoned_datetime(london());
    let date = now.date().to_string();
    let day = now.strftime("%a").to_string().to_lowercase();
    let entry = times
        .get("dates")
        .and_then(|dates| dates.get(&date))
        .or_else(|| times.get("days").and_then(|days| days.get(&day)))?;
    Some(OpeningHours {
        opens: entry
            .get("open")
            .and_then(|v| v.as_str())
            .map(str::to_owned),
        closes: entry
            .get("close")
            .and_then(|v| v.as_str())
            .map(str::to_owned),
        closed: entry
            .get("isClosed")
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
    })
}

fn minutes(hhmm: &str) -> Option<i32> {
    let (h, m) = hhmm.split_once(':')?;
    Some(h.parse::<i32>().ok()? * 60 + m.parse::<i32>().ok()?)
}

/// A Wetherspoon's pub.
///
/// Its name, address, status and location come from the pub list, read once and kept, so they cost nothing.
/// Everything else says what it costs.
#[Object]
impl Pub {
    /// The pub's venue ref, which everything else takes as `pub`.
    #[graphql(name = "ref")]
    async fn venue_ref(&self) -> i64 {
        self.venue.venue_ref as i64
    }
    async fn name(&self) -> &str {
        &self.venue.name
    }
    async fn town(&self) -> Option<&str> {
        self.venue.address.town.as_deref()
    }
    async fn postcode(&self) -> Option<&str> {
        self.venue.address.postcode.as_deref()
    }
    /// The street address on one line.
    async fn address(&self) -> String {
        let a = &self.venue.address;
        [&a.line1, &a.line2, &a.line3, &a.town, &a.postcode]
            .into_iter()
            .flatten()
            .map(String::as_str)
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>()
            .join(", ")
    }
    async fn country(&self) -> &str {
        &self.venue.address.country.name
    }
    /// `open`, `closed_temporary`, `closing_temporary` or `opening_soon`, as the pub list has it. Free.
    async fn status(&self) -> &str {
        &self.venue.status
    }
    /// Trading, as opposed to closed for a refit or for good. Not whether it's open at this hour: see `openNow`.
    /// Free.
    async fn trading(&self) -> bool {
        self.venue.status == "open" && !self.venue.is_closed
    }
    async fn location(&self) -> Option<LatLng> {
        self.venue.address.location.as_ref().map(|l| LatLng {
            lat: l.latitude,
            lng: l.longitude,
        })
    }
    /// From `near`, when `pubs` was given one. Straight-line metres.
    async fn distance_metres(&self) -> Option<f64> {
        self.distance.map(f64::round)
    }
    /// `GBP`, or `EUR` in the Republic of Ireland.
    async fn currency(&self) -> &str {
        self.currency_code()
    }
    /// Today's opening hours. One request per pub, shared with `facilities`, `openNow`, `canOrder` and
    /// `salesAreas`.
    async fn hours_today(&self, ctx: &Context<'_>) -> Result<Option<OpeningHours>> {
        Ok(self.detail(ctx).await.as_ref().and_then(today_hours))
    }
    /// Open at this moment, by today's hours in London. Same request as `hoursToday`.
    async fn open_now(&self, ctx: &Context<'_>) -> Result<Option<bool>> {
        let Some(hours) = self.detail(ctx).await.as_ref().and_then(today_hours) else {
            return Ok(None);
        };
        if hours.closed || self.venue.status != "open" {
            return Ok(Some(false));
        }
        let (Some(opens), Some(closes)) = (
            hours.opens.as_deref().and_then(minutes),
            hours.closes.as_deref().and_then(minutes),
        ) else {
            return Ok(None);
        };
        let now = jiff::Timestamp::now().to_zoned_datetime(london());
        let now = i32::from(now.hour()) * 60 + i32::from(now.minute());
        // Closing at or after midnight runs into tomorrow; before opening, it may still be last night's session.
        Ok(Some(if closes <= opens {
            now >= opens || now < closes
        } else {
            (opens..closes).contains(&now)
        }))
    }
    /// Takes orders through the app. Same request as `hoursToday`.
    async fn can_order(&self, ctx: &Context<'_>) -> Option<bool> {
        self.detail(ctx).await.map(|d| d.can_place_order)
    }
    /// `Step-free access`, `Wi-Fi` and so on. Same request as `hoursToday`.
    async fn facilities(&self, ctx: &Context<'_>) -> Option<Vec<String>> {
        self.detail(ctx).await.map(|d| d.facilities)
    }
    /// The parts of the pub with their own menus. Same request as `hoursToday`.
    async fn sales_areas(&self, ctx: &Context<'_>) -> Option<Vec<SalesArea>> {
        self.detail(ctx).await.map(|d| {
            d.sales_areas
                .into_iter()
                .map(|a| SalesArea {
                    id: a.id as i64,
                    name: a.name,
                })
                .collect()
        })
    }
    /// How long food and drink are taking now. One request, live; null when the pub doesn't say.
    async fn wait_times(&self, ctx: &Context<'_>) -> Option<WaitTimes> {
        loader(ctx)
            .load_one(WaitKey(self.venue.venue_ref))
            .await
            .ok()
            .flatten()
            .map(|w| WaitTimes {
                food_minutes: w.food.map(|m| m as i32),
                drink_minutes: w.drink.map(|m| m as i32),
                updated_at: w.updated_at,
            })
    }
    /// The real ales on now and those coming soon, with their styles and colours. One request; null when the
    /// list couldn't be read, which isn't the same as none on.
    async fn ales(&self, ctx: &Context<'_>) -> Result<Option<Vec<Ale>>> {
        Ok(loader(ctx)
            .load_one(AlesKey(self.venue.venue_ref))
            .await
            .map_err(gql)?
            .map(|ales| ales.into_iter().map(Ale).collect()))
    }
    /// Its tables. One request; null when they couldn't be read.
    async fn tables(
        &self,
        ctx: &Context<'_>,
        #[graphql(desc = "Only those taking orders now.", default = false)] available_only: bool,
    ) -> Result<Option<Vec<Table>>> {
        Ok(loader(ctx)
            .load_one(TablesKey(self.venue.venue_ref))
            .await
            .map_err(gql)?
            .map(|tables| {
                tables
                    .into_iter()
                    .filter(|t| !available_only || t.available)
                    .map(|t| Table {
                        number: t.number as i32,
                        name: t.name,
                        available: t.available,
                        sales_area: t.sales_area_id as i64,
                    })
                    .collect()
            }))
    }
    /// Everything the pub sells to a table, sorted into kinds, with sizes and prices. Parts of a pub such as a
    /// garden can have their own menu and prices, so pass the `table` being ordered to when there is one.
    ///
    /// Cost: the first read of a sales area each day fetches its menus, about eight requests and several
    /// megabytes, plus one for the table list when `table` is given; after that it's free until midnight in
    /// London. Filtering, searching and paging are free.
    async fn menu(
        &self,
        ctx: &Context<'_>,
        #[graphql(desc = "The table being ordered to. Without it, the pub's main menu.")]
        table: Option<i32>,
        #[graphql(
            desc = "Only products matching every word, in name, brewery, style, category or taste notes."
        )]
        search: Option<String>,
        #[graphql(desc = "Only these kinds.")] kinds: Option<Vec<Kind>>,
    ) -> Result<Menu> {
        let service = service(ctx);
        let table = table
            .map(|t| u32::try_from(t).map_err(|_| gql("a table number is positive")))
            .transpose()?;
        let pub_ref = self.pub_ref();
        let area = service.sales_area(&pub_ref, table).await.map_err(gql)?;
        let catalogue = service.catalogue(&pub_ref, area).await.map_err(gql)?;
        Ok(Menu {
            pub_: self.clone(),
            table,
            sales_area: area,
            catalogue,
            search,
            kinds,
        })
    }
}

/// A real ale from the pub's ale list.
#[derive(Clone)]
pub struct Ale(pub ApiAle);

/// A real ale from the pub's ale list.
#[Object]
impl Ale {
    async fn id(&self) -> ID {
        ID(self.0.id.to_string())
    }
    async fn name(&self) -> &str {
        &self.0.name
    }
    async fn brewery(&self) -> Option<&str> {
        self.0.brewery.as_deref()
    }
    /// `Brewery - Name`, which is how the menu lists it.
    async fn full_name(&self) -> Option<&str> {
        self.0.full_name.as_deref()
    }
    /// `Dark brown/ruby` and so on.
    async fn style(&self) -> Option<&str> {
        self.0.style.as_ref().map(|s| s.label.as_str())
    }
    async fn colour(&self) -> Option<Colour> {
        let style = self.0.style.as_ref()?;
        Some(Colour {
            hex: style.hex_color.clone()?.to_lowercase(),
            label: style.label.clone(),
            source: ColourSource::AleList,
        })
    }
    async fn abv(&self) -> Option<f64> {
        self.0.abv
    }
    /// In a pint.
    async fn units(&self) -> Option<f64> {
        self.0.units
    }
    async fn description(&self) -> Option<&str> {
        self.0.description.as_deref()
    }
    /// Ordered in, not on yet.
    async fn coming_soon(&self) -> bool {
        self.0.coming_soon
    }
    /// The pub's price band for it, `A` cheapest.
    async fn price_band(&self) -> Option<&str> {
        self.0.price_band.as_deref()
    }
}

#[derive(SimpleObject, Clone, Debug)]
pub struct Colour {
    /// `#63130e`.
    pub hex: String,
    pub label: String,
    pub source: ColourSource,
}

impl From<&crate::catalogue::Colour> for Colour {
    fn from(c: &crate::catalogue::Colour) -> Self {
        Self {
            hex: c.hex.clone(),
            label: c.label.clone(),
            source: c.source,
        }
    }
}

/// One sales area's menu, as a table there gets it.
pub struct Menu {
    pub pub_: Pub,
    pub table: Option<u32>,
    pub sales_area: u64,
    pub catalogue: Arc<Catalogue>,
    pub search: Option<String>,
    pub kinds: Option<Vec<Kind>>,
}

impl Menu {
    fn wrap(&self, entry: &Entry) -> Product {
        Product {
            entry: entry.clone(),
            currency: self.pub_.currency_code(),
            catalogue: self.catalogue.clone(),
        }
    }

    fn matching<'a>(
        &'a self,
        search: Option<&'a str>,
        kinds: Option<&'a [Kind]>,
    ) -> impl Iterator<Item = &'a Entry> {
        self.catalogue.entries.iter().filter(move |e| {
            self.kinds.as_deref().is_none_or(|k| k.contains(&e.kind))
                && kinds.is_none_or(|k| k.contains(&e.kind))
                && self
                    .search
                    .as_deref()
                    .is_none_or(|s| crate::search::product_matches(s, e))
                && search.is_none_or(|s| crate::search::product_matches(s, e))
        })
    }
}

/// A category of the pub's own menu.
#[derive(SimpleObject, Clone)]
pub struct MenuCategory {
    pub name: String,
    /// The kind most of it is; the first product's.
    pub kind: Kind,
    pub count: u32,
}

/// One kind's products on a menu.
pub struct MenuSection {
    kind: Kind,
    products: Vec<Product>,
}

/// One kind's products on a menu.
#[Object]
impl MenuSection {
    async fn kind(&self) -> Kind {
        self.kind
    }
    /// `Cask ales`, `Alcohol-free` and so on.
    async fn label(&self) -> &str {
        self.kind.label()
    }
    async fn count(&self) -> usize {
        self.products.len()
    }
    /// In the pub's own order, guest ales first among cask ales.
    async fn products(
        &self,
        #[graphql(desc = "At most this many.")] first: Option<i32>,
    ) -> Vec<Product> {
        let take = first.map_or(usize::MAX, |f| usize::try_from(f.max(0)).unwrap_or(0));
        self.products.iter().take(take).cloned().collect()
    }
}

/// One sales area's menu, as a table there gets it. Everything below it is free once it's read.
#[Object]
impl Menu {
    #[graphql(name = "pub")]
    async fn pub_(&self) -> &Pub {
        &self.pub_
    }
    /// The table it is for, when one was given.
    async fn table(&self) -> Option<i32> {
        self.table.map(|t| t as i32)
    }
    async fn sales_area(&self) -> i64 {
        self.sales_area as i64
    }
    /// Products matching the menu's `search` and `kinds`.
    async fn total_count(&self) -> usize {
        self.matching(None, None).count()
    }
    /// The menu by kind, in the order people usually read it: ales, beers, cider, wine, spirits, cocktails,
    /// alcohol-free, soft and hot drinks, food. Empty kinds are left out.
    async fn sections(
        &self,
        #[graphql(desc = "Only these kinds.")] kinds: Option<Vec<Kind>>,
    ) -> Vec<MenuSection> {
        Kind::ALL
            .iter()
            .filter(|k| kinds.as_deref().is_none_or(|ks| ks.contains(k)))
            .filter_map(|kind| {
                let mut entries: Vec<&Entry> = self
                    .matching(None, Some(std::slice::from_ref(kind)))
                    .collect();
                entries.sort_by_key(|e| (!e.guest, e.position));
                (!entries.is_empty()).then(|| MenuSection {
                    kind: *kind,
                    products: entries.into_iter().map(|e| self.wrap(e)).collect(),
                })
            })
            .collect()
    }
    /// Products, in the pub's order, or with `search`, best match first. Search forgives what voice gets wrong:
    /// misheard words ("abot"), brand styling ("sumo" finds "Mad Squirrel $umo"), and what a drink is rather than
    /// its name ("mocktail", "virgin", "alcohol free", "beer", "soft drink"), across name, the pub's category,
    /// kind, style, notes and description. When nothing has every word, the nearest come back with
    /// `closeMatches`. Cursors are product ids; page with `first` and `after`. Free once the menu is read.
    #[allow(clippy::too_many_arguments)]
    async fn products(
        &self,
        #[graphql(desc = "What someone asked for, in their words.")] search: Option<String>,
        kinds: Option<Vec<Kind>>,
        #[graphql(
            desc = "Only the pub's own category (or group) of this name, from `categories`; forgiving."
        )]
        category: Option<String>,
        #[graphql(desc = "Only guest ales.", default = false)] guest_only: bool,
        #[graphql(desc = "Only what can be ordered now.", default = false)] orderable_only: bool,
        #[graphql(desc = "Page size, 50 when not given, at most 300.")] first: Option<i32>,
        after: Option<String>,
    ) -> Result<ListConnection<Product>> {
        let wanted = category.as_deref();
        // The menu's own search ranks too, when this field has none.
        let search = search.or_else(|| self.search.clone());
        let filtered = self
            .matching(None, kinds.as_deref())
            .filter(|e| !guest_only || e.guest)
            .filter(|e| !orderable_only || e.orderable())
            .filter(|e| {
                wanted.is_none_or(|w| {
                    [Some(e.category.as_str()), e.group.as_deref()]
                        .into_iter()
                        .flatten()
                        .any(|c| crate::search::category_matches(w, c))
                })
            })
            .collect::<Vec<&Entry>>();
        let (entries, close): (Vec<&Entry>, bool) = match search
            .as_deref()
            .and_then(|s| crate::search::products(s, filtered.iter().copied()))
        {
            Some(ranked) => (
                ranked.items.into_iter().map(|(_, e)| e).collect(),
                ranked.close,
            ),
            None => (filtered, false),
        };
        let items = entries
            .into_iter()
            .map(|e| (e.product.id.to_string(), self.wrap(e)))
            .collect();
        page(items, after, first, 50, 300, close)
    }
    /// The pub's own menu categories, as its app lists them ("0% cocktails", "Soft drinks", "Spritz cocktails"),
    /// with each one's kind and how many products are in it. Pass a name to `products(category:)`. Free.
    async fn categories(&self) -> Vec<MenuCategory> {
        let mut out: Vec<MenuCategory> = Vec::new();
        for e in self.matching(None, None) {
            match out.iter_mut().find(|c| c.name == e.category) {
                Some(c) => c.count += 1,
                None => out.push(MenuCategory {
                    name: e.category.clone(),
                    kind: e.kind,
                    count: 1,
                }),
            }
        }
        out
    }
    /// The guest real ales on now: the rotating casks, named "Brewery - Beer", that a search for "ale" misses.
    async fn guest_ales(&self) -> Vec<Product> {
        self.catalogue
            .entries
            .iter()
            .filter(|e| e.guest)
            .map(|e| self.wrap(e))
            .collect()
    }
    /// Real ales ordered in and not on yet. Not orderable.
    async fn coming_soon(&self) -> Vec<Ale> {
        self.catalogue
            .coming_soon
            .iter()
            .cloned()
            .map(Ale)
            .collect()
    }
    /// One product by id, whatever the menu's filters.
    async fn product(&self, id: ID) -> Option<Product> {
        let id: u64 = id.parse().ok()?;
        self.catalogue.get(id).map(|e| self.wrap(e))
    }
}

/// Something on a menu.
#[derive(Clone)]
pub struct Product {
    pub entry: Entry,
    pub currency: &'static str,
    pub catalogue: Arc<Catalogue>,
}

/// A question a product asks before it can be ordered, such as a mixer or a side. One option per group.
#[derive(SimpleObject, Clone)]
pub struct OptionGroup {
    /// What `options` and `choices` can name it by.
    pub id: ID,
    /// `Choose a side?`, `Mixer Choice`.
    pub name: String,
    /// Must be answered before it can be quoted. Ask the user; don't pick for them.
    pub required: bool,
    /// The menu never asks for more than one option a group: 1 when required, else 0.
    pub min_picks: i32,
    pub max_picks: i32,
    /// The sizes that ask it, when not every size does.
    pub sizes: Vec<String>,
    pub options: Vec<ProductOption>,
}

/// One answer to an option group.
#[derive(SimpleObject, Clone)]
pub struct ProductOption {
    pub id: ID,
    pub label: String,
    /// What it adds to the price; nothing for most.
    pub price: Money,
    pub kcal: Option<f64>,
    pub available: bool,
}

/// What kind of deal.
#[derive(Enum, Copy, Clone, Eq, PartialEq, Debug)]
pub enum DealKind {
    /// A meal that comes with a drink: pass `includeDrink` with one from `includedDrinks`.
    IncludesDrink,
    /// Any N for a price: the till takes it off when enough are ordered.
    MultiBuy,
    Other,
}

/// A deal a product is part of. The till applies deals; a quote shows what they `savings`.
#[derive(SimpleObject, Clone)]
pub struct Deal {
    /// As the menu says it: `Includes a drink`, `Any 2 for £6.60`.
    pub name: String,
    pub kind: DealKind,
    /// For a multi-buy, how many make the deal.
    pub buy: Option<i32>,
}

/// One size a product comes in, with its price at this table.
#[derive(SimpleObject, Clone)]
pub struct Portion {
    /// As the menu labels it, which is what `portion` arguments match: `250ml glass (185 kcal, 3.0 units)`.
    pub label: String,
    /// The size alone: `250ml glass`, `Pint`, `Double`.
    pub name: String,
    pub price: Money,
    pub is_default: bool,
    pub millilitres: Option<f64>,
    /// Alcohol units, from the label or worked out from the ABV and size.
    pub units: Option<f64>,
    pub kcal: Option<f64>,
}

/// Something on a menu, with what could be read from its description.
#[Object]
impl Product {
    /// What `basketAdd` and `quote` take.
    async fn id(&self) -> ID {
        ID(self.entry.product.id.to_string())
    }
    /// As the menu has it: `Rooster’s - Scoundrel`.
    async fn name(&self) -> &str {
        &self.entry.product.name
    }
    /// Without the brewery, for a guest ale: `Scoundrel`.
    async fn short_name(&self) -> &str {
        &self.entry.short_name
    }
    async fn brewery(&self) -> Option<&str> {
        self.entry.brewery.as_deref()
    }
    async fn kind(&self) -> Kind {
        self.entry.kind
    }
    /// A guest real ale, as opposed to one the pub always has.
    async fn guest(&self) -> bool {
        self.entry.guest
    }
    /// The pub's own category for it, and the group within: `Wine, prosecco & sparkling` / `Red wine`.
    async fn category(&self) -> &str {
        &self.entry.category
    }
    async fn group(&self) -> Option<&str> {
        self.entry.group.as_deref()
    }
    async fn description(&self) -> Option<&str> {
        self.entry.product.description.as_deref()
    }
    /// Read from the description, best-effort: `roasted`, `chocolaty`.
    async fn taste_notes(&self) -> &[String] {
        &self.entry.notes
    }
    /// For a wine, where it's from.
    async fn origin(&self) -> Option<&str> {
        self.entry.origin.as_deref()
    }
    /// A real ale's style from the ale list.
    async fn style(&self) -> Option<&str> {
        self.entry.style.as_deref()
    }
    /// Percent.
    async fn abv(&self) -> Option<f64> {
        self.entry.abv
    }
    /// Alcohol units in the default size.
    async fn units(&self) -> Option<f64> {
        self.entry.units
    }
    /// Calories in the default size.
    async fn kcal(&self) -> Option<f64> {
        self.entry.kcal
    }
    /// The drink's colour, from the ale list, the description, or what's typical of its kind.
    async fn colour(&self) -> Option<Colour> {
        self.entry.colour.as_ref().map(Colour::from)
    }
    async fn portions(&self) -> Vec<Portion> {
        self.entry.sizes.iter().map(|s| self.portion(s)).collect()
    }
    async fn default_portion(&self) -> Option<Portion> {
        self.entry.default_size().map(|s| self.portion(s))
    }
    /// Can be put in a basket and quoted now.
    async fn orderable(&self) -> bool {
        self.entry.orderable()
    }
    /// Why not, when it can't: out of stock, or a spirit's mixer choice, which can't be ordered here yet.
    async fn why_not(&self) -> Option<&str> {
        self.entry.why_not.as_deref()
    }
    async fn out_of_stock(&self) -> bool {
        self.entry.product.is_out_of_stock
    }
    /// Allergens as the menu tags them: `gluten`, `milk`.
    async fn allergens(&self) -> Vec<&str> {
        self.entry.product.allergens().collect()
    }
    async fn vegan(&self) -> bool {
        self.entry
            .product
            .keywords
            .iter()
            .any(|k| k.kind == "vegan")
    }
    /// Sold to over-18s only.
    async fn age_restricted(&self) -> bool {
        self.entry.product.age_restriction.is_some_and(|a| a >= 18)
    }
    /// The questions it asks before it can be ordered: a spirit's mixer, a meal's side. Answer them with
    /// `choices` (the user's words, such as \"chips\" or \"lemonade\") or `options` on `basketAdd` or a quote item.
    async fn option_groups(&self) -> Vec<OptionGroup> {
        let product = &self.entry.product;
        let sizes: Vec<&spoons_api_client::PortionOption> = product
            .options
            .portion
            .iter()
            .flat_map(|p| &p.options)
            .filter(|o| !o.value.hidden)
            .collect();
        product
            .options
            .choices
            .iter()
            .filter_map(|group| {
                let asking: Vec<&&spoons_api_client::PortionOption> = sizes
                    .iter()
                    .filter(|s| s.value.choices.iter().any(|c| c.choice_id == group.id))
                    .collect();
                if asking.is_empty() {
                    return None;
                }
                let required = group.required
                    || asking.iter().any(|s| {
                        s.value
                            .choices
                            .iter()
                            .any(|c| c.choice_id == group.id && c.required)
                    });
                Some(OptionGroup {
                    id: ID(group.id.to_string()),
                    name: group.title().to_owned(),
                    required,
                    min_picks: i32::from(required),
                    max_picks: 1,
                    sizes: if asking.len() == sizes.len() {
                        Vec::new()
                    } else {
                        asking
                            .iter()
                            .map(|s| s.label.split(" (").next().unwrap_or(&s.label).to_owned())
                            .collect()
                    },
                    options: group
                        .options
                        .iter()
                        .map(|o| ProductOption {
                            id: ID(o.id.to_string()),
                            label: o.name.clone(),
                            price: Money::new(
                                spoons_api_client::option_price(o).unwrap_or(0.0),
                                self.currency,
                            ),
                            kcal: o.calories,
                            available: !o.is_out_of_stock,
                        })
                        .collect(),
                })
            })
            .collect()
    }
    /// Asks something that must be answered before ordering. Free.
    async fn needs_choices(&self) -> bool {
        let product = &self.entry.product;
        product
            .options
            .portion
            .iter()
            .flat_map(|p| &p.options)
            .any(|o| o.value.choices.iter().any(|c| c.required))
    }
    /// The deals it's part of. Free.
    async fn deals(&self) -> Vec<Deal> {
        self.entry
            .product
            .options
            .linked
            .iter()
            .filter(|l| !l.name.trim().is_empty())
            .map(|l| {
                let name = l.name.trim().to_owned();
                let lower = name.to_lowercase();
                let buy = regex::Regex::new(r"(?i)any (\d+) for")
                    .ok()
                    .and_then(|re| re.captures(&name))
                    .and_then(|c| c[1].parse().ok());
                let kind = if lower.contains("includes a drink") {
                    DealKind::IncludesDrink
                } else if buy.is_some() {
                    DealKind::MultiBuy
                } else {
                    DealKind::Other
                };
                Deal { name, kind, buy }
            })
            .collect()
    }
    /// For a meal that includes a drink, the drinks it can come with, matching `search` (forgiving, as product
    /// search is). Pass one as `includeDrink`. Free.
    async fn included_drinks(
        &self,
        search: Option<String>,
        #[graphql(desc = "At most this many, 20 when not given.")] first: Option<i32>,
    ) -> Vec<Product> {
        let Some(deal) = self.catalogue.included_drinks(&self.entry.product) else {
            return Vec::new();
        };
        let entries: Vec<&Entry> = deal
            .products
            .iter()
            .filter_map(|p| self.catalogue.get(p.id))
            .collect();
        let ranked: Vec<&Entry> = match search
            .as_deref()
            .and_then(|s| crate::search::products(s, entries.iter().copied()))
        {
            Some(r) => r.items.into_iter().map(|(_, e)| e).collect(),
            None => entries,
        };
        ranked
            .into_iter()
            .take(first.map_or(20, |f| usize::try_from(f.max(1)).unwrap_or(20)))
            .map(|e| Product {
                entry: e.clone(),
                currency: self.currency,
                catalogue: self.catalogue.clone(),
            })
            .collect()
    }
}

impl Product {
    fn portion(&self, size: &crate::catalogue::Size) -> Portion {
        Portion {
            label: size.label.clone(),
            name: size.name.clone(),
            price: Money::new(size.price, self.currency),
            is_default: size.is_default,
            millilitres: size.millilitres,
            units: size.units,
            kcal: size
                .kcal
                .or_else(|| size.is_default.then_some(self.entry.kcal).flatten()),
        }
    }
}

/// What's being ordered in this conversation, kept by the server so the model and any app showing it agree.
pub struct Basket {
    pub basket: SessionBasket,
}

/// A line in the basket.
#[derive(SimpleObject, Clone)]
pub struct BasketLine {
    /// What `basketRemove` and `basketSet` take.
    pub id: ID,
    pub product_id: ID,
    pub name: String,
    pub portion: String,
    pub quantity: i32,
    /// The options chosen: `Peas`, `Chips`.
    pub options: Vec<String>,
    /// For a meal's included drink, the meal line's `id` it goes with; it follows the meal's quantity.
    pub included_with: Option<ID>,
    pub kind: Kind,
    pub colour: Option<Colour>,
    /// One of it, at the menu's price for the basket's table.
    pub unit_price: Option<Money>,
    pub line_total: Option<Money>,
    /// Why it can't be quoted as it is.
    pub note: Option<String>,
}

impl Basket {
    fn currency(&self) -> &'static str {
        self.basket.pub_ref.as_ref().map_or("GBP", |p| p.currency)
    }

    /// One line a person would read: "4 × Abbot Ale (Pint) to table 4 at The Watch House: £14.68 on the menu".
    pub fn sentence(&self) -> String {
        let b = &self.basket;
        if b.items.is_empty() {
            return match &b.pub_ref {
                Some(p) => format!("The basket is empty (at {}).", p.name),
                None => "The basket is empty.".to_owned(),
            };
        }
        let lines = b
            .items
            .iter()
            .map(|i| {
                let what = describe_line(&i.name, &i.portion, &i.choices);
                match &i.with {
                    Some(_) => format!("{} × {what}, included with the meal", i.quantity),
                    None => format!("{} × {what}", i.quantity),
                }
            })
            .collect::<Vec<_>>()
            .join(", ");
        let place = match (&b.table, &b.pub_ref) {
            (Some(t), Some(p)) => format!(" to table {t} at {}", p.name),
            (None, Some(p)) => format!(" at {}, no table yet", p.name),
            _ => String::new(),
        };
        let total = b
            .total()
            .map(|t| format!(": {} on the menu", Money::new(t, self.currency()).formatted))
            .unwrap_or_default();
        format!("{lines}{place}{total}.")
    }
}

/// What's being ordered in this conversation, kept by the server so the model and any app showing it agree.
#[Object]
impl Basket {
    /// The pub it's for, once something is in it or a table is set.
    #[graphql(name = "pub")]
    async fn pub_(&self, ctx: &Context<'_>) -> Result<Option<Pub>> {
        let Some(pub_ref) = &self.basket.pub_ref else {
            return Ok(None);
        };
        let venue = service(ctx).summary(pub_ref.venue_ref).await.map_err(gql)?;
        Ok(Some(Pub {
            venue,
            distance: None,
        }))
    }
    /// The table it goes to. Only ever what the user said: null until `setTable`.
    async fn table(&self) -> Option<i32> {
        self.basket.table.map(|t| t as i32)
    }
    async fn lines(&self) -> Vec<BasketLine> {
        let currency = self.currency();
        self.basket
            .items
            .iter()
            .map(|i| BasketLine {
                id: ID(i.id()),
                product_id: ID(i.product_id.to_string()),
                name: i.name.clone(),
                portion: i.portion.clone(),
                quantity: i.quantity as i32,
                options: i.choices.clone(),
                included_with: i.with.clone().map(ID),
                kind: i.kind,
                colour: i.colour.as_ref().map(Colour::from),
                unit_price: i.unit_price.map(|p| Money::new(p, currency)),
                line_total: i
                    .unit_price
                    .map(|p| Money::new(p * f64::from(i.quantity), currency)),
                note: i.note.clone(),
            })
            .collect()
    }
    /// Items, counting quantities.
    async fn count(&self) -> u32 {
        self.basket.count()
    }
    /// At the menu's prices. The till's total, which is what's charged, comes with `quote`.
    async fn menu_total(&self) -> Option<Money> {
        self.basket.total().map(|t| Money::new(t, self.currency()))
    }
    /// Goes up with every change.
    async fn version(&self) -> u64 {
        self.basket.version
    }
    /// Ready to quote: something in it, a table set, and every line on that table's menu.
    async fn ready(&self) -> bool {
        !self.basket.items.is_empty()
            && self.basket.table.is_some()
            && self.basket.items.iter().all(|i| i.note.is_none())
    }
    /// The basket in a sentence.
    async fn summary(&self) -> String {
        self.sentence()
    }
}

/// Where an order has got to.
#[derive(Enum, Copy, Clone, Debug, Eq, PartialEq, serde::Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum OrderStatus {
    /// Quoted; `pay` hasn't been confirmed.
    NotStarted,
    /// A request to start the payment was being sent when this was read.
    Preparing,
    /// Starting the payment got no clear answer. Nobody can pay a payment started that way; `pay` can be confirmed
    /// again.
    Unconfirmed,
    /// Waiting for the card on the payment page.
    Pending,
    /// The card is being authorised; the user may need to approve it in their banking app.
    InProgress,
    /// Paid for, being sent to the pub.
    Paid,
    /// Paid for and sent to the pub.
    Placed,
    /// Held back as a repeat of a recent order to this table: `sendDuplicate` or `cancelOrder`, on the user's word.
    Duplicate,
    Cancelled,
    /// A state this server doesn't know; see `detail`.
    Unknown,
}

pub fn order_status(payment: &Payment, live: Option<&PaymentStatus>) -> OrderStatus {
    if let Some(live) = live {
        return match live {
            PaymentStatus::Placed { .. } => OrderStatus::Placed,
            PaymentStatus::Pending => OrderStatus::Pending,
            PaymentStatus::InProgress => OrderStatus::InProgress,
            PaymentStatus::Paid => OrderStatus::Paid,
            PaymentStatus::Duplicate => OrderStatus::Duplicate,
            PaymentStatus::Cancelled => OrderStatus::Cancelled,
            PaymentStatus::Other { .. } => OrderStatus::Unknown,
        };
    }
    match payment {
        Payment::None => OrderStatus::NotStarted,
        Payment::Preparing => OrderStatus::Preparing,
        Payment::Unconfirmed { .. } => OrderStatus::Unconfirmed,
        Payment::Prepared { .. } => OrderStatus::Pending,
        Payment::Cancelled => OrderStatus::Cancelled,
    }
}

pub fn meaning(status: OrderStatus, live: Option<&PaymentStatus>) -> String {
    if let Some(live) = live {
        return explain(live).to_owned();
    }
    match status {
        OrderStatus::NotStarted => {
            "Quoted, not paid. Show the user the lines and total; on their yes, preview and confirm `pay`."
        }
        OrderStatus::Preparing => "A payment was being started. Query `order` again.",
        OrderStatus::Unconfirmed => {
            "Starting the payment got no clear answer; nobody can pay it, so `pay` can be previewed and confirmed \
             again."
        }
        OrderStatus::Cancelled => "Cancelled. Quote again to order.",
        _ => "See `status`.",
    }
    .to_owned()
}

/// A basket priced at the pub's till for a table. Quoting changes nothing at the pub.
pub struct Quote {
    pub order: Stored,
}

/// "Steak & ale pudding (Peas, Chips)", "Hennessy VS (Double, R White's lemonade)", "Carling (Pint)": the size when
/// there's a choice of size, and the options.
pub fn describe_line(name: &str, portion: &str, options: &[String]) -> String {
    let size = portion.split(" (").next().unwrap_or(portion).trim();
    let mut parts: Vec<&str> = Vec::new();
    if !size.eq_ignore_ascii_case("standard") && !size.is_empty() {
        parts.push(size);
    }
    parts.extend(options.iter().map(String::as_str));
    if parts.is_empty() {
        name.to_owned()
    } else {
        format!("{name} ({})", parts.join(", "))
    }
}

/// One line of a quote.
#[derive(SimpleObject, Clone)]
pub struct QuoteLine {
    pub product_id: ID,
    pub name: String,
    pub portion: String,
    pub quantity: i32,
    /// The options chosen: `Peas`, `Chips`.
    pub options: Vec<String>,
    /// The line as a person reads it: `Steak & ale pudding (Peas, Chips)`.
    pub description: String,
    /// The deal it's ordered through: `included with Steak & ale pudding`.
    pub deal: Option<String>,
    /// As the till priced the line.
    pub till_price: Option<Money>,
    /// One of it as the menu priced it.
    pub menu_price: Option<Money>,
}

impl Quote {
    pub fn currency(&self) -> &str {
        &self.order.currency
    }

    pub fn menu_sum(&self) -> Option<f64> {
        self.order
            .lines
            .iter()
            .map(|l| l.menu_price.map(|p| p * f64::from(l.quantity)))
            .sum::<Option<f64>>()
            .map(|t| (t * 100.0).round() / 100.0)
    }

    /// "4 × Greene King Abbot Ale (Pint) to table 4 at The Watch House: £14.68."
    pub fn sentence(&self) -> String {
        let o = &self.order;
        let lines = o
            .lines
            .iter()
            .map(|l| {
                let what = describe_line(&l.name, &l.portion, &l.choices);
                match &l.deal {
                    Some(deal) => format!("{} × {what}, {deal}", l.quantity),
                    None => format!("{} × {what}", l.quantity),
                }
            })
            .collect::<Vec<_>>()
            .join(", ");
        let total = Money::new(o.total, &o.currency).formatted;
        let saved = o.savings.unwrap_or(0.0);
        let deals = if saved > 0.004 {
            format!(
                " Deals took off {}.",
                Money::new(saved, &o.currency).formatted
            )
        } else {
            String::new()
        };
        let differs = match self.menu_sum() {
            Some(menu) if (menu - saved - o.total).abs() > 0.005 => format!(
                " The menu says {}; the till's price is the one charged.",
                Money::new(menu, &o.currency).formatted
            ),
            _ => String::new(),
        };
        format!(
            "{lines} to table {} at {}: {total}.{deals}{differs}",
            o.table, o.venue_name
        )
    }
}

/// A basket priced at the pub's till for a table. Quoting changes nothing at the pub.
#[Object]
impl Quote {
    /// The quote id, which `pay`, `order`, `cancelOrder` and `sendDuplicate` take.
    async fn id(&self) -> ID {
        ID(self.order.quote_id.clone())
    }
    /// The pub's ref.
    async fn pub_ref(&self) -> i64 {
        self.order.venue_ref as i64
    }
    async fn pub_name(&self) -> &str {
        &self.order.venue_name
    }
    async fn table(&self) -> i32 {
        self.order.table as i32
    }
    async fn lines(&self) -> Vec<QuoteLine> {
        let currency = self.currency();
        self.order
            .lines
            .iter()
            .map(|l| QuoteLine {
                product_id: ID(l.product_id.to_string()),
                name: l.name.clone(),
                portion: l.portion.clone(),
                quantity: l.quantity as i32,
                options: l.choices.clone(),
                description: describe_line(&l.name, &l.portion, &l.choices),
                deal: l.deal.clone(),
                till_price: l.price.map(|p| Money::new(p, currency)),
                menu_price: l.menu_price.map(|p| Money::new(p, currency)),
            })
            .collect()
    }
    /// What deals took off (a meal's included drink, a multi-buy), when they did. Already out of `total`.
    async fn savings(&self) -> Option<Money> {
        self.order
            .savings
            .map(|s| Money::new(s, &self.order.currency))
    }
    /// What will be charged, from the till.
    async fn total(&self) -> Money {
        Money::new(self.order.total, &self.order.currency)
    }
    /// The same lines at the menu's prices, when every line had one.
    async fn menu_total(&self) -> Option<Money> {
        self.menu_sum().map(|t| Money::new(t, &self.order.currency))
    }
    /// The till charges something other than the menu says.
    async fn differs_from_menu(&self) -> bool {
        self.menu_sum().is_some_and(|m| {
            (m - self.order.savings.unwrap_or(0.0) - self.order.total).abs() > 0.005
        })
    }
    /// When it was quoted, in London.
    async fn created_at(&self) -> String {
        jiff::Timestamp::from_second(self.order.created_at as i64)
            .map(|t| {
                t.to_zoned_datetime(london())
                    .strftime("%Y-%m-%dT%H:%M:%S%:z")
                    .to_string()
            })
            .unwrap_or_default()
    }
    /// Where its payment is, as this server last recorded it. Free; `order` asks the payment API.
    async fn status(&self) -> OrderStatus {
        order_status(&self.order.payment, None)
    }
    /// The quote in a sentence, with any difference between till and menu.
    async fn summary(&self) -> String {
        self.sentence()
    }
}

/// An order: a quote and where its payment has got to.
pub struct Order {
    pub quote: Quote,
    pub live: Option<PaymentStatus>,
}

/// An order: a quote and where its payment has got to.
#[Object]
impl Order {
    async fn quote(&self) -> &Quote {
        &self.quote
    }
    async fn status(&self) -> OrderStatus {
        order_status(&self.quote.order.payment, self.live.as_ref())
    }
    /// What the status means for the user, and what to do next.
    async fn meaning(&self) -> String {
        meaning(
            order_status(&self.quote.order.payment, self.live.as_ref()),
            self.live.as_ref(),
        )
    }
    /// The pub's order number, once placed.
    async fn order_ref(&self) -> Option<&str> {
        match &self.live {
            Some(PaymentStatus::Placed { order_ref, .. }) => Some(order_ref),
            _ => None,
        }
    }
    async fn placed_at(&self) -> Option<&str> {
        match &self.live {
            Some(PaymentStatus::Placed { placed_at, .. }) => placed_at.as_deref(),
            _ => None,
        }
    }
    /// The payment page, while it waits for a card.
    async fn payment_page(&self) -> Option<&str> {
        match (&self.quote.order.payment, &self.live) {
            (Payment::Prepared { url, .. }, Some(PaymentStatus::Pending)) => Some(url),
            _ => None,
        }
    }
    /// The payment API's own words, for a state this server doesn't know.
    async fn detail(&self) -> Option<String> {
        match &self.live {
            Some(PaymentStatus::Other {
                order_status,
                payment_status,
                message,
            }) => Some(format!(
                "order {}, payment {}: {}",
                order_status.as_deref().unwrap_or("?"),
                payment_status.as_deref().unwrap_or("?"),
                message.as_deref().unwrap_or("")
            )),
            _ => None,
        }
    }
}

/// Two-step actions: look, then act with the token the look returned.
#[derive(Enum, Copy, Clone, Eq, PartialEq, Debug)]
pub enum ConfirmAction {
    /// Show what would happen and return a `confirmationToken`. Changes nothing.
    Preview,
    /// Do it, with the token from PREVIEW and the same arguments. Only on the user's explicit yes.
    Confirm,
}

#[derive(InputObject, Clone, Debug)]
pub struct QuoteItemInput {
    pub product_id: ID,
    /// A size's label or name, such as `Pint` or `Half pint`, any case; the default size when not given.
    pub portion: Option<String>,
    #[graphql(default = 1)]
    pub quantity: i32,
    /// Options as the user said them ("chips", "mushy peas", "lemonade", "neat") or by id; each lands in the group
    /// it answers. A required option left out is refused with the choices listed: ask the user, don't guess.
    pub choices: Option<Vec<String>>,
    /// Options by group, for when a word could answer two groups.
    pub options: Option<Vec<OptionPickInput>>,
    /// For a meal that includes a drink (see `deals`), the drink, from `includedDrinks`. The till takes the deal off.
    pub include_drink: Option<IncludedDrinkInput>,
}

/// Picks for one of a product's option groups.
#[derive(InputObject, Clone, Debug)]
pub struct OptionPickInput {
    /// The group's id, or words from its name ("side", "mixer").
    pub group: Option<String>,
    /// Option ids or names. A group takes one.
    pub options: Vec<String>,
}

/// The drink that comes with a meal.
#[derive(InputObject, Clone, Debug)]
pub struct IncludedDrinkInput {
    pub product_id: ID,
    pub portion: Option<String>,
    /// Its own options, such as a spirit's mixer.
    pub choices: Option<Vec<String>>,
    pub options: Option<Vec<OptionPickInput>>,
}

/// A basket change: whether it took, and the basket either way.
#[derive(SimpleObject)]
#[graphql(complex)]
pub struct BasketResult {
    pub success: bool,
    pub error: Option<String>,
    #[graphql(skip)]
    pub basket: SessionBasket,
}

#[ComplexObject]
impl BasketResult {
    /// The basket after the change, or as it was when the change was refused.
    async fn basket(&self) -> Basket {
        Basket {
            basket: self.basket.clone(),
        }
    }
}

/// A quote, or why there isn't one.
#[derive(SimpleObject)]
pub struct QuoteResult {
    pub success: bool,
    pub error: Option<String>,
    pub quote: Option<Quote>,
}

/// What `pay` previewed or did.
#[derive(SimpleObject)]
pub struct PayResult {
    pub success: bool,
    pub action: ConfirmAction,
    /// PREVIEW: what CONFIRM will do. Show it to the user.
    pub preview: Option<String>,
    /// PREVIEW: what CONFIRM takes.
    pub confirmation_token: Option<String>,
    pub quote: Option<Quote>,
    /// CONFIRM: where the payment is.
    pub status: Option<OrderStatus>,
    /// CONFIRM: the page where the user enters their card, when there's no saved card.
    pub payment_page: Option<String>,
    pub opened_in_browser: bool,
    /// CONFIRM: how entering the saved card went.
    pub card: Option<String>,
    /// CONFIRM: a question for the user, such as whether a repeat order is wanted. Ask it; do nothing else.
    pub question: Option<String>,
    /// What to do next.
    pub next: Option<String>,
    pub error: Option<String>,
}

/// What `cancelOrder` or `sendDuplicate` previewed or did.
#[derive(SimpleObject)]
pub struct OrderActionResult {
    pub success: bool,
    pub action: ConfirmAction,
    pub preview: Option<String>,
    pub confirmation_token: Option<String>,
    pub order: Option<Order>,
    pub error: Option<String>,
}
