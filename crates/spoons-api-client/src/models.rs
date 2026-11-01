//! What the API sends, as far as it's been read. Fields not modelled here are still in [`crate::Client::raw`]'s
//! bodies; anything uncertain is optional, since nothing about this API is promised.

use serde::{Deserialize, Serialize};
use serde_json::Value;

fn jdw() -> String {
    crate::client::FRANCHISE.to_owned()
}

/// A pub in the venue list.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VenueSummary {
    /// Which operator's routes the pub is under: `jdw` for Wetherspoon's own, or a holiday park or university.
    #[serde(default = "jdw")]
    pub franchise: String,
    /// Internal; routes take `venue_ref`.
    #[serde(deserialize_with = "crate::de::id")]
    pub id: u64,
    #[serde(deserialize_with = "crate::de::id")]
    pub venue_ref: u64,
    pub name: String,
    /// `open`, `closed_temporary`, `closing_temporary`, `opening_soon`.
    pub status: String,
    /// `pub` or `pub_hotel`.
    #[serde(rename = "type")]
    pub kind: String,
    pub address: Address,
    #[serde(default)]
    pub is_closed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Address {
    pub line1: Option<String>,
    pub line2: Option<String>,
    pub line3: Option<String>,
    pub town: Option<String>,
    pub county: Option<String>,
    pub postcode: Option<String>,
    pub country: Country,
    pub location: Option<Location>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Country {
    pub name: String,
    pub code: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Location {
    pub latitude: f64,
    pub longitude: f64,
}

/// One pub in full.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Venue {
    #[serde(default = "jdw")]
    pub franchise: String,
    /// `airport`, `haven`, `concession`, or nothing for a high-street pub.
    pub sub_type: Option<String>,
    #[serde(deserialize_with = "crate::de::id")]
    pub id: u64,
    #[serde(deserialize_with = "crate::de::id")]
    pub venue_ref: u64,
    pub name: String,
    pub status: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub address: Address,
    #[serde(default)]
    pub facilities: Vec<String>,
    pub contact_details: Option<ContactDetails>,
    pub currency: Option<Currency>,
    /// By weekday (`days`) and by date (`dates`), each `open`, `close` and `isClosed`.
    pub opening_times: Option<Value>,
    #[serde(default)]
    pub sales_areas: Vec<SalesArea>,
    pub allergens_url: Option<String>,
    #[serde(default)]
    pub display_images: Vec<String>,
    #[serde(default)]
    pub can_place_order: bool,
    #[serde(default)]
    pub is_closed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContactDetails {
    pub email: Option<String>,
    pub telephone: Option<String>,
    pub website: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Currency {
    pub code: String,
    pub symbol: Option<String>,
}

/// A part of a pub with its own menus, usually just "Bar".
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SalesArea {
    #[serde(deserialize_with = "crate::de::id")]
    pub id: u64,
    pub name: String,
}

/// A real ale, from a pub's ale list or its menu.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Ale {
    #[serde(deserialize_with = "crate::de::id")]
    pub id: u64,
    pub brewery: Option<String>,
    pub name: String,
    pub full_name: Option<String>,
    #[serde(rename = "type")]
    pub style: Option<AleStyle>,
    /// The pub's price band for it, `A` being the cheapest.
    pub price_band: Option<String>,
    pub abv: Option<f64>,
    pub units: Option<f64>,
    pub description: Option<String>,
    /// Ordered in but not yet on.
    #[serde(default)]
    pub coming_soon: bool,
    pub allergens: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AleStyle {
    pub key: String,
    pub label: String,
    pub hex_color: Option<String>,
}

/// Minutes an order is taking.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WaitTimes {
    pub food: Option<u32>,
    pub drink: Option<u32>,
    /// Local time, no zone given.
    pub updated_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MenuSummary {
    /// Usually a number, but a pub's specials menu is just `specials`.
    #[serde(deserialize_with = "crate::de::key")]
    pub id: String,
    /// `Drinks`, `Breakfast`, `Real Ales` and so on.
    pub name: String,
    /// The internal name, which says which season's menu it is: `Drinks Main JUL26`.
    pub description: Option<String>,
    pub updated: Option<String>,
    #[serde(default)]
    pub can_order: bool,
    #[serde(default)]
    pub is_hidden: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Menu {
    #[serde(deserialize_with = "crate::de::key")]
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub updated: Option<String>,
    #[serde(default)]
    pub categories: Vec<Category>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Category {
    #[serde(deserialize_with = "crate::de::id")]
    pub id: u64,
    pub name: String,
    #[serde(default)]
    pub hidden: bool,
    #[serde(default)]
    pub item_groups: Vec<ItemGroup>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ItemGroup {
    pub name: Option<String>,
    pub description: Option<String>,
    #[serde(default)]
    pub items: Vec<Item>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "itemType", rename_all = "lowercase")]
pub enum Item {
    Product(Box<Product>),
    Ale(Box<Ale>),
    Text {
        text: String,
    },
    Divider,
    #[serde(other)]
    Other,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Product {
    #[serde(deserialize_with = "crate::de::id")]
    pub id: u64,
    pub name: String,
    /// For drinks, the strength and units: `4.0% ABV, 2.3 units`.
    pub description: Option<String>,
    pub calories: Option<f64>,
    #[serde(default)]
    pub is_out_of_stock: bool,
    pub age_restriction: Option<u32>,
    pub course_id: Option<u64>,
    pub display_record_id: Option<u64>,
    /// How the till knows the product; passed back as it came when ordering.
    #[serde(default)]
    pub checkout: Value,
    #[serde(default)]
    pub options: ProductOptions,
    /// Allergens (`AL::…`) and the drink's add-ons, among other things.
    #[serde(default)]
    pub keywords: Vec<Keyword>,
}

impl Product {
    /// Each size it comes in with its price. A product with no sizes has none; the price is on its options.
    pub fn portions(&self) -> impl Iterator<Item = (&str, &Price)> {
        self.options
            .portion
            .iter()
            .flat_map(|portion| &portion.options)
            .filter(|option| !option.value.hidden)
            .map(|option| (option.label.as_str(), &option.value.price))
    }

    pub fn allergens(&self) -> impl Iterator<Item = &str> {
        self.keywords
            .iter()
            .filter(|keyword| keyword.kind == "AL")
            .filter_map(|keyword| keyword.value.as_str())
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProductOptions {
    pub portion: Option<Portion>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Portion {
    #[serde(default)]
    pub options: Vec<PortionOption>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PortionOption {
    /// `Pint`, `Half pint`, `Single`, `Double`, `250ml` and so on.
    pub label: String,
    #[serde(default)]
    pub is_default: bool,
    pub value: PortionValue,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PortionValue {
    #[serde(deserialize_with = "crate::de::id")]
    pub id: u64,
    pub price: Price,
    #[serde(default)]
    pub hidden: bool,
    /// Choices this size asks for, such as a spirit's mixer.
    #[serde(default)]
    pub choices: Vec<PortionChoice>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PortionChoice {
    #[serde(deserialize_with = "crate::de::id")]
    pub choice_id: u64,
    #[serde(default)]
    pub required: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Price {
    pub value: f64,
    pub initial_value: Option<f64>,
    pub currency: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Keyword {
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(default)]
    pub value: Value,
    /// Flags the app reads, such as `isVegan`; an empty list when there are none.
    #[serde(default)]
    pub tags: Value,
}

/// A table that can be ordered to.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Table {
    pub number: u32,
    pub name: Option<String>,
    #[serde(default)]
    pub available: bool,
    #[serde(deserialize_with = "crate::de::id")]
    pub sales_area_id: u64,
}

/// A basket as the pub's till priced it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BasketValidation {
    pub id: String,
    pub total: Price,
    #[serde(default)]
    pub items: Vec<ValidatedLine>,
    /// Signed by the API over the basket and its total; preparing a payment needs it.
    pub token: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidatedLine {
    #[serde(deserialize_with = "crate::de::id")]
    pub id: u64,
    pub quantity: u32,
    pub price: Price,
}
