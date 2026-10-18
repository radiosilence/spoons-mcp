use std::collections::HashSet;
use std::fs;
use std::io::{BufWriter, Write};
use std::path::Path;

use anyhow::Result;
use serde::Serialize;
use spoons_api_client::{Item, Menu, Venue};

use crate::dump::{data, read};

/// One drink in one size at one pub.
#[derive(Serialize)]
struct Row<'a> {
    venue_ref: u64,
    pub_name: &'a str,
    /// `airport` and the like; nothing for a high-street pub.
    sub_type: Option<&'a str>,
    town: Option<&'a str>,
    country: &'a str,
    currency: &'a str,
    menu: &'a str,
    category: &'a str,
    product_id: u64,
    product: &'a str,
    abv: Option<f64>,
    units: Option<f64>,
    portion: &'a str,
    price: f64,
    calories: Option<f64>,
    out_of_stock: bool,
}

/// Flattens every dumped menu into `prices.jsonl`, one line per drink and size per pub, for reading into anything.
/// A drink listed under several categories is listed once per category, since a meal deal's "includes a drink"
/// price isn't the drink's own; the category says which is which.
pub fn build(dir: &Path) -> Result<()> {
    let mut out = BufWriter::new(fs::File::create(dir.join("prices.jsonl"))?);
    let mut rows = 0;
    for entry in fs::read_dir(dir.join("pubs"))? {
        let pub_dir = entry?.path();
        let Ok(body) = read(&pub_dir.join("venue.json"), false) else {
            continue;
        };
        let venue: Venue = data(&body)?;
        let currency = venue
            .currency
            .as_ref()
            .map_or("GBP", |currency| currency.code.as_str());
        for file in fs::read_dir(&pub_dir)? {
            let path = file?.path();
            let name = path.file_name().unwrap_or_default().to_string_lossy();
            if !(name.starts_with("menu-") && name.ends_with(".json.gz")) {
                continue;
            }
            let menu: Menu = data(&read(&path, true)?)?;
            let mut seen = HashSet::new();
            for category in &menu.categories {
                for product in category
                    .item_groups
                    .iter()
                    .flat_map(|group| &group.items)
                    .filter_map(|item| match item {
                        Item::Product(product) => Some(product),
                        _ => None,
                    })
                {
                    let (abv, units) = strength(product.description.as_deref().unwrap_or_default());
                    for (portion, price) in product.portions() {
                        if !seen.insert((category.id, product.id, portion.to_owned())) {
                            continue;
                        }
                        serde_json::to_writer(
                            &mut out,
                            &Row {
                                venue_ref: venue.venue_ref,
                                pub_name: &venue.name,
                                sub_type: venue.sub_type.as_deref(),
                                town: venue.address.town.as_deref(),
                                country: &venue.address.country.name,
                                currency,
                                menu: &menu.name,
                                category: category.name.trim(),
                                product_id: product.id,
                                product: &product.name,
                                abv,
                                units,
                                portion,
                                price: price.value,
                                calories: product.calories,
                                out_of_stock: product.is_out_of_stock,
                            },
                        )?;
                        out.write_all(b"\n")?;
                        rows += 1;
                    }
                }
            }
        }
    }
    out.flush()?;
    tracing::info!("{rows} prices in {}", dir.join("prices.jsonl").display());
    Ok(())
}

/// `4.0% ABV, 2.3 units` into its two numbers.
fn strength(description: &str) -> (Option<f64>, Option<f64>) {
    let number_before = |marker: &str| {
        let end = description.find(marker)?;
        let start = description[..end]
            .rfind(|c: char| !(c.is_ascii_digit() || c == '.'))
            .map_or(0, |i| i + 1);
        description[start..end].parse().ok()
    };
    (number_before("% ABV"), number_before(" units"))
}

#[cfg(test)]
mod tests {
    use super::strength;

    #[test]
    fn reads_strength_and_units() {
        assert_eq!(strength("4.0% ABV, 2.3 units"), (Some(4.0), Some(2.3)));
        assert_eq!(
            strength("Smirnoff 37.5% ABV, 0.9 units"),
            (Some(37.5), Some(0.9))
        );
        assert_eq!(strength("Served with ice"), (None, None));
    }
}
