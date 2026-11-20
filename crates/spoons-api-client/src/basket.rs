//! Basket lines in the shape the app sends to `basket/validate`, built from a menu's products.
//!
//! The shape is the app's own (`uCt` in its bundle): the product as the menu listed it, the size with the choice
//! groups that size asks for, and one option for each group, keyed by the group's id, each option sent as a product
//! of its own under the parent's category, with the group's id in its checkout. A size whose required choices
//! aren't all made is refused here with the options listed, rather than sent: the till only says "unavailable".

use serde::Serialize;
use serde_json::{Map, Value, json};

use crate::models::{ChoiceGroup, PortionOption, Product};

#[derive(Debug, thiserror::Error)]
pub enum LineError {
    #[error("{0} is out of stock")]
    OutOfStock(String),
    #[error("{product} doesn't come as {portion}; it comes as {available}")]
    NoSuchPortion {
        product: String,
        portion: String,
        available: String,
    },
    #[error("a quantity of nothing")]
    NoQuantity,
    #[error("{product} needs a choice for “{group}”: {options}")]
    MissingChoice {
        product: String,
        group: String,
        options: String,
    },
    #[error("{product} has no option {option} for “{group}”; the options are {options}")]
    NoSuchChoice {
        product: String,
        group: String,
        option: u64,
        options: String,
    },
    #[error("{product} doesn't ask “{group}” for this size")]
    NoSuchGroup { product: String, group: String },
    #[error("{option} is out of stock, for {product}")]
    ChoiceOutOfStock { product: String, option: String },
}

/// One product in one size, with its choices made.
#[derive(Debug, Clone)]
pub struct BasketLine {
    body: Value,
    /// The size chosen, as the menu labels it.
    pub portion: String,
    /// The options chosen, by name, in the order the menu asks.
    pub choices: Vec<String>,
}

impl Serialize for BasketLine {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.body.serialize(serializer)
    }
}

/// A size's label, matched without regard to case, or the default size when `None`.
pub fn find_portion<'a>(product: &'a Product, portion: Option<&str>) -> Result<&'a PortionOption, LineError> {
    let options = product
        .options
        .portion
        .iter()
        .flat_map(|portion| &portion.options)
        .filter(|option| !option.value.hidden);
    let chosen = match portion {
        Some(wanted) => options
            .clone()
            .find(|option| option.label.eq_ignore_ascii_case(wanted)),
        None => options
            .clone()
            .find(|option| option.is_default)
            .or_else(|| options.clone().next()),
    };
    chosen.ok_or_else(|| LineError::NoSuchPortion {
        product: product.name.clone(),
        portion: portion.unwrap_or("any size").to_owned(),
        available: options
            .map(|option| option.label.as_str())
            .collect::<Vec<_>>()
            .join(", "),
    })
}

/// The choice groups a size asks for, in the menu's order, with whether each is required.
pub fn groups_for<'a>(product: &'a Product, portion: &PortionOption) -> Vec<(&'a ChoiceGroup, bool)> {
    product
        .options
        .choices
        .iter()
        .filter_map(|group| {
            let asked = portion.value.choices.iter().find(|c| c.choice_id == group.id)?;
            Some((group, asked.required || group.required))
        })
        .collect()
}

/// An option's names and prices, for telling someone what they can pick.
pub fn describe(group: &ChoiceGroup) -> String {
    group
        .options
        .iter()
        .filter(|o| !o.is_out_of_stock)
        .map(|o| match option_price(o) {
            Some(price) if price > 0.0 => format!("{} (+{price:.2})", o.name),
            _ => o.name.clone(),
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// What an option adds to the price: its default size's price.
pub fn option_price(option: &Product) -> Option<f64> {
    find_portion(option, None).ok().map(|p| p.value.price.value)
}

/// The product as the till knows it, under `category`, in the size `portion_id`.
fn till_product(category: u64, product: &Product, portion_id: u64) -> Value {
    let mut keyword_tags = Map::new();
    for keyword in &product.keywords {
        if let Value::Object(tags) = &keyword.tags {
            keyword_tags.extend(tags.clone());
        }
    }
    let mut checkout = match &product.checkout {
        Value::Object(checkout) => checkout.clone(),
        _ => Map::new(),
    };
    checkout.insert("displayRecordId".into(), json!(product.display_record_id));
    json!({
        "id": product.id,
        "name": product.name,
        "category": { "id": category },
        "course": { "id": product.course_id },
        "portion": { "id": portion_id },
        "keywordTags": keyword_tags,
        "checkout": checkout,
    })
}

impl BasketLine {
    /// `category` is the id of the menu category the product was listed under (a deal's, for a meal's included
    /// drink); `portion` is a size's label or the default; `picks` are (group id, option id), one per group.
    pub fn new(
        category: u64,
        product: &Product,
        portion: Option<&str>,
        quantity: u32,
        picks: &[(u64, u64)],
    ) -> Result<Self, LineError> {
        if quantity == 0 {
            return Err(LineError::NoQuantity);
        }
        if product.is_out_of_stock {
            return Err(LineError::OutOfStock(product.name.clone()));
        }
        let chosen = find_portion(product, portion)?;
        let groups = groups_for(product, chosen);
        for (group_id, _) in picks {
            if !groups.iter().any(|(g, _)| g.id == *group_id) {
                let name = product
                    .options
                    .choices
                    .iter()
                    .find(|g| g.id == *group_id)
                    .map_or_else(|| group_id.to_string(), |g| g.title().to_owned());
                return Err(LineError::NoSuchGroup {
                    product: product.name.clone(),
                    group: name,
                });
            }
        }
        let mut choices = Map::new();
        let mut names = Vec::new();
        for (group, required) in &groups {
            let Some((_, option_id)) = picks.iter().find(|(g, _)| *g == group.id) else {
                if *required {
                    return Err(LineError::MissingChoice {
                        product: product.name.clone(),
                        group: group.title().to_owned(),
                        options: describe(group),
                    });
                }
                continue;
            };
            let option = group.options.iter().find(|o| o.id == *option_id).ok_or_else(|| {
                LineError::NoSuchChoice {
                    product: product.name.clone(),
                    group: group.title().to_owned(),
                    option: *option_id,
                    options: describe(group),
                }
            })?;
            if option.is_out_of_stock {
                return Err(LineError::ChoiceOutOfStock {
                    product: product.name.clone(),
                    option: option.name.clone(),
                });
            }
            let option_portion = find_portion(option, None).map(|p| p.value.id).unwrap_or(1);
            let mut body = till_product(category, option, option_portion);
            body["checkout"]["choiceId"] = json!(group.id);
            choices.insert(group.id.to_string(), body);
            names.push(option.name.clone());
        }

        Ok(Self {
            portion: chosen.label.clone(),
            choices: names,
            body: json!({
                "product": till_product(category, product, chosen.value.id),
                "quantity": quantity,
                "portion": { "id": chosen.value.id, "choices": chosen.value.choices },
                "choices": choices,
                "addOns": {},
                "tags": {},
                "swaps": {},
                "tillRequests": [],
            }),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn product(extra: Value) -> Product {
        let mut product = json!({
            "id": 10000000311_u64,
            "name": "Guinness",
            "courseId": 12,
            "displayRecordId": 523614,
            "checkout": { "id": 10000000311_u64, "name": "Guinness", "menuId": 34585, "messages": [] },
            "keywords": [
                { "type": "AL", "value": "gluten", "tags": [] },
                { "type": "vegan", "value": null, "tags": { "isVegan": true } },
            ],
            "options": { "portion": { "options": [
                { "label": "Half pint", "isDefault": false, "value": { "id": 2, "price": { "value": 2.11 }, "choices": [] } },
                { "label": "Pint", "isDefault": true, "value": { "id": 1, "price": { "value": 4.22 }, "choices": [] } },
            ] } },
        });
        if let (Value::Object(product), Value::Object(extra)) = (&mut product, extra) {
            product.extend(extra);
        }
        serde_json::from_value(product).unwrap()
    }

    #[test]
    fn builds_the_line_the_app_sends() {
        let line = BasketLine::new(400300, &product(json!({})), Some("pint"), 2, &[]).unwrap();
        assert_eq!(
            serde_json::to_value(&line).unwrap(),
            json!({
                "product": {
                    "id": 10000000311_u64,
                    "name": "Guinness",
                    "category": { "id": 400300 },
                    "course": { "id": 12 },
                    "portion": { "id": 1 },
                    "keywordTags": { "isVegan": true },
                    "checkout": { "id": 10000000311_u64, "name": "Guinness", "menuId": 34585, "messages": [], "displayRecordId": 523614 },
                },
                "quantity": 2,
                "portion": { "id": 1, "choices": [] },
                "choices": {},
                "addOns": {},
                "tags": {},
                "swaps": {},
                "tillRequests": [],
            })
        );
    }

    #[test]
    fn takes_the_default_size_when_none_is_asked_for() {
        let line = BasketLine::new(1, &product(json!({})), None, 1, &[]).unwrap();
        assert_eq!(line.body["portion"]["id"], 1);
        assert_eq!(line.portion, "Pint");
    }

    #[test]
    fn names_the_sizes_when_the_one_asked_for_is_missing() {
        let error = BasketLine::new(1, &product(json!({})), Some("Schooner"), 1, &[]).unwrap_err();
        assert_eq!(
            error.to_string(),
            "Guinness doesn't come as Schooner; it comes as Half pint, Pint"
        );
    }

    fn spirit() -> Product {
        product(json!({ "name": "Bell's", "categoryNote": null, "options": {
            "portion": { "options": [
                { "label": "Single", "isDefault": true, "value": { "id": 1, "price": { "value": 2.5 },
                  "choices": [{ "choiceId": 10000001149_u64, "portionId": 1, "displayRecordId": 7, "required": true }] } },
            ] },
            "choices": [{ "id": 10000001149_u64, "name": ".Mixer Choice", "required": true, "options": [
                { "id": 31, "name": "No mixer", "courseId": 3, "displayRecordId": 99,
                  "checkout": { "id": 31, "name": "NO MIXER" },
                  "options": { "portion": { "options": [
                      { "label": "Standard", "isDefault": true, "value": { "id": 1, "price": { "value": 0 } } } ] } } },
                { "id": 32, "name": "R White's lemonade", "courseId": 3, "displayRecordId": 100,
                  "checkout": { "id": 32, "name": "LEMONADE" },
                  "options": { "portion": { "options": [
                      { "label": "Standard", "isDefault": true, "value": { "id": 1, "price": { "value": 0 } } } ] } } },
            ] }],
        } }))
    }

    #[test]
    fn asks_for_a_spirits_mixer_naming_them() {
        let error = BasketLine::new(1, &spirit(), None, 1, &[]).unwrap_err();
        assert_eq!(
            error.to_string(),
            "Bell's needs a choice for “Mixer Choice”: No mixer, R White's lemonade"
        );
    }

    #[test]
    fn sends_a_choice_as_the_app_does() {
        let line = BasketLine::new(400260, &spirit(), None, 2, &[(10000001149, 32)]).unwrap();
        let body = serde_json::to_value(&line).unwrap();
        assert_eq!(line.choices, ["R White's lemonade"]);
        assert_eq!(
            body["portion"],
            json!({ "id": 1, "choices": [{ "choiceId": 10000001149_u64, "portionId": 1, "displayRecordId": 7, "required": true }] })
        );
        let choice = &body["choices"]["10000001149"];
        assert_eq!(choice["id"], 32);
        assert_eq!(choice["category"], json!({ "id": 400260 }));
        assert_eq!(choice["course"], json!({ "id": 3 }));
        assert_eq!(choice["portion"], json!({ "id": 1 }));
        assert_eq!(choice["checkout"], json!({ "id": 32, "name": "LEMONADE", "displayRecordId": 100, "choiceId": 10000001149_u64 }));
    }

    #[test]
    fn refuses_an_option_from_nowhere() {
        let error = BasketLine::new(1, &spirit(), None, 1, &[(10000001149, 77)]).unwrap_err();
        assert!(matches!(error, LineError::NoSuchChoice { .. }), "{error}");
        let error = BasketLine::new(1, &spirit(), None, 1, &[(5, 31)]).unwrap_err();
        assert!(matches!(error, LineError::NoSuchGroup { .. }), "{error}");
    }

    #[test]
    fn refuses_what_is_out_of_stock() {
        let gone = product(json!({ "isOutOfStock": true }));
        assert!(matches!(
            BasketLine::new(1, &gone, None, 1, &[]),
            Err(LineError::OutOfStock(_))
        ));
    }
}
