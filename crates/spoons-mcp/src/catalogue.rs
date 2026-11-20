//! A pub's menus as somebody at the bar reads them: every product once, sorted into the kinds of thing people ask
//! for, with the strength, units, calories, colour and taste notes the API only has as prose.
//!
//! The API's own categories are the pub's menu layout ("Lager, beer, stout and craft | Draught", "2 for £6.60",
//! "Includes a drink"), which mixes kinds together and lists the same product under several of them. A guest ale
//! is a product named "Brewery - Beer" under "Real ale", which no search for "ale" finds. So kinds are worked out
//! here from the category, the group within it and the product itself, and everything is parsed best-effort: a
//! field that can't be read is left empty rather than guessed.

use std::sync::LazyLock;

use regex::Regex;
use spoons_api_client::{Ale, BasketLine, Item, Menu, Product};

/// What a product is, as people ask for it.
#[derive(async_graphql::Enum, Copy, Clone, Debug, Eq, PartialEq, Hash, serde::Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Kind {
    /// Real ale on hand pump, the pub's own and the guests.
    CaskAle,
    Lager,
    /// Keg craft beer and bottled world beers.
    CraftAndWorld,
    Stout,
    Cider,
    /// Wine, prosecco, sparkling and champagne.
    Wine,
    Spirits,
    /// Cocktails, spritzes, premixed drinks, bombs and shots.
    Cocktails,
    /// Alcohol-free and low-alcohol drinks (0.5% ABV or less).
    AlcoholFree,
    /// Soft drinks, mixers and children's drinks.
    Soft,
    /// Tea, coffee and hot chocolate.
    Hot,
    /// Meals, sides, desserts and bar snacks.
    Food,
    /// Charity donations, pin badges and anything else.
    Other,
}

impl Kind {
    pub const ALL: [Kind; 13] = [
        Kind::CaskAle,
        Kind::Lager,
        Kind::CraftAndWorld,
        Kind::Stout,
        Kind::Cider,
        Kind::Wine,
        Kind::Spirits,
        Kind::Cocktails,
        Kind::AlcoholFree,
        Kind::Soft,
        Kind::Hot,
        Kind::Food,
        Kind::Other,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Kind::CaskAle => "Cask ales",
            Kind::Lager => "Lager",
            Kind::CraftAndWorld => "Craft & world beer",
            Kind::Stout => "Stout",
            Kind::Cider => "Cider",
            Kind::Wine => "Wine & fizz",
            Kind::Spirits => "Spirits",
            Kind::Cocktails => "Cocktails",
            Kind::AlcoholFree => "Alcohol-free",
            Kind::Soft => "Soft drinks",
            Kind::Hot => "Hot drinks",
            Kind::Food => "Food",
            Kind::Other => "Other",
        }
    }

    pub fn is_drink(self) -> bool {
        !matches!(self, Kind::Food | Kind::Other)
    }
}

/// Where a colour came from, best first.
#[derive(async_graphql::Enum, Copy, Clone, Debug, Eq, PartialEq, serde::Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ColourSource {
    /// The pub's ale list gives the beer's style and its colour.
    AleList,
    /// Read from the description, such as "Mahogany coloured".
    Description,
    /// Typical of the kind of drink, such as a stout's black.
    Kind,
}

#[derive(Clone, Debug, serde::Serialize)]
pub struct Colour {
    pub hex: String,
    pub label: String,
    pub source: ColourSource,
}

/// One size a product comes in.
#[derive(Clone, Debug, serde::Serialize)]
pub struct Size {
    /// As the menu labels it, which the till matches on: `250ml glass (185 kcal, 3.0 units)`.
    pub label: String,
    /// Without the calories and units some labels carry: `250ml glass`.
    pub name: String,
    pub price: f64,
    pub is_default: bool,
    pub millilitres: Option<f64>,
    pub units: Option<f64>,
    pub kcal: Option<f64>,
}

/// A product as listed, with what could be read from it.
#[derive(Clone, Debug)]
pub struct Entry {
    pub product: Product,
    /// The category the till is told it came from: never a meal deal's when the product is listed elsewhere too,
    /// since a deal's "includes a drink" price is not the drink's own.
    pub category_id: u64,
    /// The pub's own name for that category, and the group within it.
    pub category: String,
    pub group: Option<String>,
    pub kind: Kind,
    /// What else it is, for an alcohol-free drink: a lager, a stout, a cider. Searching "alcohol free lager" needs
    /// both.
    pub also: Option<Kind>,
    pub guest: bool,
    pub brewery: Option<String>,
    /// The name without the brewery, for a guest ale.
    pub short_name: String,
    pub abv: Option<f64>,
    pub units: Option<f64>,
    pub kcal: Option<f64>,
    pub notes: Vec<String>,
    pub origin: Option<String>,
    pub colour: Option<Colour>,
    pub style: Option<String>,
    pub sizes: Vec<Size>,
    /// Why it can't be ordered here, if it can't.
    pub why_not: Option<String>,
    /// Lower comes first: the pub's own order.
    pub position: usize,
}

impl Entry {
    pub fn orderable(&self) -> bool {
        self.why_not.is_none()
    }

    pub fn default_size(&self) -> Option<&Size> {
        self.sizes
            .iter()
            .find(|size| size.is_default)
            .or(self.sizes.first())
    }
}

/// One sales area's whole menu.
#[derive(Clone, Debug, Default)]
pub struct Catalogue {
    pub entries: Vec<Entry>,
    pub coming_soon: Vec<Ale>,
}

impl Catalogue {
    /// From the orderable menus of one sales area, in the order the app lists them, and the pub's ale list (which
    /// carries each real ale's style and colour) when it could be read.
    pub fn build(menus: &[Menu], ales: &[Ale]) -> Self {
        let mut catalogue = Catalogue::default();
        let mut position = 0;
        for menu in menus {
            for category in menu.categories.iter().filter(|c| !c.hidden) {
                let deal = is_deal(&category.name);
                for group in &category.item_groups {
                    for item in &group.items {
                        match item {
                            Item::Product(product) => {
                                if let Some(existing) = catalogue
                                    .entries
                                    .iter_mut()
                                    .find(|e| e.product.id == product.id)
                                {
                                    // First listing wins, unless that was a deal and this is the product's own.
                                    if is_deal(&existing.category) && !deal {
                                        let better = entry(
                                            &menu.name,
                                            category.id,
                                            &category.name,
                                            group.name.as_deref(),
                                            product,
                                            ales,
                                            existing.position,
                                        );
                                        *existing = better;
                                    }
                                    continue;
                                }
                                catalogue.entries.push(entry(
                                    &menu.name,
                                    category.id,
                                    &category.name,
                                    group.name.as_deref(),
                                    product,
                                    ales,
                                    position,
                                ));
                                position += 1;
                            }
                            Item::Ale(ale)
                                if ale.coming_soon
                                    && !catalogue.coming_soon.iter().any(|a| a.id == ale.id) =>
                            {
                                catalogue.coming_soon.push((**ale).clone());
                            }
                            _ => {}
                        }
                    }
                }
            }
        }
        for ale in ales.iter().filter(|a| a.coming_soon) {
            if !catalogue.coming_soon.iter().any(|a| a.id == ale.id) {
                catalogue.coming_soon.push(ale.clone());
            }
        }
        catalogue
    }

    pub fn get(&self, product_id: u64) -> Option<&Entry> {
        self.entries.iter().find(|e| e.product.id == product_id)
    }
}

/// Lower-cased, without apostrophes and the accents people don't type, so "roosters" finds "Rooster’s".
pub fn fold(text: &str) -> String {
    text.to_lowercase()
        .replace(['’', '‘', '`', '\''], "")
        .replace(['é', 'è', 'ê'], "e")
        .replace('ï', "i")
}

fn is_deal(category: &str) -> bool {
    static DEAL: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?i)includes a drink|\d+ for £|deals?\b|any \d+ for").expect("valid pattern")
    });
    DEAL.is_match(category)
}

static ABV: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)(\d+(?:\.\d+)?)\s*%\s*(?:ABV|vol)").expect("valid pattern"));
static UNITS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)(\d+(?:\.\d+)?)\s*units?\b").expect("valid pattern"));
static KCAL: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)(\d+(?:\.\d+)?)\s*kcal").expect("valid pattern"));
static ML: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)(\d+(?:\.\d+)?)\s*(ml|cl|l)\b").expect("valid pattern"));
static COLOURED: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)([a-z]+(?:[ /-][a-z]+)?)\s+colou?red").expect("valid pattern")
});

fn number(re: &Regex, text: &str) -> Option<f64> {
    re.captures(text)?.get(1)?.as_str().parse().ok()
}

fn entry(
    menu: &str,
    category_id: u64,
    category: &str,
    group: Option<&str>,
    product: &Product,
    ales: &[Ale],
    position: usize,
) -> Entry {
    let description = product.description.clone().unwrap_or_default();
    let abv = number(&ABV, &description).or_else(|| number(&ABV, &product.name));
    let kind = classify(menu, category, group, product, abv);
    let listed = ales.iter().find(|ale| {
        ale.full_name
            .as_deref()
            .is_some_and(|n| fold(n) == fold(&product.name))
    });
    let (brewery, short_name) = match (listed, product.name.split_once(" - ")) {
        (Some(ale), _) => (ale.brewery.clone(), ale.name.clone()),
        (None, Some((brewery, name))) if kind == Kind::CaskAle => {
            (Some(brewery.trim().to_owned()), name.trim().to_owned())
        }
        _ => (None, product.name.clone()),
    };
    let also = (kind == Kind::AlcoholFree)
        .then(|| alcohol_free_also(&product.name))
        .flatten();
    let guest = kind == Kind::CaskAle && (listed.is_some() || product.name.contains(" - "));
    let units = number(&UNITS, &description).or_else(|| listed.and_then(|a| a.units));
    let kcal = product.calories.or_else(|| number(&KCAL, &description));
    let abv = abv.or_else(|| listed.and_then(|a| a.abv));
    let (notes, origin) = notes(&description, kind);
    // Wines carry their colour as a keyword (`WO`: red, white, rose, spark), which beats guessing from the group.
    let wine = product
        .keywords
        .iter()
        .find(|k| k.kind == "WO")
        .and_then(|k| k.value.as_str())
        .unwrap_or("");
    let colour = colour(
        listed,
        &description,
        kind,
        Some(&format!("{} {wine}", group.unwrap_or(""))),
        &product.name,
    );
    let sizes = sizes(product, abv);
    let why_not = BasketLine::new(0, product, None, 1)
        .err()
        .map(|error| error.to_string());
    Entry {
        product: product.clone(),
        category_id,
        category: category.trim().to_owned(),
        group: group.map(|g| g.trim().to_owned()).filter(|g| !g.is_empty()),
        kind,
        also,
        guest,
        brewery,
        short_name,
        abv,
        units,
        kcal,
        notes,
        origin,
        colour,
        style: listed
            .and_then(|a| a.style.as_ref())
            .map(|s| s.label.clone()),
        sizes,
        why_not,
        position,
    }
}

/// The kind of thing a product is. The group within a category is the most specific clue, then the category,
/// then the product's own name; a product on a food menu that nothing marks as a drink is food.
pub fn classify(
    menu: &str,
    category: &str,
    group: Option<&str>,
    product: &Product,
    abv: Option<f64>,
) -> Kind {
    let name = fold(&product.name);
    let alcohol_free = name.contains("alcohol free")
        || name.contains("alcohol-free")
        || name.contains("low alcohol")
        || name.contains(" 0.0")
        || name.contains(" 0%");
    let category = fold(category);
    if alcohol_free
        || category.contains("alcohol free")
        || category.starts_with("0%")
        || abv.is_some_and(|abv| abv <= 0.5)
    {
        return Kind::AlcoholFree;
    }
    if category.contains("real ale") {
        return Kind::CaskAle;
    }
    if let Some(kind) = group.map(fold).as_deref().and_then(by_words) {
        return kind;
    }
    if let Some(kind) = by_words(&category) {
        return kind;
    }
    if name.contains("guinness") || name.contains("stout") {
        return Kind::Stout;
    }
    if name.contains("cider") {
        return Kind::Cider;
    }
    let menu = fold(menu);
    if [
        "food",
        "breakfast",
        "children",
        "deal",
        "christmas",
        "festive",
        "lunch",
        "dinner",
    ]
    .iter()
    .any(|word| menu.contains(word))
    {
        return Kind::Food;
    }
    Kind::Other
}

fn by_words(text: &str) -> Option<Kind> {
    let has = |words: &[&str]| words.iter().any(|word| text.contains(word));
    // Order matters: "Lager, beer, stout and craft" is a lager category with stout groups inside it.
    if has(&["real ale", "cask"]) {
        Some(Kind::CaskAle)
    } else if has(&["cider"]) {
        Some(Kind::Cider)
    } else if has(&["lager", "pints"]) {
        Some(Kind::Lager)
    } else if has(&["stout", "porter"]) {
        Some(Kind::Stout)
    } else if has(&["craft", "world beer", "beer"]) {
        Some(Kind::CraftAndWorld)
    } else if has(&["wine", "prosecco", "champagne", "sparkling", "fizz"]) {
        Some(Kind::Wine)
    } else if has(&[
        "cocktail",
        "spritz",
        "premixed",
        "bomb",
        "shot",
        "buzzball",
        "long drink",
    ]) {
        Some(Kind::Cocktails)
    } else if has(&[
        "vodka", "gin", "rum", "whisk", "tequila", "liqueur", "cognac", "brandy", "spirit",
    ]) {
        Some(Kind::Spirits)
    } else if has(&["soft drink", "childrens drinks", "mixer", "juice"]) {
        Some(Kind::Soft)
    } else if has(&["hot drink", "tea", "coffee", "hot chocolate"]) {
        Some(Kind::Hot)
    } else if has(&["snack", "crisps", "peanuts", "biscuit", "dessert", "side"]) {
        Some(Kind::Food)
    } else if has(&["charity", "pin", "sock", "hat", "top trumps", "donation"]) {
        Some(Kind::Other)
    } else {
        None
    }
}

/// What an alcohol-free drink would be with the alcohol, from its name.
fn alcohol_free_also(name: &str) -> Option<Kind> {
    let name = fold(name);
    let has = |words: &[&str]| words.iter().any(|w| name.contains(w));
    if has(&["guinness", "stout"]) {
        Some(Kind::Stout)
    } else if has(&["kopparberg", "cider", "rekorderlig", "thatchers"]) {
        Some(Kind::Cider)
    } else if has(&["punk", "ipa", "pale ale"]) {
        Some(Kind::CraftAndWorld)
    } else if has(&[
        "beck",
        "corona",
        "heineken",
        "stella",
        "peroni",
        "madri",
        "erdinger",
        "lager",
        "san miguel",
        "bud",
    ]) {
        Some(Kind::Lager)
    } else if has(&["gin", "gordon", "tanqueray"]) {
        Some(Kind::Spirits)
    } else {
        None
    }
}

/// Taste notes, and for wine where it comes from, from a description such as "4.2% ABV. 2.4 units. Black
/// coloured, roasted and chocolaty" or "Valle Central, Chile\n12% ABV\nJuicy and smooth".
fn notes(description: &str, kind: Kind) -> (Vec<String>, Option<String>) {
    static STRIP: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(
            r"(?i)\d+(?:\.\d+)?\s*%\s*(?:abv|vol)|\d+(?:\.\d+)?\s*units?|\d+(?:\.\d+)?\s*kcal|\d+(?:\.\d+)?\s*(?:ml|cl)(?:\s*(?:bottle|can|glass))?|[a-z' ]+ brewery\.",
        )
        .expect("valid pattern")
    });
    let lines: Vec<&str> = description
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    let origin = (kind == Kind::Wine)
        .then(|| lines.first())
        .flatten()
        .map(|first| {
            STRIP
                .replace_all(first, "")
                .trim()
                .trim_end_matches([',', '.'])
                .trim()
                .to_owned()
        })
        .filter(|o| !o.is_empty() && o.len() < 60);
    // A one-line wine description is only where it's from.
    if kind == Kind::Wine && lines.len() < 2 {
        return (Vec::new(), origin);
    }
    if !kind.is_drink() || matches!(kind, Kind::Soft | Kind::Hot) {
        return (Vec::new(), None);
    }
    let Some(last) = lines.last() else {
        return (Vec::new(), origin);
    };
    let text = STRIP.replace_all(last, " ");
    let text = COLOURED.replace_all(&text, " ");
    let notes = text
        .split([',', '.', ';'])
        .flat_map(|part| part.split(" and "))
        .map(|note| note.trim().trim_matches(['-', '–']).trim().to_lowercase())
        .filter(|note| {
            !note.is_empty()
                && note.len() < 32
                && note.split_whitespace().count() <= 3
                && !note.chars().any(|c| c.is_ascii_digit())
                && origin
                    .as_deref()
                    .is_none_or(|o| !fold(o).contains(note.as_str()))
        })
        .collect();
    (notes, origin)
}

fn colour(
    listed: Option<&Ale>,
    description: &str,
    kind: Kind,
    group: Option<&str>,
    name: &str,
) -> Option<Colour> {
    if let Some(style) = listed.and_then(|ale| ale.style.as_ref())
        && let Some(hex) = &style.hex_color
    {
        return Some(Colour {
            hex: hex.to_lowercase(),
            label: style.label.clone(),
            source: ColourSource::AleList,
        });
    }
    if let Some(found) = COLOURED.captures(description).and_then(|c| c.get(1)) {
        let words = fold(found.as_str());
        // The darkest word named wins: "deep ruby" is ruby, "golden amber" amber.
        let hex = SHADES
            .iter()
            .filter(|(word, _)| words.contains(word))
            .max_by_key(|(_, hex)| darkness(hex))
            .map(|(_, hex)| *hex);
        if let Some(hex) = hex {
            let mut label = found.as_str().trim().to_owned();
            if let Some(first) = label.get(..1) {
                label = first.to_uppercase() + &label[1..];
            }
            return Some(Colour {
                hex: hex.to_owned(),
                label,
                source: ColourSource::Description,
            });
        }
    }
    let group = group.map(fold).unwrap_or_default();
    let name = fold(name);
    let (hex, label) = match kind {
        Kind::Stout => ("#120a06", "Black"),
        Kind::Lager => ("#e3b440", "Gold"),
        Kind::CraftAndWorld => ("#d99a2b", "Golden"),
        Kind::Cider => ("#e8b84a", "Golden"),
        Kind::CaskAle => ("#b8732e", "Amber"),
        Kind::Wine if group.contains("red") => ("#6e1423", "Red"),
        Kind::Wine
            if group.contains("spark")
                || group.contains("prosecco")
                || group.contains("champagne") =>
        {
            ("#f3e6a8", "Sparkling")
        }
        Kind::Wine if group.contains("ros") || name.contains("rosé") || name.contains("rose") => {
            ("#f2a7a0", "Rosé")
        }
        Kind::Wine => ("#efe3a6", "Pale straw"),
        Kind::AlcoholFree if name.contains("guinness") => ("#120a06", "Black"),
        Kind::AlcoholFree
            if [
                "beck", "corona", "erdinger", "heineken", "stella", "punk", "peroni", "madri",
            ]
            .iter()
            .any(|beer| name.contains(beer)) =>
        {
            ("#e3b440", "Gold")
        }
        _ => return None,
    };
    Some(Colour {
        hex: hex.to_owned(),
        label: label.to_owned(),
        source: ColourSource::Kind,
    })
}

/// Words a description uses for a beer's colour, and a colour for each.
const SHADES: &[(&str, &str)] = &[
    ("straw", "#ecd98a"),
    ("pale", "#ece2ac"),
    ("blond", "#e6c76a"),
    ("light", "#ecd98a"),
    ("gold", "#d9a227"),
    ("yellow", "#e3c04a"),
    ("tawny", "#b8732e"),
    ("amber", "#c47a2c"),
    ("copper", "#a0522d"),
    ("red", "#8c2b17"),
    ("chestnut", "#7b3f1d"),
    ("ruby", "#5c0f12"),
    ("brown", "#5a2d16"),
    ("mahogany", "#4a1c12"),
    ("dark", "#1d0f0a"),
    ("black", "#0b0705"),
];

fn darkness(hex: &str) -> u32 {
    let channel = |range: std::ops::Range<usize>| {
        u32::from_str_radix(hex.get(range).unwrap_or("ff"), 16).unwrap_or(255)
    };
    765 - (channel(1..3) + channel(3..5) + channel(5..7))
}

fn sizes(product: &Product, abv: Option<f64>) -> Vec<Size> {
    static PAREN: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"\s*\([^)]*\)").expect("valid pattern"));
    product
        .options
        .portion
        .iter()
        .flat_map(|portion| &portion.options)
        .filter(|option| !option.value.hidden)
        .map(|option| {
            let label = option.label.clone();
            let name = PAREN.replace_all(&label, "").trim().to_owned();
            let millilitres = millilitres(&name);
            let units = number(&UNITS, &label).or_else(|| {
                abv.zip(millilitres)
                    .map(|(abv, ml)| (abv * ml / 100.0).round() / 10.0)
            });
            let kcal = number(&KCAL, &label);
            Size {
                label,
                name,
                price: option.value.price.value,
                is_default: option.is_default,
                millilitres,
                units,
                kcal,
            }
        })
        .collect()
}

fn millilitres(name: &str) -> Option<f64> {
    let lower = fold(name);
    if let Some(captures) = ML.captures(&lower) {
        let value: f64 = captures[1].parse().ok()?;
        return Some(match &captures[2] {
            "cl" => value * 10.0,
            "l" => value * 1000.0,
            _ => value,
        });
    }
    Some(match lower.as_str() {
        "pint" => 568.0,
        "half pint" | "half" => 284.0,
        "third pint" | "third" => 189.0,
        "two-thirds pint" | "two thirds" => 379.0,
        "single" => 25.0,
        "double" => 50.0,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn product(name: &str, description: &str, portions: serde_json::Value) -> Product {
        serde_json::from_value(json!({
            "id": 1, "name": name, "description": description,
            "options": { "portion": { "options": portions } },
        }))
        .unwrap()
    }

    fn pints() -> serde_json::Value {
        json!([
            { "label": "Third pint", "isDefault": false, "value": { "id": 3, "price": { "value": 1.13 } } },
            { "label": "Half pint", "isDefault": false, "value": { "id": 2, "price": { "value": 1.71 } } },
            { "label": "Pint", "isDefault": true, "value": { "id": 1, "price": { "value": 3.41 } } },
        ])
    }

    #[test]
    fn finds_a_guest_ale_without_ale_in_its_name() {
        let scoundrel = product(
            "Rooster’s - Scoundrel",
            "4.2% ABV.  2.4 units. Black coloured, roasted and chocolaty",
            pints(),
        );
        let e = entry("Drinks", 400255, "Real ale", None, &scoundrel, &[], 0);
        assert_eq!(e.kind, Kind::CaskAle);
        assert!(e.guest);
        assert_eq!(e.brewery.as_deref(), Some("Rooster’s"));
        assert_eq!(e.short_name, "Scoundrel");
        assert_eq!(e.abv, Some(4.2));
        assert_eq!(e.units, Some(2.4));
        assert_eq!(e.notes, vec!["roasted", "chocolaty"]);
        let colour = e.colour.clone().unwrap();
        assert_eq!(
            (colour.hex.as_str(), colour.source),
            ("#0b0705", ColourSource::Description)
        );
        assert_eq!(e.sizes[0].millilitres, Some(189.0));
        assert_eq!(e.sizes[2].units, Some(2.4));
        assert!(crate::search::product_matches("roosters", &e));
        assert!(crate::search::product_matches("ale", &e));
    }

    #[test]
    fn takes_an_ales_colour_from_the_ale_list() {
        let ale: Ale = serde_json::from_value(json!({
            "id": 403, "brewery": "George’s", "name": "Broadsword", "fullName": "George’s - Broadsword",
            "type": { "key": "brown", "label": "Dark brown/ruby", "hexColor": "#63130E" },
            "abv": 4.7, "units": 2.7, "comingSoon": false,
        }))
        .unwrap();
        let broadsword = product(
            "George’s - Broadsword",
            "4.7% ABV.  2.7 units. Mahogany coloured, traditional and balanced",
            pints(),
        );
        let e = entry("Drinks", 1, "Real ale", None, &broadsword, &[ale], 0);
        let colour = e.colour.clone().unwrap();
        assert_eq!(colour.hex, "#63130e");
        assert_eq!(colour.source, ColourSource::AleList);
        assert_eq!(e.style.as_deref(), Some("Dark brown/ruby"));
    }

    #[test]
    fn keeps_the_pubs_own_ales_apart_from_guests() {
        let abbot = product(
            "Greene King Abbot Ale",
            "5.0% ABV, 2.8 units Greene King Brewery. Rich amber coloured, malty and bittersweet.",
            pints(),
        );
        let e = entry("Drinks", 1, "Real ale", None, &abbot, &[], 0);
        assert_eq!(e.kind, Kind::CaskAle);
        assert!(!e.guest);
        assert_eq!(e.notes, vec!["malty", "bittersweet"]);
        assert_eq!(e.colour.unwrap().label, "Rich amber");
    }

    #[test]
    fn sorts_the_draught_category_by_its_groups() {
        let p = product("Guinness", "4.2% ABV, 2.4 units", pints());
        let cat = "Lager, beer, stout and craft | Draught";
        assert_eq!(
            classify("Drinks", cat, Some("Stout"), &p, Some(4.2)),
            Kind::Stout
        );
        assert_eq!(
            classify("Drinks", cat, Some("Lager"), &p, Some(4.2)),
            Kind::Lager
        );
        assert_eq!(classify("Drinks", cat, None, &p, Some(4.2)), Kind::Lager);
        assert_eq!(
            classify("Drinks", cat, Some("Craft"), &p, Some(4.2)),
            Kind::CraftAndWorld
        );
    }

    #[test]
    fn puts_alcohol_free_first() {
        let p = product("Guinness 0.0 - Alcohol free", "0.05% ABV 538ml", json!([]));
        assert_eq!(
            classify("Drinks", "Low and alcohol free", None, &p, Some(0.05)),
            Kind::AlcoholFree
        );
        let gin = product(
            "Gordons Premium Pink 0.0%",
            "0.0% ABV, 25ml, 0.0 units",
            json!([]),
        );
        assert_eq!(
            classify("Drinks", "Gin", None, &gin, Some(0.0)),
            Kind::AlcoholFree
        );
    }

    #[test]
    fn reads_a_wines_origin_notes_and_glass_sizes() {
        let merlot = product(
            "Coldwater Creek Merlot by Concha y Toro",
            "Valle Central, Chile\n12% ABV\nFruity, warm and velvety, with a touch of spice",
            json!([
                { "label": "125ml glass (93 kcal, 1.5 units)", "isDefault": false, "value": { "id": 1, "price": { "value": 2.35 } } },
                { "label": "250ml glass (185 kcal, 3.0 units)", "isDefault": true, "value": { "id": 2, "price": { "value": 4.35 } } },
            ]),
        );
        let e = entry(
            "Drinks",
            1,
            "Wine, prosecco & sparkling",
            Some("Red wine"),
            &merlot,
            &[],
            0,
        );
        assert_eq!(e.kind, Kind::Wine);
        assert_eq!(e.origin.as_deref(), Some("Valle Central, Chile"));
        assert_eq!(e.notes, vec!["fruity", "warm", "velvety"]);
        assert_eq!(e.sizes[1].name, "250ml glass");
        assert_eq!(e.sizes[1].kcal, Some(185.0));
        assert_eq!(e.sizes[1].units, Some(3.0));
        assert_eq!(e.colour.unwrap().label, "Red");
    }

    #[test]
    fn prefers_a_products_own_category_to_a_deal() {
        let menu: Menu = serde_json::from_value(json!({
            "id": 1, "name": "Drinks", "categories": [
                { "id": 9, "name": "Includes a drink", "itemGroups": [{ "items": [
                    { "itemType": "product", "id": 5, "name": "Carling", "description": "4.0% ABV" }
                ] }] },
                { "id": 7, "name": "Lager, beer, stout and craft | Draught", "itemGroups": [{ "name": "Lager", "items": [
                    { "itemType": "product", "id": 5, "name": "Carling", "description": "4.0% ABV" }
                ] }] },
            ],
        }))
        .unwrap();
        let catalogue = Catalogue::build(&[menu], &[]);
        assert_eq!(catalogue.entries.len(), 1);
        assert_eq!(catalogue.entries[0].category_id, 7);
        assert_eq!(catalogue.entries[0].kind, Kind::Lager);
    }
}
