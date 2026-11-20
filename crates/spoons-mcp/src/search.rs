//! Finding things the way people ask for them, which is mostly by voice: misheard ("Broccoli Barge"), styled
//! ("Mad Squirrel $umo"), or by what a drink is rather than what it's called ("a mocktail").
//!
//! A query is normalised and split into words. Each word is scored against each field of a candidate: exactly,
//! as a prefix, inside a word, within a small edit distance, or sounding alike (Soundex), weighted by the field. A
//! small synonym map adds what the menu never says: a mocktail is an alcohol-free cocktail. Candidates that match
//! every word come back ranked; when none do, the nearest come back flagged as close matches, never nothing.

use crate::catalogue::{Entry, Kind};

/// Lower-case ASCII words: `$`→s, accents, ®/™, apostrophes and curly quotes folded away, other punctuation a
/// space. `%` stays, and a `.` between digits, so "0.0%" survives.
pub fn normalise(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let chars: Vec<char> = text.chars().collect();
    for (i, c) in chars.iter().enumerate() {
        let folded = match c {
            '$' => "s",
            'á' | 'à' | 'â' | 'ä' | 'ã' | 'å' | 'Á' | 'À' | 'Â' | 'Ä' => "a",
            'é' | 'è' | 'ê' | 'ë' | 'É' | 'È' | 'Ê' | 'Ë' => "e",
            'í' | 'ì' | 'î' | 'ï' | 'Í' | 'Î' | 'Ï' => "i",
            'ó' | 'ò' | 'ô' | 'ö' | 'õ' | 'ø' | 'Ó' | 'Ô' | 'Ö' => "o",
            'ú' | 'ù' | 'û' | 'ü' | 'Ú' | 'Û' | 'Ü' => "u",
            'ç' | 'Ç' => "c",
            'ñ' | 'Ñ' => "n",
            'ß' => "ss",
            '’' | '‘' | '\'' | '`' | '®' | '™' | '©' => "",
            '&' => " and ",
            '%' => "%",
            '.' if i > 0
                && chars[i - 1].is_ascii_digit()
                && chars.get(i + 1).is_some_and(char::is_ascii_digit) =>
            {
                "."
            }
            c if c.is_alphanumeric() => {
                out.extend(c.to_lowercase());
                continue;
            }
            _ => " ",
        };
        out.push_str(folded);
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Words that say nothing about which product or pub: filler, and sizes, which are chosen separately.
const PRODUCT_STOPWORDS: &[&str] = &[
    "something",
    "anything",
    "drink",
    "drinks",
    "a",
    "an",
    "the",
    "of",
    "and",
    "with",
    "some",
    "please",
    "me",
    "i",
    "id",
    "want",
    "get",
    "could",
    "have",
    "like",
    "would",
    "any",
    "for",
    "pint",
    "pints",
    "half",
    "halves",
    "third",
    "thirds",
    "glass",
    "glasses",
    "bottle",
    "bottles",
    "can",
    "cans",
    "large",
    "small",
    "x",
];

const PUB_STOPWORDS: &[&str] = &[
    "a",
    "an",
    "the",
    "of",
    "and",
    "in",
    "at",
    "near",
    "by",
    "pub",
    "pubs",
    "spoons",
    "spoon",
    "wetherspoon",
    "wetherspoons",
    "weatherspoons",
    "weatherspoon",
    "jd",
    "jdw",
    "one",
];

/// What the menu never says. A query word (or two) adds these kinds, and these phrases looked for in the pub's
/// own category names.
struct Synonym {
    words: &'static str,
    kinds: &'static [Kind],
    phrases: &'static [&'static str],
}

const SYNONYMS: &[Synonym] = &[
    Synonym {
        words: "mocktail",
        kinds: &[Kind::AlcoholFree],
        phrases: &["0% cocktail"],
    },
    Synonym {
        words: "virgin",
        kinds: &[Kind::AlcoholFree],
        phrases: &["0% cocktail"],
    },
    Synonym {
        words: "alcohol free",
        kinds: &[Kind::AlcoholFree],
        phrases: &["alcohol free", "0%"],
    },
    Synonym {
        words: "non alcoholic",
        kinds: &[Kind::AlcoholFree],
        phrases: &["alcohol free", "0%"],
    },
    Synonym {
        words: "nonalcoholic",
        kinds: &[Kind::AlcoholFree],
        phrases: &["alcohol free", "0%"],
    },
    Synonym {
        words: "zero alcohol",
        kinds: &[Kind::AlcoholFree],
        phrases: &["alcohol free", "0%"],
    },
    Synonym {
        words: "af",
        kinds: &[Kind::AlcoholFree],
        phrases: &["alcohol free"],
    },
    Synonym {
        words: "0%",
        kinds: &[Kind::AlcoholFree],
        phrases: &["0%", "alcohol free"],
    },
    Synonym {
        words: "sober",
        kinds: &[Kind::AlcoholFree, Kind::Soft],
        phrases: &["alcohol free"],
    },
    Synonym {
        words: "cocktail",
        kinds: &[Kind::Cocktails, Kind::AlcoholFree],
        phrases: &["cocktail", "spritz"],
    },
    Synonym {
        words: "lager",
        kinds: &[Kind::Lager],
        phrases: &["lager"],
    },
    Synonym {
        words: "pilsner",
        kinds: &[Kind::Lager],
        phrases: &["lager"],
    },
    Synonym {
        words: "stout",
        kinds: &[Kind::Stout],
        phrases: &["stout"],
    },
    Synonym {
        words: "porter",
        kinds: &[Kind::Stout, Kind::CaskAle],
        phrases: &["stout"],
    },
    Synonym {
        words: "ale",
        kinds: &[Kind::CaskAle, Kind::CraftAndWorld],
        phrases: &["real ale"],
    },
    Synonym {
        words: "real ale",
        kinds: &[Kind::CaskAle],
        phrases: &["real ale"],
    },
    Synonym {
        words: "cask",
        kinds: &[Kind::CaskAle],
        phrases: &["real ale"],
    },
    Synonym {
        words: "bitter",
        kinds: &[Kind::CaskAle],
        phrases: &["real ale"],
    },
    Synonym {
        words: "guest",
        kinds: &[Kind::CaskAle],
        phrases: &["real ale"],
    },
    Synonym {
        words: "ipa",
        kinds: &[Kind::CraftAndWorld, Kind::CaskAle],
        phrases: &["craft"],
    },
    Synonym {
        words: "craft",
        kinds: &[Kind::CraftAndWorld],
        phrases: &["craft"],
    },
    Synonym {
        words: "beer",
        kinds: &[Kind::Lager, Kind::CraftAndWorld, Kind::CaskAle, Kind::Stout],
        phrases: &["beer", "lager", "real ale", "craft"],
    },
    Synonym {
        words: "cider",
        kinds: &[Kind::Cider],
        phrases: &["cider"],
    },
    Synonym {
        words: "wine",
        kinds: &[Kind::Wine],
        phrases: &["wine"],
    },
    Synonym {
        words: "fizz",
        kinds: &[Kind::Wine],
        phrases: &["prosecco", "sparkling"],
    },
    Synonym {
        words: "bubbly",
        kinds: &[Kind::Wine],
        phrases: &["prosecco", "sparkling"],
    },
    Synonym {
        words: "spirit",
        kinds: &[Kind::Spirits],
        phrases: &[],
    },
    Synonym {
        words: "shot",
        kinds: &[Kind::Spirits, Kind::Cocktails],
        phrases: &["shot"],
    },
    Synonym {
        words: "soft drink",
        kinds: &[Kind::Soft],
        phrases: &["soft drink"],
    },
    Synonym {
        words: "soft",
        kinds: &[Kind::Soft],
        phrases: &["soft drink"],
    },
    Synonym {
        words: "soda",
        kinds: &[Kind::Soft],
        phrases: &["soft drink"],
    },
    Synonym {
        words: "fizzy",
        kinds: &[Kind::Soft],
        phrases: &["soft drink"],
    },
    Synonym {
        words: "juice",
        kinds: &[Kind::Soft],
        phrases: &[],
    },
    Synonym {
        words: "hot drink",
        kinds: &[Kind::Hot],
        phrases: &["hot drink"],
    },
    Synonym {
        words: "coffee",
        kinds: &[Kind::Hot],
        phrases: &["coffee"],
    },
    Synonym {
        words: "tea",
        kinds: &[Kind::Hot],
        phrases: &["tea"],
    },
    Synonym {
        words: "food",
        kinds: &[Kind::Food],
        phrases: &[],
    },
    Synonym {
        words: "snack",
        kinds: &[Kind::Food],
        phrases: &["bar snacks"],
    },
];

/// A query word, or two words a synonym knows as one.
struct Term {
    words: Vec<String>,
    synonym: Option<&'static Synonym>,
}

fn stem(word: &str) -> &str {
    if word.len() > 3 && word.ends_with("es") && !word.ends_with("ses") {
        &word[..word.len() - 2]
    } else if word.len() > 3 && word.ends_with('s') && !word.ends_with("ss") {
        &word[..word.len() - 1]
    } else {
        word
    }
}

fn terms(query: &str, stopwords: &[&str]) -> Vec<Term> {
    let words: Vec<String> = normalise(query)
        .split(' ')
        // Bare numbers are quantities and table numbers ("4 pints of Abbot"), not names.
        .filter(|w| {
            !w.is_empty() && !stopwords.contains(w) && !w.chars().all(|c| c.is_ascii_digit())
        })
        .map(str::to_owned)
        .collect();
    let synonym = |text: &str| {
        SYNONYMS
            .iter()
            .find(|s| s.words == text || s.words == stem(text))
    };
    let mut out = Vec::new();
    let mut i = 0;
    while i < words.len() {
        if let Some(next) = words.get(i + 1) {
            let pair = format!("{} {}", stem(&words[i]), stem(next));
            if let Some(found) = SYNONYMS.iter().find(|s| s.words == pair) {
                out.push(Term {
                    words: vec![words[i].clone(), next.clone()],
                    synonym: Some(found),
                });
                i += 2;
                continue;
            }
        }
        out.push(Term {
            words: vec![words[i].clone()],
            synonym: synonym(&words[i]),
        });
        i += 1;
    }
    out
}

/// Edit distance, for short words only: the callers bound it.
pub fn levenshtein(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.iter().enumerate() {
        let mut previous = row[0];
        row[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let substitution = previous + usize::from(ca != cb);
            previous = row[j + 1];
            row[j + 1] = substitution.min(row[j] + 1).min(row[j + 1] + 1);
        }
    }
    row[b.len()]
}

/// American Soundex: how a word sounds, near enough for "broccoli" to find "Brockley".
pub fn soundex(word: &str) -> String {
    let code = |c: char| match c {
        'b' | 'f' | 'p' | 'v' => '1',
        'c' | 'g' | 'j' | 'k' | 'q' | 's' | 'x' | 'z' => '2',
        'd' | 't' => '3',
        'l' => '4',
        'm' | 'n' => '5',
        'r' => '6',
        _ => '0',
    };
    let mut chars = word.chars().filter(char::is_ascii_alphabetic);
    let Some(first) = chars.next() else {
        return String::new();
    };
    let mut out = String::from(first);
    let mut last = code(first);
    for c in chars {
        let digit = code(c);
        if digit != '0' && digit != last {
            out.push(digit);
            if out.len() == 4 {
                break;
            }
        }
        if c != 'h' && c != 'w' {
            last = digit;
        }
    }
    while out.len() < 4 {
        out.push('0');
    }
    out
}

/// How well one query word matches one word of a field, from 0 to 1.
fn word_score(query: &str, word: &str) -> f32 {
    let (q, w) = (stem(query), stem(word));
    if q == w {
        1.0
    } else if q.len() >= 2 && w.starts_with(q) {
        0.85
    } else if q.len() >= 3 && w.contains(q) {
        0.7
    } else if q.len() >= 4 && w.len() >= 3 && levenshtein(q, w) <= if q.len() >= 7 { 2 } else { 1 }
    {
        0.6
    } else if q.len() >= 4
        && w.len() >= 4
        && q.chars().zip(w.chars()).take_while(|(a, b)| a == b).count() >= 3
        && soundex(q) == soundex(w)
    {
        // Sounds alike and starts alike: "broccoli" is "Brockley", but "stella" isn't "style".
        0.45
    } else {
        0.0
    }
}

/// How well a term matches a field's text: each of its words against the field's best word.
fn field_score(term: &Term, field: &str) -> f32 {
    let words: Vec<&str> = field.split(' ').collect();
    let total: f32 = term
        .words
        .iter()
        .map(|q| words.iter().map(|w| word_score(q, w)).fold(0.0, f32::max))
        .sum();
    let mean = total / term.words.len() as f32;
    // A two-word term counts only when both words are there.
    if term.words.len() > 1
        && term
            .words
            .iter()
            .any(|q| words.iter().all(|w| word_score(q, w) == 0.0))
    {
        0.0
    } else {
        mean
    }
}

/// How well every word of `query` is found in `text`, both normalised: 0 when any word is missing, else the mean
/// of each word's best match. For short labels such as an option's name.
pub fn text_score(query: &str, text: &str) -> f32 {
    let words: Vec<&str> = text.split(' ').collect();
    let query: Vec<&str> = query.split(' ').filter(|w| !w.is_empty()).collect();
    if query.is_empty() {
        return 0.0;
    }
    let mut total = 0.0;
    for q in &query {
        let best = words.iter().map(|w| word_score(q, w)).fold(0.0, f32::max);
        if best == 0.0 {
            return 0.0;
        }
        total += best;
    }
    total / query.len() as f32
}

/// Text to search, with its weight.
pub struct Field {
    pub text: String,
    pub weight: f32,
}

/// What a candidate brings to a search: its fields, and its kind for synonyms.
pub struct Candidate {
    pub fields: Vec<Field>,
    /// What it is: one kind, or two for an alcohol-free lager, which is both.
    pub kinds: Vec<Kind>,
    /// The texts a synonym's phrases are looked for in: the pub's own category and group names.
    pub categories: Vec<String>,
}

#[derive(Debug, Clone, Copy)]
pub struct Score {
    pub total: f32,
    /// Every word of the query found something.
    pub complete: bool,
    pub matched: usize,
}

fn score(terms: &[Term], candidate: &Candidate) -> Score {
    let mut total = 0.0;
    let mut matched = 0;
    for term in terms {
        let direct = candidate
            .fields
            .iter()
            .map(|f| f.weight * field_score(term, &f.text))
            .fold(0.0, f32::max);
        let (kind_hit, phrase_hit) = match term.synonym {
            Some(s) => (
                candidate.kinds.iter().any(|k| s.kinds.contains(k)),
                s.phrases.iter().any(|p| {
                    candidate
                        .categories
                        .iter()
                        .any(|c| c.contains(&normalise(p)))
                }),
            ),
            None => (false, false),
        };
        // A synonym's phrase in the pub's own category is as good as the word in the name; its kind alone a little
        // less; and either on top of a direct match puts the thing meant above the thing merely named ("cocktail"
        // finds cocktails before prawn cocktail crisps).
        let mut term_score = direct;
        if phrase_hit {
            term_score = term_score.max(3.0);
        } else if kind_hit {
            term_score = term_score.max(2.0);
        }
        if term_score > 0.0 && (kind_hit || phrase_hit) {
            term_score += 1.0;
        }
        if term_score > 0.0 {
            matched += 1;
        }
        total += term_score;
    }
    Score {
        total,
        complete: matched == terms.len(),
        matched,
    }
}

/// Ranked candidates: those matching every word, best first, or else the nearest, flagged as close.
pub struct Ranked<T> {
    pub items: Vec<(f32, T)>,
    pub close: bool,
}

fn rank<T>(query: &str, stopwords: &[&str], items: Vec<(T, Candidate)>) -> Option<Ranked<T>> {
    let terms = terms(query, stopwords);
    if terms.is_empty() {
        return None;
    }
    let scored: Vec<(Score, T)> = items
        .into_iter()
        .map(|(item, c)| (score(&terms, &c), item))
        .collect();
    let complete = scored.iter().any(|(s, _)| s.complete);
    let mut kept: Vec<(Score, T)> = if complete {
        scored.into_iter().filter(|(s, _)| s.complete).collect()
    } else {
        // Nothing has every word: the ones with the most, if any have one.
        let most = scored.iter().map(|(s, _)| s.matched).max().unwrap_or(0);
        scored
            .into_iter()
            .filter(|(s, _)| most > 0 && s.matched == most)
            .collect()
    };
    kept.sort_by(|a, b| b.0.total.total_cmp(&a.0.total));
    Some(Ranked {
        items: kept.into_iter().map(|(s, t)| (s.total, t)).collect(),
        close: !complete,
    })
}

/// A product's fields, weighted: its name above all, then the pub's own category and group for it, its kind and
/// style, then its notes and description.
pub fn product(entry: &Entry) -> Candidate {
    let field = |text: &str, weight: f32| Field {
        text: normalise(text),
        weight,
    };
    let mut fields = vec![
        field(&entry.product.name, 3.0),
        field(&entry.category, 2.0),
        field(entry.kind.label(), 2.0),
        field(&entry.notes.join(" "), 1.0),
        field(entry.product.description.as_deref().unwrap_or(""), 1.0),
    ];
    for extra in [entry.brewery.as_deref(), entry.style.as_deref()]
        .into_iter()
        .flatten()
    {
        fields.push(field(extra, 1.5));
    }
    if let Some(group) = &entry.group {
        fields.push(field(group, 2.0));
    }
    if let Some(origin) = &entry.origin {
        fields.push(field(origin, 1.0));
    }
    Candidate {
        fields,
        kinds: [Some(entry.kind), entry.also]
            .into_iter()
            .flatten()
            .collect(),
        categories: [Some(entry.category.as_str()), entry.group.as_deref()]
            .into_iter()
            .flatten()
            .map(normalise)
            .collect(),
    }
}

pub fn products<'a>(
    query: &str,
    entries: impl Iterator<Item = &'a Entry>,
) -> Option<Ranked<&'a Entry>> {
    rank(
        query,
        PRODUCT_STOPWORDS,
        entries.map(|e| (e, product(e))).collect(),
    )
}

/// A pub's fields: its name, then where it is.
pub fn pub_candidate(venue: &spoons_api_client::VenueSummary) -> Candidate {
    let a = &venue.address;
    let field = |text: Option<&String>, weight: f32| Field {
        text: normalise(text.map_or("", String::as_str)),
        weight,
    };
    let postcode = a.postcode.as_deref().unwrap_or("");
    Candidate {
        fields: vec![
            field(Some(&venue.name), 3.0),
            field(a.town.as_ref(), 2.0),
            field(a.line2.as_ref(), 1.5),
            field(a.line3.as_ref(), 1.5),
            field(a.county.as_ref(), 1.0),
            // "SE4 2RR", "se4" and "se42rr" all find it.
            Field {
                text: format!(
                    "{} {}",
                    normalise(postcode),
                    normalise(postcode).replace(' ', "")
                ),
                weight: 2.0,
            },
        ],
        kinds: Vec::new(),
        categories: Vec::new(),
    }
}

pub fn pubs<'a>(
    query: &str,
    venues: impl Iterator<Item = &'a spoons_api_client::VenueSummary>,
) -> Option<Ranked<&'a spoons_api_client::VenueSummary>> {
    rank(
        query,
        PUB_STOPWORDS,
        venues.map(|v| (v, pub_candidate(v))).collect(),
    )
}

/// Whether one of the pub's own category names is the one asked for: its words, forgivingly, or a synonym's
/// phrase ("mocktails" is "0% cocktails").
pub fn category_matches(wanted: &str, name: &str) -> bool {
    let name = normalise(name);
    let wanted_terms = terms(wanted, PRODUCT_STOPWORDS);
    if wanted_terms.is_empty() || name.contains(&normalise(wanted)) {
        return true;
    }
    // A word with a synonym means the synonym: "mocktails" is one letter from "cocktails" but not the same thing.
    wanted_terms.iter().all(|term| match term.synonym {
        Some(s) if !s.phrases.is_empty() => s.phrases.iter().any(|p| name.contains(&normalise(p))),
        _ => field_score(term, &name) > 0.0,
    })
}

/// Whether a product passes a filter on the menu: every word of the query found something.
pub fn product_matches(query: &str, entry: &Entry) -> bool {
    let terms = terms(query, PRODUCT_STOPWORDS);
    terms.is_empty() || score(&terms, &product(entry)).complete
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalises_brand_styling() {
        assert_eq!(normalise("Mad Squirrel $umo"), "mad squirrel sumo");
        assert_eq!(normalise("Rooster’s – Scoundrel®"), "roosters scoundrel");
        assert_eq!(normalise("1664 Bière"), "1664 biere");
        assert_eq!(
            normalise("Guinness 0.0 - Alcohol free"),
            "guinness 0.0 alcohol free"
        );
        assert_eq!(normalise("Gin & Tonic"), "gin and tonic");
    }

    #[test]
    fn hears_misheard_words() {
        assert_eq!(soundex("broccoli"), soundex("brockley"));
        assert_eq!(soundex("robert"), "r163");
        assert!(word_score("broccoli", "brockley") > 0.0);
        assert!(word_score("abot", "abbot") > 0.0);
        assert_eq!(word_score("cat", "dog"), 0.0);
        assert_eq!(word_score("stella", "style"), 0.0);
    }

    #[test]
    fn knows_two_word_synonyms() {
        let t = terms("an alcohol free beer please", PRODUCT_STOPWORDS);
        assert_eq!(t.len(), 2);
        assert_eq!(t[0].synonym.unwrap().words, "alcohol free");
        assert_eq!(t[1].synonym.unwrap().words, "beer");
    }
}
