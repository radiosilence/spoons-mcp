use std::collections::HashMap;
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::Path;

use anyhow::Result;
use serde::{Deserialize, Serialize};

/// A round of staples: drinks almost every pub sells, whose prices vary from pub to pub. Abbot is the ale because it's
/// the chain's own and on everywhere; Doom Bar, the obvious choice, is missing from one pub in seven. A size matches
/// by how its label starts, since some menus add the calories to it.
const BASKET: [(&str, &str); 6] = [
    ("Carling", "Pint"),
    ("Guinness", "Pint"),
    ("Greene King Abbot Ale", "Pint"),
    ("Strongbow", "Pint"),
    ("Coldwater Creek Merlot by Concha y Toro", "250ml glass"),
    ("Bell’s", "Double"),
];

/// The Irish pubs sell neither Carling nor Bell's, and call the Merlot something shorter.
const IRISH_BASKET: [(&str, &str); 6] = [
    ("Coors", "Pint"),
    ("Guinness", "Pint"),
    ("Greene King Abbot Ale", "Pint"),
    ("Strongbow", "Pint"),
    ("Coldwater Creek Merlot", "250ml glass"),
    ("Jameson", "Double"),
];

/// Below this many of the basket, a pub's index says too little to rank it.
const ENOUGH: usize = 4;

/// Cheapest first. Each tier is the pubs up to that share of the way through the ranking.
const TIERS: [(&str, f64); 6] = [
    ("S", 0.10),
    ("A", 0.30),
    ("B", 0.55),
    ("C", 0.80),
    ("D", 0.95),
    ("F", 1.0),
];

#[derive(Deserialize)]
struct Row {
    venue_ref: u64,
    pub_name: String,
    sub_type: Option<String>,
    town: Option<String>,
    country: String,
    currency: String,
    category: String,
    product: String,
    portion: String,
    price: f64,
}

#[derive(Serialize)]
pub struct Ranked {
    /// `S` to `F`, or `airport`: an airside pint isn't a local price, so airports are ranked among themselves.
    tier: &'static str,
    venue_ref: u64,
    pub_name: String,
    town: Option<String>,
    country: String,
    currency: String,
    /// The pub's prices against the median, 1.0 being typical.
    index: f64,
    /// What the basket would cost at this pub's prices.
    round: f64,
    prices: HashMap<String, f64>,
}

/// Ranks every pub by what the basket costs there, into `tiers.json` and `tiers.md`.
pub fn build(dir: &Path) -> Result<()> {
    // A meal deal's "includes a drink" price isn't the drink's own, so deal categories are left out; where a drink is
    // listed twice at one pub, its lower price is the one anyone would pay.
    let mut pubs: HashMap<u64, (Row, HashMap<String, f64>)> = HashMap::new();
    for line in BufReader::new(fs::File::open(dir.join("prices.jsonl"))?).lines() {
        let row: Row = serde_json::from_str(&line?)?;
        if row.category.starts_with("Includes") || row.category.contains(" for ") {
            continue;
        }
        let basket = if row.currency == "EUR" {
            &IRISH_BASKET
        } else {
            &BASKET
        };
        let Some((product, portion)) = basket
            .iter()
            .find(|(product, portion)| row.product == *product && row.portion.starts_with(portion))
        else {
            continue;
        };
        let key = format!("{product} ({portion})");
        let price = row.price;
        let (_, prices) = pubs
            .entry(row.venue_ref)
            .or_insert_with(|| (row, HashMap::new()));
        prices
            .entry(key)
            .and_modify(|existing| *existing = existing.min(price))
            .or_insert(price);
    }

    let mut out = String::from("# Wetherspoon's, by what a round costs\n\n");
    out += "A round is a pint each of Carling, Guinness, Abbot Ale and Strongbow, a 250 ml glass of Coldwater Creek Merlot and a double Bell's with a mixer; in the Republic of Ireland, Coors and Jameson stand in for Carling and Bell's. ";
    out += "Each pub's price for each drink is set against the median across every pub; the average of those is its index, and the round is the basket at the median priced by that index, so a pub missing a drink is still comparable. ";
    out += "Tiers are by rank: S is the cheapest tenth, F the dearest twentieth, and pubs charging the same share a tier. Airport pubs are ranked separately.\n";

    let mut ranked_all = Vec::new();
    for (currency, airports, heading) in [
        ("GBP", false, "UK and the Isle of Man"),
        ("GBP", true, "UK airports"),
        ("EUR", false, "Republic of Ireland"),
    ] {
        let basket = if currency == "EUR" {
            &IRISH_BASKET
        } else {
            &BASKET
        };
        let group: Vec<_> = pubs
            .values()
            .filter(|(row, _)| {
                row.currency == currency && (row.sub_type.as_deref() == Some("airport")) == airports
            })
            .collect();
        if group.is_empty() {
            continue;
        }
        let medians: HashMap<String, f64> = basket
            .iter()
            .map(|(product, portion)| format!("{product} ({portion})"))
            .filter_map(|key| {
                let mut prices: Vec<f64> = group
                    .iter()
                    .filter_map(|(_, prices)| prices.get(&key).copied())
                    .collect();
                (!prices.is_empty()).then(|| (key, median(&mut prices)))
            })
            .collect();
        let full_round: f64 = medians.values().sum();
        let mut ranked: Vec<Ranked> = group
            .iter()
            .filter(|(_, prices)| prices.len() >= ENOUGH)
            .map(|(row, prices)| {
                let ratios: Vec<f64> = prices
                    .iter()
                    .filter_map(|(key, price)| medians.get(key).map(|median| price / median))
                    .collect();
                let index = ratios.iter().sum::<f64>() / ratios.len() as f64;
                Ranked {
                    tier: "",
                    venue_ref: row.venue_ref,
                    pub_name: row.pub_name.trim().to_owned(),
                    town: row.town.clone(),
                    country: row.country.clone(),
                    currency: row.currency.clone(),
                    index,
                    round: full_round * index,
                    prices: prices.clone(),
                }
            })
            .collect();
        ranked.sort_by(|a, b| a.index.total_cmp(&b.index));
        // A pub's tier is decided by the first pub at its price, so a zone's pubs aren't split across two.
        let count = ranked.len();
        let mut first_at_price = 0;
        for i in 0..count {
            if (ranked[i].round - ranked[first_at_price].round).abs() >= 0.005 {
                first_at_price = i;
            }
            let through = (first_at_price + 1) as f64 / count as f64;
            ranked[i].tier = if airports {
                "airport"
            } else {
                TIERS
                    .iter()
                    .find(|(_, share)| through <= *share + f64::EPSILON)
                    .map_or("F", |(tier, _)| tier)
            };
        }

        let symbol = if currency == "EUR" { "€" } else { "£" };
        out += &format!("\n## {heading}\n\nThe typical round costs {symbol}{full_round:.2}.\n");
        if airports {
            out += "\n";
            for pub_ in &ranked {
                out += &format!(
                    "- {symbol}{:.2} {}, {}\n",
                    pub_.round,
                    pub_.pub_name,
                    pub_.town.as_deref().unwrap_or(&pub_.country)
                );
            }
            ranked_all.extend(ranked);
            continue;
        }
        for (tier, _) in TIERS {
            let members: Vec<_> = ranked.iter().filter(|pub_| pub_.tier == tier).collect();
            if members.is_empty() {
                continue;
            }
            out += &format!(
                "\n### {tier} ({} pubs, {symbol}{:.2}–{symbol}{:.2})\n\n",
                members.len(),
                members[0].round,
                members[members.len() - 1].round
            );
            for pub_ in members {
                out += &format!(
                    "- {symbol}{:.2} {}, {}\n",
                    pub_.round,
                    pub_.pub_name,
                    pub_.town.as_deref().unwrap_or(&pub_.country)
                );
            }
        }
        ranked_all.extend(ranked);
    }
    fs::write(dir.join("tiers.md"), out)?;
    fs::write(
        dir.join("tiers.json"),
        serde_json::to_vec_pretty(&ranked_all)?,
    )?;
    tracing::info!(
        "{} pubs ranked into {}",
        ranked_all.len(),
        dir.join("tiers.md").display()
    );
    Ok(())
}

fn median(values: &mut [f64]) -> f64 {
    values.sort_by(f64::total_cmp);
    let middle = values.len() / 2;
    if values.len().is_multiple_of(2) {
        (values[middle - 1] + values[middle]) / 2.0
    } else {
        values[middle]
    }
}

#[cfg(test)]
mod tests {
    use super::median;

    #[test]
    fn medians() {
        assert_eq!(median(&mut [3.0, 1.0, 2.0]), 2.0);
        assert_eq!(median(&mut [4.0, 1.0, 2.0, 3.0]), 2.5);
    }
}
