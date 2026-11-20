//! Turning what someone said into the options a product asks for: "chips", "mushy peas", "lemonade", "neat", or
//! the ids from the menu. Each group takes one option. Anything that can't be placed, or could be placed in two
//! groups, is refused with the choices listed, so the model asks the user rather than guessing; a required group
//! left empty is refused the same way, by [`spoons_api_client::BasketLine::new`].

use spoons_api_client::{ChoiceGroup, Product, describe, find_portion, groups_for};

use crate::search::{normalise, text_score};

/// One group's picks, named by the group's id or words from its name.
#[derive(Clone, Debug, Default)]
pub struct Explicit {
    pub group: Option<String>,
    pub options: Vec<String>,
}

/// What people say for an option the menu names otherwise.
const ALIASES: &[(&str, &str)] = &[
    ("neat", "no mixer"),
    ("straight", "no mixer"),
    ("on its own", "no mixer"),
    ("nothing", "no mixer"),
    ("no peas", "plain"),
];

/// Words that make a mixer a variant of the plain one.
const VARIANTS: &[&str] = &[
    "diet",
    "light",
    "lite",
    "zero",
    "max",
    "cherry",
    "elderflower",
    "raspberry",
    "blackcurrant",
    "lime",
    "pink",
    "slimline",
    "sugar",
    "free",
    "flavour",
    "flavoured",
    "orange",
    "mango",
    "strawberry",
    "cranberry",
    "and",
];

fn is_plain(name: &str) -> bool {
    normalise(name).split(' ').all(|w| !VARIANTS.contains(&w))
}

fn option_score(said: &str, option: &Product) -> f32 {
    if option.id.to_string() == said.trim() {
        return 10.0;
    }
    let said = normalise(said);
    let said = ALIASES
        .iter()
        .find(|(alias, _)| *alias == said)
        .map_or(said.clone(), |(_, means)| (*means).to_owned());
    let name = normalise(&option.name);
    if name == said {
        return 5.0;
    }
    text_score(&said, &name)
}

fn group_matches(said: &str, group: &ChoiceGroup) -> bool {
    group.id.to_string() == said.trim()
        || text_score(&normalise(said), &normalise(group.title())) > 0.0
}

fn all_choices(groups: &[(&ChoiceGroup, bool)]) -> String {
    groups
        .iter()
        .map(|(g, _)| format!("“{}”: {}", g.title(), describe(g)))
        .collect::<Vec<_>>()
        .join("; ")
}

/// The (group id, option id) picks for `product` in `portion`, from loose words and explicit picks.
pub fn resolve(
    product: &Product,
    portion: Option<&str>,
    said: &[String],
    explicit: &[Explicit],
) -> Result<Vec<(u64, u64)>, String> {
    let size = find_portion(product, portion).map_err(|e| e.to_string())?;
    let groups = groups_for(product, size);
    if groups.is_empty() && (said.iter().any(|s| !s.trim().is_empty()) || !explicit.is_empty()) {
        return Err(format!("{} has no options to choose", product.name));
    }
    let mut picks: Vec<(u64, u64)> = Vec::new();
    let take = |group: &ChoiceGroup, option: &Product, picks: &mut Vec<(u64, u64)>| {
        if let Some((_, already)) = picks.iter().find(|(g, _)| *g == group.id) {
            if *already == option.id {
                return Ok(());
            }
            return Err(format!(
                "“{}” takes one option, for {}: {}",
                group.title(),
                product.name,
                describe(group)
            ));
        }
        picks.push((group.id, option.id));
        Ok(())
    };

    for pick in explicit {
        let candidates: Vec<&ChoiceGroup> = match &pick.group {
            Some(named) => {
                let found: Vec<&ChoiceGroup> = groups
                    .iter()
                    .map(|(g, _)| *g)
                    .filter(|g| group_matches(named, g))
                    .collect();
                if found.is_empty() {
                    return Err(format!(
                        "{} asks no “{named}”; it asks {}",
                        product.name,
                        all_choices(&groups)
                    ));
                }
                found
            }
            None => groups.iter().map(|(g, _)| *g).collect(),
        };
        for wanted in &pick.options {
            let (group, option) = best(wanted, &candidates).ok_or_else(|| {
                format!(
                    "“{wanted}” isn't one of {}'s options: {}",
                    product.name,
                    all_choices(&groups)
                )
            })??;
            take(group, option, &mut picks)?;
        }
    }

    for wanted in said.iter().filter(|s| !s.trim().is_empty()) {
        // Groups already answered are only candidates for the same answer again.
        let open: Vec<&ChoiceGroup> = groups
            .iter()
            .map(|(g, _)| *g)
            .filter(|g| !picks.iter().any(|(id, _)| *id == g.id))
            .collect();
        let every: Vec<&ChoiceGroup> = groups.iter().map(|(g, _)| *g).collect();
        let (group, option) = best(wanted, &open)
            .or_else(|| best(wanted, &every))
            .ok_or_else(|| {
                format!(
                    "“{wanted}” isn't one of {}'s options: {}",
                    product.name,
                    all_choices(&groups)
                )
            })??;
        take(group, option, &mut picks)?;
    }
    Ok(picks)
}

/// The best option for `wanted` across `groups`; an error when two groups match it equally well.
fn best<'a>(
    wanted: &str,
    groups: &[&'a ChoiceGroup],
) -> Option<Result<(&'a ChoiceGroup, &'a Product), String>> {
    let mut scored: Vec<(f32, &ChoiceGroup, &Product)> = groups
        .iter()
        .flat_map(|g| {
            g.options
                .iter()
                .map(move |o| (option_score(wanted, o), *g, o))
        })
        .filter(|(score, _, _)| *score > 0.0)
        .collect();
    scored.sort_by(|a, b| b.0.total_cmp(&a.0));
    let (top, group, option) = *scored.first()?;
    if let Some((_, other, _)) = scored
        .iter()
        .skip(1)
        .find(|(s, g, _)| *s == top && g.id != group.id)
    {
        return Some(Err(format!(
            "“{wanted}” could be for “{}” or “{}”; say which, or pick by id",
            group.title(),
            other.title()
        )));
    }
    // Equally good options in one group: the plain one is what "lemonade" or "tonic" means (R White's, not
    // raspberry; Indian tonic, not diet or elderflower). Two equally plain ones ("potato": mash or jacket) are the
    // user's to choose.
    let tied: Vec<&Product> = scored
        .iter()
        .filter(|(s, g, _)| *s == top && g.id == group.id)
        .map(|(_, _, o)| *o)
        .collect();
    if tied.len() > 1 {
        let plain: Vec<&Product> = tied.iter().copied().filter(|o| is_plain(&o.name)).collect();
        return Some(match plain.as_slice() {
            [only] => Ok((group, *only)),
            _ => Err(format!(
                "“{wanted}” could be {}: which?",
                tied.iter()
                    .map(|o| o.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            )),
        });
    }
    Some(Ok((group, option)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn option(id: u64, name: &str, price: f64) -> serde_json::Value {
        json!({ "id": id, "name": name, "checkout": {},
                "options": { "portion": { "options": [
                    { "label": "Standard", "isDefault": true, "value": { "id": 1, "price": { "value": price } } } ] } } })
    }

    fn pudding() -> Product {
        serde_json::from_value(json!({
            "id": 10000161536_u64, "name": "Steak & ale pudding",
            "options": {
                "portion": { "options": [{ "label": "Standard", "isDefault": true, "value": { "id": 1,
                    "price": { "value": 9.12 },
                    "choices": [{ "choiceId": 943, "required": true }, { "choiceId": 559, "required": true }] } }] },
                "choices": [
                    { "id": 943, "name": "Peas or mushy peas?", "required": true,
                      "options": [option(921, "Peas", 0.0), option(922, "Mushy peas", 0.0)] },
                    { "id": 559, "name": "Choose a side?", "required": true,
                      "options": [option(31, "Chips", 0.0), option(32, "Mash potato", 0.0), option(33, "Side salad", 0.0), option(34, "Jacket potato", 0.0)] },
                ],
            },
        }))
        .unwrap()
    }

    fn said(words: &[&str]) -> Vec<String> {
        words.iter().map(|w| (*w).to_owned()).collect()
    }

    #[test]
    fn places_spoken_choices_in_their_groups() {
        let picks = resolve(&pudding(), None, &said(&["chips", "mushy peas"]), &[]).unwrap();
        assert_eq!(picks, [(559, 31), (943, 922)]);
        let picks = resolve(&pudding(), None, &said(&["mash", "peas"]), &[]).unwrap();
        assert_eq!(picks, [(559, 32), (943, 921)]);
    }

    #[test]
    fn takes_ids_and_explicit_groups() {
        let explicit = [Explicit {
            group: Some("side".into()),
            options: vec!["33".into()],
        }];
        assert_eq!(
            resolve(&pudding(), None, &[], &explicit).unwrap(),
            [(559, 33)]
        );
    }

    #[test]
    fn lists_the_choices_for_something_that_isnt_one() {
        let error = resolve(&pudding(), None, &said(&["rice"]), &[]).unwrap_err();
        assert!(
            error.contains("“Choose a side?”: Chips, Mash potato, Side salad"),
            "{error}"
        );
    }

    #[test]
    fn asks_which_when_two_options_fit_equally() {
        let error = resolve(&pudding(), None, &said(&["potato"]), &[]).unwrap_err();
        assert!(error.contains("Mash potato"), "{error}");
        assert_eq!(
            resolve(&pudding(), None, &said(&["mash"]), &[]).unwrap(),
            [(559, 32)]
        );
    }

    #[test]
    fn refuses_two_answers_to_one_question() {
        let error = resolve(
            &pudding(),
            None,
            &said(&["chips", "mash", "peas", "side salad"]),
            &[],
        )
        .unwrap_err();
        assert!(error.contains("takes one option"), "{error}");
    }
}
