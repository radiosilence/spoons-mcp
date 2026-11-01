//! Basket lines in the shape the app sends to `basket/validate`, built from a menu's products.

use serde::Serialize;
use serde_json::{Map, Value, json};

use crate::models::Product;

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
    #[error("{0} needs a choice made, such as a mixer, which isn't supported yet")]
    NeedsChoice(String),
}

/// One product in a basket, in one size.
#[derive(Debug, Clone, Serialize)]
pub struct BasketLine(Value);

impl BasketLine {
    /// `category` is the id of the menu category the product was listed under; `portion` is a size's label, such as
    /// `Pint`, matched without regard to case, or the default size when `None`.
    pub fn new(
        category: u64,
        product: &Product,
        portion: Option<&str>,
        quantity: u32,
    ) -> Result<Self, LineError> {
        if product.is_out_of_stock {
            return Err(LineError::OutOfStock(product.name.clone()));
        }
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
        let Some(chosen) = chosen else {
            return Err(LineError::NoSuchPortion {
                product: product.name.clone(),
                portion: portion.unwrap_or("any size").to_owned(),
                available: options
                    .map(|option| option.label.as_str())
                    .collect::<Vec<_>>()
                    .join(", "),
            });
        };
        if chosen.value.choices.iter().any(|choice| choice.required) {
            return Err(LineError::NeedsChoice(product.name.clone()));
        }

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

        Ok(Self(json!({
            "product": {
                "id": product.id,
                "name": product.name,
                "category": { "id": category },
                "course": { "id": product.course_id },
                "portion": { "id": chosen.value.id },
                "keywordTags": keyword_tags,
                "checkout": checkout,
            },
            "quantity": quantity,
            "portion": { "id": chosen.value.id, "choices": [] },
            "choices": {},
            "addOns": {},
            "tags": {},
            "swaps": {},
            "tillRequests": [],
        })))
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
        let line = BasketLine::new(400300, &product(json!({})), Some("pint"), 2).unwrap();
        assert_eq!(
            line.0,
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
        let line = BasketLine::new(1, &product(json!({})), None, 1).unwrap();
        assert_eq!(line.0["portion"]["id"], 1);
    }

    #[test]
    fn names_the_sizes_when_the_one_asked_for_is_missing() {
        let error = BasketLine::new(1, &product(json!({})), Some("Schooner"), 1).unwrap_err();
        assert_eq!(
            error.to_string(),
            "Guinness doesn't come as Schooner; it comes as Half pint, Pint"
        );
    }

    #[test]
    fn refuses_a_size_that_needs_a_mixer() {
        let spirit = product(
            json!({ "name": "Bell's", "options": { "portion": { "options": [
            { "label": "Single", "isDefault": true, "value": { "id": 1, "price": { "value": 2.5 },
              "choices": [{ "choiceId": 10000160789_u64, "required": true }] } },
        ] } } }),
        );
        assert!(matches!(
            BasketLine::new(1, &spirit, None, 1),
            Err(LineError::NeedsChoice(_))
        ));
    }

    #[test]
    fn refuses_what_is_out_of_stock() {
        let gone = product(json!({ "isOutOfStock": true }));
        assert!(matches!(
            BasketLine::new(1, &gone, None, 1),
            Err(LineError::OutOfStock(_))
        ));
    }
}
