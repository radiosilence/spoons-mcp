//! Entering a card on the Axepta payment page, as the page's own script does, so a payment can be made without a
//! browser.
//!
//! The page is a form of hidden fields describing the payment; its script adds the card and posts the lot to
//! `payinterim.aspx`. What follows is 3-D Secure: redirects and pages that submit themselves, which are followed
//! here, and possibly a challenge in the card holder's banking app. Whether the payment went through is the payment
//! API's to say ([`crate::Payments::status`]), not anything these pages show.

use std::fmt;
use std::sync::LazyLock;
use std::time::Duration;

use regex::Regex;
use reqwest::header::{CONTENT_TYPE, HeaderValue, ORIGIN, REFERER};
use reqwest::{Method, StatusCode, Url, redirect};

/// Only pages under here are fetched with a card, and only https ones after it.
pub const PAYMENT_PAGE: &str = "https://paymentpage.axepta.bnpparibas/";
/// The payment page lays itself out for the browser it sees, and 3-D Secure scores it, so this is an ordinary phone.
const USER_AGENT: &str = "Mozilla/5.0 (iPhone; CPU iPhone OS 26_0 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like \
                          Gecko) Version/26.0 Mobile/15E148 Safari/604.1";
const MAX_HOPS: usize = 12;

/// A card as the payment page takes it. Its `Debug` hides everything but the last four digits.
#[derive(Clone)]
pub struct Card {
    number: String,
    month: String,
    year: String,
    cvc: String,
    holder: String,
}

impl fmt::Debug for Card {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Card(…{})", &self.number[self.number.len() - 4..])
    }
}

impl Card {
    /// From the number, an expiry such as `07/29` or `07/2029`, the CVC and the name on the card.
    pub fn new(number: &str, expiry: &str, cvc: &str, holder: &str) -> Result<Self, String> {
        let number: String = number
            .chars()
            .filter(|c| !c.is_whitespace() && *c != '-')
            .collect();
        if !(12..=19).contains(&number.len()) || !number.bytes().all(|b| b.is_ascii_digit()) {
            return Err("the card number must be 12 to 19 digits".into());
        }
        let (month, year) = expiry
            .trim()
            .split_once('/')
            .ok_or("the expiry must look like MM/YY")?;
        let (month, year) = (month.trim(), year.trim());
        let month_ok = month.parse::<u8>().is_ok_and(|m| (1..=12).contains(&m));
        let year_ok = matches!(year.len(), 2 | 4) && year.bytes().all(|b| b.is_ascii_digit());
        if !month_ok || !year_ok {
            return Err("the expiry must look like MM/YY".into());
        }
        let cvc = cvc.trim();
        if !(3..=4).contains(&cvc.len()) || !cvc.bytes().all(|b| b.is_ascii_digit()) {
            return Err("the CVC must be 3 or 4 digits".into());
        }
        let holder = holder.trim();
        if holder.is_empty() {
            return Err("the name on the card is missing".into());
        }
        Ok(Self {
            number,
            month: format!("{month:0>2}"),
            year: if year.len() == 2 {
                format!("20{year}")
            } else {
                year.to_owned()
            },
            cvc: cvc.to_owned(),
            holder: holder.to_owned(),
        })
    }

    /// As the page's brand picker names it.
    fn brand(&self) -> &'static str {
        match self.number.as_bytes()[0] {
            b'4' => "VISA",
            b'3' => "AMEX",
            _ => "MasterCard",
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum CardError {
    /// The card was never sent, so the payment page is untouched and can still be paid in a browser.
    #[error("the card wasn't sent: {0}")]
    NotSent(String),
    /// The card was sent, or may have been, and then something failed. Whether it was charged is the payment API's
    /// to say.
    #[error("the card was sent, then {0}")]
    AfterSending(String),
}

/// Where entering the card got to: the pages visited, without their queries, which can carry the payment's details.
#[derive(Debug)]
pub struct Entered {
    pub hops: Vec<String>,
}

/// Enters the card on a payment page and follows 3-D Secure as far as it goes without a person. Sent once: the
/// card is never posted twice, whatever happens after.
pub async fn enter(url: &str, card: &Card) -> Result<Entered, CardError> {
    if !url.starts_with(PAYMENT_PAGE) {
        return Err(CardError::NotSent(format!(
            "{url} isn't an Axepta payment page"
        )));
    }
    let not_sent = |error: reqwest::Error| CardError::NotSent(error.to_string());
    let http = reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .cookie_store(true)
        .redirect(redirect::Policy::none())
        .timeout(Duration::from_secs(60))
        .build()
        .map_err(not_sent)?;
    let page_url = Url::parse(url).map_err(|e| CardError::NotSent(e.to_string()))?;
    let response = http.get(page_url.clone()).send().await.map_err(not_sent)?;
    if !response.status().is_success() {
        return Err(CardError::NotSent(format!(
            "the payment page answered {}",
            response.status()
        )));
    }
    let page = response.text().await.map_err(not_sent)?;
    let mut fields = inputs(&page);
    if !fields.iter().any(|(name, _)| name == "MerchantID") {
        return Err(CardError::NotSent(
            "the payment page has no payment form".into(),
        ));
    }
    fields.push((
        "binfo".into(),
        format!(
            "javaEnabled=false&colorDepth=24&screenHeight=844&screenWidth=390&timeZoneOffset=0&userAgent={USER_AGENT}\
             &browserLanguage=en-GB"
        ),
    ));
    fields.extend([
        ("cardNumber".into(), card.number.clone()),
        ("cardExpirationMonth".into(), card.month.clone()),
        ("cardExpirationYear".into(), card.year.clone()),
        ("cccvc".into(), card.cvc.clone()),
        ("KKName".into(), card.brand().into()),
        ("CreditCardHolder".into(), card.holder.clone()),
    ]);
    let post = page_url
        .join("payinterim.aspx")
        .map_err(|e| CardError::NotSent(e.to_string()))?;

    let after = |error: String| CardError::AfterSending(error);
    let mut hops = vec![strip(&page_url)];
    let mut request = Some((Method::POST, post, Some(fields), page_url));
    while let Some((method, url, form, referer)) = request.take() {
        if hops.len() > MAX_HOPS {
            return Err(after(format!(
                "3-D Secure went on for more than {MAX_HOPS} pages"
            )));
        }
        if url.scheme() != "https" {
            return Err(after(format!(
                "3-D Secure led to a page that isn't https ({})",
                strip(&url)
            )));
        }
        hops.push(strip(&url));
        let mut builder = http
            .request(method.clone(), url.clone())
            .header(REFERER, referer.as_str());
        if let Ok(origin) = HeaderValue::from_str(&referer.origin().ascii_serialization()) {
            builder = builder.header(ORIGIN, origin);
        }
        if let Some(form) = &form {
            builder = builder
                .header(CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(encode(form));
        }
        let response = builder.send().await.map_err(|e| after(e.to_string()))?;
        let status = response.status();
        tracing::info!(%status, page = strip(&url), "3-D Secure");
        if status.is_redirection() {
            let location = response
                .headers()
                .get("location")
                .and_then(|value| value.to_str().ok())
                .ok_or_else(|| after(format!("{} redirected nowhere", strip(&url))))?;
            let next = url.join(location).map_err(|e| after(e.to_string()))?;
            let keep = matches!(
                status,
                StatusCode::TEMPORARY_REDIRECT | StatusCode::PERMANENT_REDIRECT
            );
            request = Some(if keep {
                (method, next, form, url)
            } else {
                (Method::GET, next, None, url)
            });
            continue;
        }
        if !status.is_success() {
            return Err(after(format!("{} answered {status}", strip(&url))));
        }
        let body = response.text().await.map_err(|e| after(e.to_string()))?;
        if let Some((action, method, form)) = auto_form(&body) {
            let next = url.join(&action).map_err(|e| after(e.to_string()))?;
            request = Some(if method == Method::GET {
                let mut next = next;
                next.query_pairs_mut().extend_pairs(&form);
                (Method::GET, next, None, url)
            } else {
                (Method::POST, next, Some(form), url)
            });
        }
    }
    Ok(Entered { hops })
}

fn strip(url: &Url) -> String {
    format!("{}{}", url.origin().ascii_serialization(), url.path())
}

fn encode(form: &[(String, String)]) -> String {
    let mut out = url::form_urlencoded::Serializer::new(String::new());
    out.extend_pairs(form);
    out.finish()
}

static INPUT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?is)<input\b[^>]*>").unwrap());
static FORM: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?is)<form\b([^>]*)>(.*?)</form>").unwrap());
static SUBMITS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?is)\.submit\s*\(\s*\)|<body[^>]*\bonload\s*=").unwrap());

fn attribute(tag: &str, name: &str) -> Option<String> {
    let pattern = format!(r#"(?is)\b{name}\s*=\s*(?:"([^"]*)"|'([^']*)'|([^\s>]+))"#);
    let captures = Regex::new(&pattern).ok()?.captures(tag)?;
    let value = captures.get(1).or(captures.get(2)).or(captures.get(3))?;
    Some(unescape(value.as_str()))
}

type Form = Vec<(String, String)>;

/// Every named input in a piece of HTML, in order, as a form would post them.
fn inputs(html: &str) -> Form {
    INPUT
        .find_iter(html)
        .filter_map(|tag| {
            let tag = tag.as_str();
            let name = attribute(tag, "name")?;
            Some((name, attribute(tag, "value").unwrap_or_default()))
        })
        .collect()
}

/// The form a page submits by itself, as 3-D Secure's interstitial pages do: its action, method and fields.
fn auto_form(html: &str) -> Option<(String, Method, Form)> {
    if !SUBMITS.is_match(html) {
        return None;
    }
    let form = FORM.captures(html)?;
    let tag = form.get(1)?.as_str();
    let method = match attribute(tag, "method").map(|m| m.to_ascii_uppercase()) {
        Some(m) if m == "GET" => Method::GET,
        _ => Method::POST,
    };
    Some((
        attribute(tag, "action").unwrap_or_default(),
        method,
        inputs(form.get(2)?.as_str()),
    ))
}

fn unescape(text: &str) -> String {
    if !text.contains('&') {
        return text.to_owned();
    }
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find('&') {
        out.push_str(&rest[..at]);
        rest = &rest[at..];
        let Some(end) = rest[..rest.len().min(10)].find(';') else {
            out.push('&');
            rest = &rest[1..];
            continue;
        };
        let entity = &rest[1..end];
        let decoded = match entity {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            _ => entity
                .strip_prefix("#x")
                .or_else(|| entity.strip_prefix("#X"))
                .and_then(|hex| u32::from_str_radix(hex, 16).ok())
                .or_else(|| entity.strip_prefix('#').and_then(|dec| dec.parse().ok()))
                .and_then(char::from_u32),
        };
        match decoded {
            Some(c) => {
                out.push(c);
                rest = &rest[end + 1..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn takes_a_card_as_people_write_it() {
        let card = Card::new("4111 1111 1111 1111", "7/29", "123", " A Person ").unwrap();
        assert_eq!((card.month.as_str(), card.year.as_str()), ("07", "2029"));
        assert_eq!(card.brand(), "VISA");
        assert_eq!(format!("{card:?}"), "Card(…1111)");
        assert!(Card::new("4111", "07/29", "123", "A").is_err());
        assert!(Card::new("4111111111111111", "13/29", "123", "A").is_err());
        assert!(Card::new("4111111111111111", "0729", "123", "A").is_err());
        assert!(Card::new("4111111111111111", "07/29", "12", "A").is_err());
    }

    #[test]
    fn reads_the_payment_form() {
        let page = r#"<form><input type="hidden" name="MerchantID" value="BNP_JDW_UK_ECOM">
            <input type=hidden name='Data' value='a&amp;b&#x3D;'><input name="Counter" value=""></form>"#;
        assert_eq!(
            inputs(page),
            [
                ("MerchantID".into(), "BNP_JDW_UK_ECOM".into()),
                ("Data".into(), "a&b=".into()),
                ("Counter".into(), String::new())
            ]
        );
    }

    #[test]
    fn follows_only_forms_that_submit_themselves() {
        let interstitial = r#"<body onload="document.forms[0].submit()"><form method="post"
            action="https://acs.example/challenge"><input name="creq" value="eyJ"></form></body>"#;
        let (action, method, form) = auto_form(interstitial).unwrap();
        assert_eq!(action, "https://acs.example/challenge");
        assert_eq!(method, Method::POST);
        assert_eq!(form, [("creq".into(), "eyJ".into())]);
        assert!(auto_form(r#"<form action="x"><input name="a"></form>"#).is_none());
    }
}
