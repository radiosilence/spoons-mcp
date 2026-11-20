//! The graph against a mock of Wetherspoon's two APIs, built from fixtures trimmed from a real dump of The Watch
//! House (venue 254), with a made-up garden (sales area 444, tables 50 and 51) whose prices are 30p dearer. Nothing
//! here reaches the live API, and the payment API is a mock that counts what it is asked: the point of most of
//! these tests is what is never sent.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{Value, json};
use spoons_api_client::Client;
use wiremock::matchers::{method, path, path_regex};
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

use super::Caller;
use crate::mcp::Spoons;
use crate::orders::Store;
use crate::service::Service;

const ABBOT: &str = "10000000315";
const CARLING: &str = "10000000292";
const TRANSACTION: &str = "025420261120ABCDEF";

fn fixture(name: &str) -> Value {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name);
    serde_json::from_slice(
        &std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display())),
    )
    .unwrap()
}

/// The till: every item at £3.70, which isn't the menu's price for anything, so a quote always differs.
struct Till;

impl Respond for Till {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        let body: Value = serde_json::from_slice(&request.body).unwrap();
        let lines = body["items"].as_array().unwrap();
        // As the real till does, a size whose required choices weren't sent is "unavailable".
        for line in lines {
            for asked in line["portion"]["choices"].as_array().into_iter().flatten() {
                let group = asked["choiceId"].to_string();
                if asked["required"] == true && line["choices"].get(&group).is_none() {
                    return ResponseTemplate::new(400).set_body_json(json!({ "success": false,
                        "message": format!("{} is currently unavailable, please make another selection. (Z216)",
                            line["product"]["name"].as_str().unwrap()) }));
                }
            }
        }
        let items: Vec<Value> = lines
            .iter()
            .map(|line| {
                let quantity = line["quantity"].as_u64().unwrap();
                // As the real till does, a guest ale's long id comes back cut to its first eleven digits.
                let id = line["product"]["id"].to_string();
                let id: u64 = id[..id.len().min(11)].parse().unwrap();
                json!({ "id": id, "quantity": quantity,
                        "price": { "value": 3.70 * quantity as f64 } })
            })
            .collect();
        let mut total: f64 = items
            .iter()
            .map(|i| i["price"]["value"].as_f64().unwrap())
            .sum();
        // A drink ordered through a meal's "Includes a drink" (category 400271 here) takes £1.96 off, as the till did
        // for a pudding and a Carling.
        let deals = lines
            .iter()
            .filter(|l| l["product"]["category"]["id"] == 400271)
            .map(|l| l["quantity"].as_f64().unwrap())
            .sum::<f64>();
        let saved = 1.96 * deals;
        total -= saved;
        ResponseTemplate::new(200).set_body_json(json!({ "data": {
            "id": "basket-1", "total": { "value": (total * 100.0).round() / 100.0, "currency": "GBP" },
            "promotionsTotal": { "value": -saved, "currency": "GBP" },
            "promotions": if deals > 0.0 { json!([{ "id": 1, "amount": { "value": -saved }, "isEmployeeDiscount": false }]) } else { json!([]) },
            "items": items, "token": "signed-by-the-api",
        } }))
    }
}

struct Harness {
    server: MockServer,
    spoons: Spoons,
    _state: TempDir,
}

struct TempDir(PathBuf);

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn caller() -> Caller {
    Caller {
        email: Some("drinker@example.com".into()),
        card: Ok(None),
    }
}

async fn harness() -> Harness {
    let server = MockServer::start().await;
    let api = "/api/v0.1";
    let venue = format!("{api}/jdw/venues/254");
    let routes: Vec<(String, Value)> = vec![
        (format!("{api}/venues"), fixture("venues.json")),
        (venue.clone(), fixture("venue.json")),
        (format!("{venue}/ales"), fixture("ales.json")),
        (format!("{venue}/wait-times"), fixture("wait-times.json")),
        (format!("{venue}/tables"), fixture("tables.json")),
        (
            format!("{venue}/sales-areas/443/menus"),
            fixture("menus-443.json"),
        ),
        (
            format!("{venue}/sales-areas/444/menus"),
            fixture("menus-444.json"),
        ),
    ];
    for (route, body) in routes {
        Mock::given(method("GET"))
            .and(path(route))
            .respond_with(ResponseTemplate::new(200).set_body_json(body))
            .mount(&server)
            .await;
    }
    for (area, menu) in [
        (443, "34585"),
        (443, "13385"),
        (443, "24203"),
        (443, "34586"),
        (444, "34585"),
    ] {
        Mock::given(method("GET"))
            .and(path(format!("{venue}/sales-areas/{area}/menus/{menu}")))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(fixture(&format!("menu-{area}-{menu}.json"))),
            )
            .mount(&server)
            .await;
    }
    Mock::given(method("POST"))
        .and(path_regex(r"/basket/validate$"))
        .respond_with(Till)
        .mount(&server)
        .await;
    // The payment API's status route answers 400 with the state until the order is placed.
    Mock::given(method("GET"))
        .and(path(format!(
            "/pay/jdw/venues/254/payment/{TRANSACTION}/status"
        )))
        .respond_with(
            ResponseTemplate::new(400).set_body_json(json!({ "success": false, "data": {
            "paymentStatus": "pending", "orderStatus": "pending", "transactionId": TRANSACTION,
        } })),
        )
        .mount(&server)
        .await;

    let dir = std::env::temp_dir().join(format!("spoons-graphql-{}", uuid::Uuid::new_v4()));
    let store = Store::at(dir.clone()).unwrap();
    let client = Client::new("test-key", &format!("{}{api}", server.uri()), "6.9.1")
        .unwrap()
        .with_interval(Duration::ZERO)
        .with_payments_base(&format!("{}/pay", server.uri()));
    let payments = client.payments(&store.device_id().unwrap()).unwrap();
    let service = Arc::new(
        Service::new(client, payments, store)
            .with_poll(Duration::from_millis(10), Duration::from_millis(50)),
    );
    let spoons = Spoons::new(service, super::schema(), None);
    Harness {
        server,
        spoons,
        _state: TempDir(dir),
    }
}

impl Harness {
    async fn run(&self, query: &str, variables: Value) -> Value {
        self.run_as(&self.spoons, query, variables).await
    }

    async fn run_as(&self, spoons: &Spoons, query: &str, variables: Value) -> Value {
        let response = spoons
            .execute(caller(), query, Some(&variables.to_string()))
            .await
            .unwrap();
        assert!(response.errors.is_empty(), "{query}: {:?}", response.errors);
        response.data.into_json().unwrap()
    }

    /// How many requests reached a route.
    async fn hits(&self, method_name: &str, suffix: &str) -> usize {
        self.server
            .received_requests()
            .await
            .unwrap()
            .iter()
            .filter(|r| r.method.as_str() == method_name && r.url.path().ends_with(suffix))
            .count()
    }

    /// The payment API as it is once `pay` has prepared a payment.
    async fn mount_prepare(&self, duplicate: bool) {
        Mock::given(method("POST"))
            .and(path("/pay/jdw/venues/254/orders/prepare/card"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "success": true, "data": {
                "transactionId": TRANSACTION,
                "url": "https://paymentpage.axepta.bnpparibas/payssl.aspx?MerchantID=x&Len=1&Data=y",
                "duplicateOrder": duplicate,
            } })))
            .mount(&self.server)
            .await;
    }

    async fn four_abbots_to_table_four(&self) -> String {
        let added = self
            .run(
                "mutation($p: ID!) { basketAdd(pub: 254, table: 4, productId: $p, portion: \"pint\", quantity: 4) \
                 { success error } quote { success error quote { id } } }",
                json!({ "p": ABBOT }),
            )
            .await;
        assert_eq!(added["basketAdd"]["success"], true, "{added}");
        assert_eq!(added["quote"]["success"], true, "{added}");
        added["quote"]["quote"]["id"].as_str().unwrap().to_owned()
    }
}

#[test]
fn schema_matches_the_snapshot() {
    // `UPDATE_SNAPSHOTS=1 cargo test` rewrites it after an intended change, for the diff to show in review.
    let file = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("schema.graphql");
    let sdl = super::sdl();
    if std::env::var_os("UPDATE_SNAPSHOTS").is_some() {
        std::fs::write(&file, &sdl).unwrap();
    }
    let committed = std::fs::read_to_string(&file).unwrap_or_default();
    assert!(
        committed == sdl,
        "schema.graphql is stale: run `UPDATE_SNAPSHOTS=1 cargo test` and review the diff"
    );
}

#[tokio::test]
async fn lists_the_nearest_pubs_first() {
    let h = harness().await;
    let data = h
        .run(
            "{ pubs(near: {lat: 51.4599, lng: -0.0116}, first: 2) { totalCount nodes { ref name distanceMetres } \
               pageInfo { hasNextPage endCursor } } }",
            json!({}),
        )
        .await;
    assert_eq!(data["pubs"]["nodes"][0]["name"], "The Watch House");
    assert!(data["pubs"]["nodes"][0]["distanceMetres"].as_f64().unwrap() < 100.0);
    assert_eq!(data["pubs"]["pageInfo"]["hasNextPage"], true);
    assert_eq!(
        data["pubs"]["pageInfo"]["endCursor"],
        data["pubs"]["nodes"][1]["ref"].to_string()
    );
    let found = h
        .run("{ pubs(query: \"brockley\") { nodes { ref } } }", json!({}))
        .await;
    assert_eq!(found["pubs"]["nodes"], json!([{ "ref": 1006 }]));
}

#[tokio::test]
async fn the_whole_menu_is_there_sorted_into_kinds() {
    let h = harness().await;
    let data = h
        .run(
            "{ pub(ref: 254) { menu { totalCount sections { kind count } \
               guestAles { name brewery shortName abv colour { hex source } portions { name price { formatted } } } \
               comingSoon { fullName } } } }",
            json!({}),
        )
        .await;
    let menu = &data["pub"]["menu"];
    // The old search stopped at fifty.
    assert!(
        menu["totalCount"].as_u64().unwrap() > 100,
        "{}",
        menu["totalCount"]
    );
    let kinds: Vec<&str> = menu["sections"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["kind"].as_str().unwrap())
        .collect();
    for kind in [
        "CASK_ALE",
        "LAGER",
        "STOUT",
        "CIDER",
        "WINE",
        "SPIRITS",
        "ALCOHOL_FREE",
        "SOFT",
        "HOT",
        "FOOD",
    ] {
        assert!(kinds.contains(&kind), "{kind} missing from {kinds:?}");
    }
    assert_eq!(kinds[0], "CASK_ALE");
    let guests = menu["guestAles"].as_array().unwrap();
    let names: Vec<&str> = guests
        .iter()
        .map(|g| g["shortName"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["Scoundrel", "Broadsword", "Moose River"]);
    assert_eq!(
        guests[1]["colour"],
        json!({ "hex": "#63130e", "source": "ALE_LIST" })
    );
    assert_eq!(
        guests[0]["portions"][0],
        json!({ "name": "Third pint", "price": { "formatted": "£1.13" } })
    );
    assert_eq!(menu["comingSoon"].as_array().unwrap().len(), 2);
}

#[tokio::test]
async fn searches_filters_and_explains_what_cannot_be_ordered() {
    let h = harness().await;
    let data = h
        .run(
            "{ pub(ref: 254) { menu { abbot: products(search: \"abbot\") { nodes { id kind tasteNotes } } \
               af: products(kinds: [ALCOHOL_FREE]) { totalCount nodes { name orderable whyNot } } \
               page: products(first: 2) { nodes { id } pageInfo { endCursor } } } } }",
            json!({}),
        )
        .await;
    let menu = &data["pub"]["menu"];
    assert_eq!(
        menu["abbot"]["nodes"],
        json!([{ "id": ABBOT, "kind": "CASK_ALE", "tasteNotes": ["malty", "bittersweet"] }])
    );
    let af = menu["af"]["nodes"].as_array().unwrap();
    let pink = af
        .iter()
        .find(|p| p["name"].as_str().unwrap().starts_with("Gordons"))
        .unwrap();
    // A mixer to choose no longer stops it: the menu says what to ask.
    assert_eq!(pink["orderable"], true);
    assert_eq!(pink["whyNot"], Value::Null);
    let after = menu["page"]["pageInfo"]["endCursor"].as_str().unwrap();
    let next = h
        .run(
            "query($a: String) { pub(ref: 254) { menu { products(first: 1, after: $a) { nodes { id } } } } }",
            json!({ "a": after }),
        )
        .await;
    assert_ne!(
        next["pub"]["menu"]["products"]["nodes"][0]["id"],
        menu["page"]["nodes"][0]["id"]
    );
}

#[tokio::test]
async fn reads_a_sales_areas_menu_once() {
    let h = harness().await;
    let query = "{ pub(ref: 254) { a: menu { totalCount } b: menu(search: \"ale\") { totalCount } ales { name } } }";
    h.run(query, json!({})).await;
    h.run(query, json!({})).await;
    assert_eq!(h.hits("GET", "/sales-areas/443/menus").await, 1);
    assert_eq!(h.hits("GET", "/sales-areas/443/menus/34585").await, 1);
}

#[tokio::test]
async fn nested_pub_fields_share_their_requests() {
    let h = harness().await;
    h.run(
        "{ pub(ref: 254) { hoursToday { opens } openNow canOrder facilities salesAreas { name } \
           tables { number } t2: tables(availableOnly: true) { number } waitTimes { drinkMinutes } } }",
        json!({}),
    )
    .await;
    assert_eq!(h.hits("GET", "/venues/254").await, 1);
    assert_eq!(h.hits("GET", "/tables").await, 1);
}

#[tokio::test]
async fn a_garden_table_gets_the_gardens_prices() {
    let h = harness().await;
    let q = "query($t: Int) { pub(ref: 254) { menu(table: $t) { salesArea product(id: \"10000000315\") \
             { defaultPortion { name price { amount } } } } } }";
    let bar = h.run(q, json!({ "t": 4 })).await;
    let garden = h.run(q, json!({ "t": 50 })).await;
    assert_eq!(
        bar["pub"]["menu"]["product"]["defaultPortion"]["price"]["amount"],
        3.67
    );
    assert_eq!(garden["pub"]["menu"]["salesArea"], 444);
    assert_eq!(
        garden["pub"]["menu"]["product"]["defaultPortion"]["price"]["amount"],
        3.97
    );
}

#[tokio::test]
async fn four_pints_of_abbot_to_table_four() {
    let h = harness().await;
    let data = h
        .run(
            "mutation($p: ID!) { basketAdd(pub: 254, table: 4, productId: $p, portion: \"pint\", quantity: 4) \
               { success error basket { table count menuTotal { formatted } ready summary \
                 lines { id portion quantity } } } }",
            json!({ "p": ABBOT }),
        )
        .await;
    let basket = &data["basketAdd"]["basket"];
    assert_eq!(data["basketAdd"]["success"], true, "{data}");
    assert_eq!(basket["count"], 4);
    assert_eq!(basket["menuTotal"]["formatted"], "£14.68");
    assert_eq!(basket["ready"], true);
    assert_eq!(basket["lines"][0]["id"], format!("{ABBOT}:pint"));
    assert_eq!(
        basket["summary"],
        "4 × Greene King Abbot Ale (Pint) to table 4 at The Watch House: £14.68 on the menu."
    );

    let quoted = h
        .run(
            "mutation { quote { success error quote { id table total { formatted } menuTotal { formatted } \
               differsFromMenu summary lines { name quantity tillPrice { amount } menuPrice { amount } } } } }",
            json!({}),
        )
        .await;
    let quote = &quoted["quote"]["quote"];
    assert_eq!(quoted["quote"]["success"], true, "{quoted}");
    assert_eq!(quote["table"], 4);
    assert_eq!(quote["total"]["formatted"], "£14.80");
    assert_eq!(quote["menuTotal"]["formatted"], "£14.68");
    assert_eq!(quote["differsFromMenu"], true);
    assert!(
        quote["summary"]
            .as_str()
            .unwrap()
            .contains("the till's price is the one charged")
    );
    assert_eq!(quote["lines"][0]["menuPrice"]["amount"], 3.67);
    // Quoting changed nothing at the pub.
    assert_eq!(h.hits("POST", "/orders/prepare/card").await, 0);
}

#[tokio::test]
async fn never_quotes_without_a_table() {
    let h = harness().await;
    let data = h
        .run(
            "mutation($p: ID!) { basketAdd(pub: 254, productId: $p) { success basket { table ready } } \
               quote { success error } }",
            json!({ "p": CARLING }),
        )
        .await;
    assert_eq!(data["basketAdd"]["success"], true);
    assert_eq!(data["basketAdd"]["basket"]["table"], Value::Null);
    assert_eq!(data["basketAdd"]["basket"]["ready"], false);
    assert_eq!(data["quote"]["success"], false);
    assert!(
        data["quote"]["error"]
            .as_str()
            .unwrap()
            .contains("ask the user which table")
    );
    assert_eq!(h.hits("POST", "/basket/validate").await, 0);
}

#[tokio::test]
async fn refuses_a_table_that_isnt_taking_orders_and_reprices_for_the_garden() {
    let h = harness().await;
    let data = h
        .run(
            "mutation($p: ID!) { add: basketAdd(pub: 254, table: 4, productId: $p) { success } \
               closed: setTable(table: 13) { success error basket { table } } \
               garden: setTable(table: 50) { success basket { table menuTotal { amount } } } }",
            json!({ "p": ABBOT }),
        )
        .await;
    assert_eq!(data["closed"]["success"], false);
    assert!(
        data["closed"]["error"]
            .as_str()
            .unwrap()
            .contains("isn't taking orders")
    );
    assert_eq!(data["closed"]["basket"]["table"], 4);
    assert_eq!(data["garden"]["basket"]["table"], 50);
    assert_eq!(data["garden"]["basket"]["menuTotal"]["amount"], 3.97);
}

#[tokio::test]
async fn edits_the_basket() {
    let h = harness().await;
    let line = format!("{ABBOT}:pint");
    let data = h
        .run(
            "mutation($p: ID!, $c: ID!, $l: ID!) { \
               a: basketAdd(pub: 254, table: 4, productId: $p, quantity: 2) { success } \
               b: basketAdd(productId: $c, portion: \"Half pint\") { success } \
               c: basketSet(lineId: $l, quantity: 5) { basket { count } } \
               d: basketRemove(lineId: $l, quantity: 2) { basket { count } } \
               e: basketSet(productId: $c, portion: \"half pint\", quantity: 0) { basket { lines { name } } } \
               f: basketAdd(pub: 1006, productId: $c) { success error } \
               g: basketClear { basket { count table } } }",
            json!({ "p": ABBOT, "c": CARLING, "l": line }),
        )
        .await;
    assert_eq!(data["c"]["basket"]["count"], 6);
    assert_eq!(data["d"]["basket"]["count"], 4);
    assert_eq!(
        data["e"]["basket"]["lines"],
        json!([{ "name": "Greene King Abbot Ale" }])
    );
    assert_eq!(data["f"]["success"], false, "a basket is for one pub");
    assert_eq!(data["g"]["basket"], json!({ "count": 0, "table": 4 }));
}

#[tokio::test]
async fn each_session_has_its_own_basket() {
    let h = harness().await;
    let other = h.spoons.for_new_session();
    h.run(
        "mutation($p: ID!) { basketAdd(pub: 254, productId: $p) { success } }",
        json!({ "p": ABBOT }),
    )
    .await;
    let theirs = h.run_as(&other, "{ basket { count } }", json!({})).await;
    assert_eq!(theirs["basket"]["count"], 0);
}

#[tokio::test]
async fn a_view_waiting_on_the_basket_hears_a_change() {
    let h = harness().await;
    let spoons = h.spoons.clone();
    let waiting = tokio::spawn(async move {
        spoons
            .execute(
                caller(),
                "{ basket(changedFrom: 0, waitSecs: 5) { version count } }",
                None,
            )
            .await
            .unwrap()
            .data
            .into_json()
            .unwrap()
    });
    tokio::time::sleep(Duration::from_millis(50)).await;
    h.run(
        "mutation($p: ID!) { basketAdd(pub: 254, productId: $p) { success } }",
        json!({ "p": ABBOT }),
    )
    .await;
    let seen = tokio::time::timeout(Duration::from_secs(3), waiting)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(seen["basket"], json!({ "version": 1, "count": 1 }));
}

#[tokio::test]
async fn pay_refuses_confirm_without_a_preview() {
    let h = harness().await;
    h.mount_prepare(false).await;
    let quote = h.four_abbots_to_table_four().await;
    let data = h
        .run(
            "mutation($q: ID!) { pay(quoteId: $q, action: CONFIRM) { success error } \
               forged: pay(quoteId: $q, action: CONFIRM, confirmationToken: \"made-up\") { success error } }",
            json!({ "q": quote }),
        )
        .await;
    assert_eq!(data["pay"]["success"], false);
    assert!(data["pay"]["error"].as_str().unwrap().contains("PREVIEW"));
    assert_eq!(data["forged"]["success"], false);
    assert_eq!(h.hits("POST", "/orders/prepare/card").await, 0);
}

#[tokio::test]
async fn pay_previews_then_confirms_once() {
    let h = harness().await;
    h.mount_prepare(false).await;
    let quote = h.four_abbots_to_table_four().await;
    let preview = h
        .run(
            "mutation($q: ID!) { pay(quoteId: $q, action: PREVIEW) { success preview confirmationToken } }",
            json!({ "q": quote }),
        )
        .await;
    let preview = &preview["pay"];
    assert_eq!(preview["success"], true);
    let text = preview["preview"].as_str().unwrap();
    assert!(text.starts_with("Pay £14.80 on a payment page"), "{text}");
    assert!(text.contains("table 4 at The Watch House"), "{text}");
    assert!(text.contains("drinker@example.com"), "{text}");
    assert_eq!(
        h.hits("POST", "/orders/prepare/card").await,
        0,
        "a preview sends nothing"
    );

    let token = preview["confirmationToken"].as_str().unwrap();
    let confirm = "mutation($q: ID!, $t: String) { pay(quoteId: $q, action: CONFIRM, confirmationToken: $t) \
                   { success error status paymentPage next quote { status } } }";
    let paid = h.run(confirm, json!({ "q": quote, "t": token })).await;
    assert_eq!(paid["pay"]["success"], true, "{paid}");
    assert_eq!(paid["pay"]["status"], "PENDING");
    assert!(
        paid["pay"]["paymentPage"]
            .as_str()
            .unwrap()
            .starts_with("https://paymentpage.axepta")
    );
    assert_eq!(h.hits("POST", "/orders/prepare/card").await, 1);
    // The token is spent.
    let again = h.run(confirm, json!({ "q": quote, "t": token })).await;
    assert_eq!(again["pay"]["success"], false);
    assert_eq!(h.hits("POST", "/orders/prepare/card").await, 1);
    // What was ordered left the basket.
    let basket = h.run("{ basket { count } }", json!({})).await;
    assert_eq!(basket["basket"]["count"], 0);
    let order = h
        .run(
            "query($q: ID!) { order(quoteId: $q) { status meaning paymentPage } }",
            json!({ "q": quote }),
        )
        .await;
    assert_eq!(order["order"]["status"], "PENDING");
    assert!(order["order"]["paymentPage"].is_string());
}

#[tokio::test]
async fn a_token_only_confirms_what_was_previewed() {
    let h = harness().await;
    h.mount_prepare(true).await;
    let quote = h.four_abbots_to_table_four().await;
    let preview = h
        .run(
            "mutation($q: ID!) { pay(quoteId: $q, action: PREVIEW) { confirmationToken } }",
            json!({ "q": quote }),
        )
        .await;
    let token = preview["pay"]["confirmationToken"].as_str().unwrap();
    let sneaky = h
        .run(
            "mutation($q: ID!, $t: String) { pay(quoteId: $q, action: CONFIRM, confirmationToken: $t, \
               confirmDuplicate: true) { success error } }",
            json!({ "q": quote, "t": token }),
        )
        .await;
    assert_eq!(sneaky["pay"]["success"], false);
    assert!(sneaky["pay"]["error"].as_str().unwrap().contains("changed"));
    assert_eq!(h.hits("POST", "/orders/prepare/card").await, 0);
}

#[tokio::test]
async fn a_repeat_order_asks_the_user_first() {
    let h = harness().await;
    h.mount_prepare(true).await;
    let quote = h.four_abbots_to_table_four().await;
    let preview = h
        .run(
            "mutation($q: ID!) { pay(quoteId: $q, action: PREVIEW) { confirmationToken } }",
            json!({ "q": quote }),
        )
        .await;
    let paid = h
        .run(
            "mutation($q: ID!, $t: String) { pay(quoteId: $q, action: CONFIRM, confirmationToken: $t) \
               { success question paymentPage } }",
            json!({ "q": quote, "t": preview["pay"]["confirmationToken"] }),
        )
        .await;
    assert!(
        paid["pay"]["question"]
            .as_str()
            .unwrap()
            .contains("Ask the user")
    );
    assert_eq!(
        paid["pay"]["paymentPage"],
        Value::Null,
        "no page until the user says yes"
    );
    // The basket stays, since nothing has been ordered yet.
    let basket = h.run("{ basket { count } }", json!({})).await;
    assert_eq!(basket["basket"]["count"], 4);
}

#[tokio::test]
async fn cancel_and_send_duplicate_need_a_preview() {
    let h = harness().await;
    h.mount_prepare(true).await;
    Mock::given(method("POST"))
        .and(path_regex("/process/resubmit$"))
        .respond_with(ResponseTemplate::new(200))
        .expect(0)
        .mount(&h.server)
        .await;
    let quote = h.four_abbots_to_table_four().await;
    let data = h
        .run(
            "mutation($q: ID!) { cancelOrder(quoteId: $q, action: CONFIRM) { success error } \
               sendDuplicate(quoteId: $q, action: CONFIRM) { success error } \
               p: cancelOrder(quoteId: $q, action: PREVIEW) { success preview confirmationToken order { status } } }",
            json!({ "q": quote }),
        )
        .await;
    assert_eq!(data["cancelOrder"]["success"], false);
    assert_eq!(data["sendDuplicate"]["success"], false);
    assert!(
        data["p"]["preview"]
            .as_str()
            .unwrap()
            .starts_with("Cancel the order of 4 × Greene King Abbot Ale")
    );
    assert_eq!(data["p"]["order"]["status"], "NOT_STARTED");
    let cancelled = h
        .run(
            "mutation($q: ID!, $t: String) { cancelOrder(quoteId: $q, action: CONFIRM, confirmationToken: $t) \
               { success order { status } } }",
            json!({ "q": quote, "t": data["p"]["confirmationToken"] }),
        )
        .await;
    assert_eq!(cancelled["cancelOrder"]["order"]["status"], "CANCELLED");
    assert_eq!(
        h.hits("POST", "/cancel").await,
        0,
        "nothing to cancel at the payment API before paying"
    );
}

#[tokio::test]
async fn same_again_refills_the_basket_from_the_last_quote() {
    let h = harness().await;
    let quote = h.four_abbots_to_table_four().await;
    let data = h
        .run("mutation { basketClear { basket { count } } }", json!({}))
        .await;
    assert_eq!(data["basketClear"]["basket"]["count"], 0);
    let quotes = h.run("{ quotes { id table } }", json!({})).await;
    assert_eq!(quotes["quotes"], json!([{ "id": quote, "table": 4 }]));
    let again = h
        .run(
            "mutation($q: ID!) { basketFromQuote(quoteId: $q) { success basket { table count ready } } }",
            json!({ "q": quote }),
        )
        .await;
    assert_eq!(
        again["basketFromQuote"]["basket"],
        json!({ "table": 4, "count": 4, "ready": true })
    );
}

#[tokio::test]
async fn now_costs_nothing() {
    let h = harness().await;
    let data = h.run("{ now { iso time date weekday } }", json!({})).await;
    assert!(data["now"]["iso"].as_str().unwrap().contains('T'));
    assert!(h.server.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn one_pub_failing_doesnt_fail_the_list() {
    let h = harness().await;
    // Only The Watch House has details in the fixtures; the others 404.
    let response = h
        .spoons
        .execute(
            caller(),
            "{ pubs(first: 6) { nodes { ref openNow waitTimes { drinkMinutes } } } }",
            None,
        )
        .await
        .unwrap();
    assert!(response.errors.is_empty(), "{:?}", response.errors);
    let data = response.data.into_json().unwrap();
    let nodes = data["pubs"]["nodes"].as_array().unwrap();
    let watch_house = nodes.iter().find(|n| n["ref"] == 254).unwrap();
    assert!(watch_house["openNow"].is_boolean());
    assert!(watch_house["waitTimes"]["drinkMinutes"].is_number());
    assert!(
        nodes
            .iter()
            .filter(|n| n["ref"] != 254)
            .all(|n| n["openNow"].is_null())
    );
}

/// A tool result's text, parsed.
fn answer(result: &rmcp::model::CallToolResult) -> Value {
    let block = serde_json::to_value(&result.content[0]).unwrap();
    serde_json::from_str(block["text"].as_str().unwrap()).unwrap()
}

#[tokio::test]
async fn a_tool_result_carries_the_screen_for_the_app() {
    let h = harness().await;
    let request: crate::mcp::GraphqlRequest = serde_json::from_value(json!({
        "query": "query($t: Int) { pub(ref: 254) { menu(table: $t, search: \"ale\") { totalCount } } }",
        "variables": "{\"t\": 4}",
        "ui": { "view": "FULL", "focus": "CASK_ALE" },
    }))
    .unwrap();
    let result = h.spoons.run_tool(caller(), &request).await;
    let structured = result.structured_content.clone().unwrap();
    assert_eq!(
        structured["view"],
        json!({ "screen": "menu", "pub": 254, "table": 4, "search": "ale" })
    );
    assert_eq!(structured["ui"]["focus"], "CASK_ALE");
    // The response itself is the text, for the model and hosts without the app; it isn't sent twice.
    assert!(structured.get("data").is_none());
    assert!(
        answer(&result)["data"]["pub"]["menu"]["totalCount"]
            .as_u64()
            .unwrap()
            > 0
    );
}

#[tokio::test]
async fn the_app_reaches_its_conversations_basket_and_no_one_elses() {
    let h = harness().await;
    let call = |query: &str, session: Option<&str>| -> crate::mcp::GraphqlRequest {
        serde_json::from_value(
            json!({ "query": query, "ui": { "view": "NONE", "session": session } }),
        )
        .unwrap()
    };
    let add = format!("mutation {{ basketAdd(pub: 254, productId: \"{ABBOT}\") {{ success }} }}");
    let model = h.spoons.run_tool(caller(), &call(&add, None)).await;
    let session = model.structured_content.unwrap()["session"]
        .as_str()
        .unwrap()
        .to_owned();
    // The host gave the app a session of its own; naming the conversation's reaches its basket.
    let app = h.spoons.for_new_session();
    let seen = app
        .run_tool(caller(), &call("{ basket { count } }", Some(&session)))
        .await;
    assert_eq!(
        seen.structured_content.as_ref().unwrap()["view"],
        Value::Null,
        "the app's own calls draw nothing"
    );
    assert_eq!(answer(&seen)["data"]["basket"]["count"], 1);
    // Someone else can't, even knowing the id.
    let stranger = Caller {
        email: Some("someone@else.com".into()),
        card: Ok(None),
    };
    let theirs = app
        .run_tool(stranger, &call("{ basket { count } }", Some(&session)))
        .await;
    assert_eq!(answer(&theirs)["data"]["basket"]["count"], 0);
}

#[tokio::test]
async fn quotes_items_by_a_sizes_name_as_well_as_its_label() {
    let h = harness().await;
    // Wine labels carry calories and units; people (and models) say "250ml glass".
    let merlot = h
        .run("{ pub(ref: 254) { menu(search: \"merlot\") { products { nodes { id portions { label name } } } } } }", json!({}))
        .await;
    let product = &merlot["pub"]["menu"]["products"]["nodes"][0];
    let big = product["portions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["name"] == "250ml glass")
        .unwrap();
    assert_ne!(
        big["label"], big["name"],
        "the fixture's label carries more than the name"
    );
    let quoted = h
        .run(
            "mutation($p: ID!) { quote(pub: 254, table: 4, items: [{productId: $p, portion: \"250ml glass\"}]) \
               { success error quote { lines { portion } } } }",
            json!({ "p": product["id"] }),
        )
        .await;
    assert_eq!(quoted["quote"]["success"], true, "{quoted}");
    assert_eq!(
        quoted["quote"]["quote"]["lines"][0]["portion"],
        big["label"]
    );
}

#[tokio::test]
async fn an_api_thats_down_is_an_error_not_a_list_of_empty_pubs() {
    let h = harness().await;
    // None of these pubs have details, ales or tables in the fixtures.
    let response = h
        .spoons
        .execute(
            caller(),
            "{ pubs(query: \"greenwich\") { nodes { name tables { number } } } }",
            None,
        )
        .await
        .unwrap();
    assert!(!response.errors.is_empty());
}

// Regressions from a real session at The Brockley Barge, ordering by voice: what was said, and what it should
// have found.

async fn names(h: &Harness, search: &str) -> (Vec<String>, bool) {
    let data = h
        .run(
            "query($s: String) { pub(ref: 254) { menu { products(search: $s, first: 10) { closeMatches nodes { name } } } } }",
            json!({ "s": search }),
        )
        .await;
    let products = &data["pub"]["menu"]["products"];
    (
        products["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|n| n["name"].as_str().unwrap().to_owned())
            .collect(),
        products["closeMatches"].as_bool().unwrap(),
    )
}

#[tokio::test]
async fn finds_a_misheard_pub() {
    let h = harness().await;
    for said in [
        "Broccoli Barge",
        "the brockley barge spoons",
        "barge",
        "brockly",
    ] {
        let data = h
            .run(
                "query($q: String) { pubs(query: $q) { closeMatches nodes { ref name } } }",
                json!({ "q": said }),
            )
            .await;
        assert_eq!(data["pubs"]["nodes"][0]["ref"], 1006, "{said}: {data}");
    }
    // A town or a postcode still works, squashed or not.
    let data = h
        .run("{ pubs(query: \"se42rr\") { nodes { ref } } }", json!({}))
        .await;
    assert_eq!(data["pubs"]["nodes"][0]["ref"], 1006);
}

#[tokio::test]
async fn sees_through_brand_styling() {
    let h = harness().await;
    let (found, close) = names(&h, "sumo").await;
    assert_eq!(
        found.first().map(String::as_str),
        Some("Mad Squirrel $umo"),
        "{found:?}"
    );
    assert!(!close);
}

#[tokio::test]
async fn knows_what_a_mocktail_is() {
    let h = harness().await;
    for said in [
        "mocktail",
        "a virgin cocktail",
        "mocktails please",
        "something alcohol free and fruity",
    ] {
        let (found, _) = names(&h, said).await;
        let top: Vec<&str> = found.iter().take(2).map(String::as_str).collect();
        assert!(
            top.iter().all(|n| [
                "Raspberry Refresher",
                "Passion Fruit Cooler",
                "Rasp Refresher"
            ]
            .contains(n)),
            "{said}: {found:?}"
        );
    }
}

#[tokio::test]
async fn a_cocktail_is_a_drink_before_it_is_crisps() {
    let h = harness().await;
    let (found, _) = names(&h, "cocktail").await;
    let crisps = found.iter().position(|n| n.contains("crisps"));
    assert!(crisps.is_none_or(|i| i >= 5), "{found:?}");
    assert!(!found[0].contains("crisps"));
}

#[tokio::test]
async fn forgives_typos_and_filler() {
    let h = harness().await;
    for said in ["abot", "4 pints of abbot please", "Abbot Ale"] {
        let (found, _) = names(&h, said).await;
        assert_eq!(
            found.first().map(String::as_str),
            Some("Greene King Abbot Ale"),
            "{said}: {found:?}"
        );
    }
}

#[tokio::test]
async fn offers_the_nearest_rather_than_nothing() {
    let h = harness().await;
    let (found, close) = names(&h, "sunrise cocktail").await;
    assert!(close, "nothing here is a sunrise");
    assert!(!found.is_empty());
}

#[tokio::test]
async fn lists_and_filters_by_the_pubs_own_categories() {
    let h = harness().await;
    let data = h
        .run(
            "{ pub(ref: 254) { menu { categories { name kind count } \
               zero: products(category: \"0% cocktails\") { nodes { name } } \
               mock: products(category: \"mocktails\") { totalCount } \
               soft: products(category: \"soft drinks\") { totalCount } } } }",
            json!({}),
        )
        .await;
    let menu = &data["pub"]["menu"];
    let categories = menu["categories"].as_array().unwrap();
    let zero = categories
        .iter()
        .find(|c| c["name"] == "0% cocktails")
        .unwrap();
    assert_eq!(zero["kind"], "ALCOHOL_FREE");
    let names: Vec<&str> = menu["zero"]["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|n| n["name"].as_str().unwrap())
        .collect();
    assert_eq!(names.len(), zero["count"].as_u64().unwrap() as usize);
    assert!(
        names.contains(&"Raspberry Refresher") && names.contains(&"Passion Fruit Cooler"),
        "{names:?}"
    );
    assert_eq!(menu["mock"]["totalCount"], zero["count"]);
    assert!(menu["soft"]["totalCount"].as_u64().unwrap() > 0);
}

#[tokio::test]
async fn pages_through_the_whole_menu() {
    let h = harness().await;
    let mut after: Option<String> = None;
    let mut seen = std::collections::HashSet::new();
    let total = loop {
        let data = h
            .run(
                "query($a: String) { pub(ref: 254) { menu { products(first: 40, after: $a) { totalCount \
                   nodes { id } pageInfo { hasNextPage endCursor } } } } }",
                json!({ "a": after }),
            )
            .await;
        let page = &data["pub"]["menu"]["products"];
        for node in page["nodes"].as_array().unwrap() {
            assert!(
                seen.insert(node["id"].as_str().unwrap().to_owned()),
                "a product came twice"
            );
        }
        if page["pageInfo"]["hasNextPage"] != true {
            break page["totalCount"].as_u64().unwrap();
        }
        after = page["pageInfo"]["endCursor"].as_str().map(str::to_owned);
    };
    assert_eq!(seen.len() as u64, total);
    assert!(total > 50, "every product, not the old first fifty");
}

#[tokio::test]
async fn an_alcohol_free_lager_is_both() {
    let h = harness().await;
    let (found, close) = names(&h, "alcohol free lager").await;
    assert!(!close, "{found:?}");
    assert!(
        found
            .iter()
            .take(3)
            .all(|n| n.to_lowercase().contains("alcohol free") || n.contains("Low alcohol")),
        "{found:?}"
    );
    let (found, _) = names(&h, "a pint of stella").await;
    assert!(found.iter().all(|n| n.contains("Stella")), "{found:?}");
}

// Products that ask a question before they can be ordered: a spirit's mixer, a meal's side, a meal's drink.

const FISH: &str = "10000162183";
const WRAP: &str = "10000159693";

async fn product_id(h: &Harness, search: &str) -> String {
    let data = h
        .run(
            "query($s: String) { pub(ref: 254) { menu { products(search: $s, first: 1) { nodes { id } } } } }",
            json!({ "s": search }),
        )
        .await;
    data["pub"]["menu"]["products"]["nodes"][0]["id"]
        .as_str()
        .unwrap()
        .to_owned()
}

#[tokio::test]
async fn shows_the_questions_a_product_asks() {
    let h = harness().await;
    let data = h
        .run(
            "query($p: ID!) { pub(ref: 254) { menu { product(id: $p) { orderable needsChoices \
               optionGroups { id name required minPicks maxPicks options { id label price { amount } available } } \
               deals { name kind buy } } } } }",
            json!({ "p": FISH }),
        )
        .await;
    let product = &data["pub"]["menu"]["product"];
    assert_eq!(product["orderable"], true);
    assert_eq!(product["needsChoices"], true);
    let group = &product["optionGroups"][0];
    assert_eq!(group["name"], "Peas, mushy peas or beans?");
    assert_eq!(
        (
            group["required"].clone(),
            group["minPicks"].clone(),
            group["maxPicks"].clone()
        ),
        (json!(true), json!(1), json!(1))
    );
    let labels: Vec<&str> = group["options"]
        .as_array()
        .unwrap()
        .iter()
        .map(|o| o["label"].as_str().unwrap())
        .collect();
    assert_eq!(
        labels,
        ["Plain", "Baked Beans", "Garden Peas", "Mushy Peas"]
    );
    assert_eq!(
        product["deals"],
        json!([{ "name": "Includes a drink", "kind": "INCLUDES_DRINK", "buy": null }])
    );
}

#[tokio::test]
async fn asks_rather_than_guesses_a_missing_choice() {
    let h = harness().await;
    let data = h
        .run(
            "mutation($p: ID!) { basketAdd(pub: 254, table: 4, productId: $p) { success error basket { count } } }",
            json!({ "p": FISH }),
        )
        .await;
    assert_eq!(data["basketAdd"]["success"], false);
    let error = data["basketAdd"]["error"].as_str().unwrap();
    assert!(
        error.contains("needs a choice for “Peas, mushy peas or beans?”: Plain, Baked Beans, Garden Peas, Mushy Peas"),
        "{error}"
    );
    let wrong = h
        .run(
            "mutation($p: ID!) { quote(pub: 254, table: 4, items: [{productId: $p, choices: [\"coleslaw\"]}]) { success error } }",
            json!({ "p": FISH }),
        )
        .await;
    assert!(
        wrong["quote"]["error"]
            .as_str()
            .unwrap()
            .contains("“coleslaw” isn't one of"),
        "{wrong}"
    );
    assert_eq!(
        h.hits("POST", "/basket/validate").await,
        0,
        "nothing reached the till unasked"
    );
}

#[tokio::test]
async fn orders_fish_and_chips_with_mushy_peas() {
    let h = harness().await;
    let data = h
        .run(
            "mutation($p: ID!) { basketAdd(pub: 254, table: 4, productId: $p, choices: [\"mushy peas\"]) \
               { success error basket { lines { id options } summary } } \
               quote { success error quote { summary lines { description options } } } }",
            json!({ "p": FISH }),
        )
        .await;
    assert_eq!(data["basketAdd"]["success"], true, "{data}");
    assert_eq!(
        data["basketAdd"]["basket"]["lines"][0]["options"],
        json!(["Mushy Peas"])
    );
    assert_eq!(data["quote"]["success"], true, "{data}");
    assert_eq!(
        data["quote"]["quote"]["lines"][0]["description"],
        "Small freshly battered fish and chips (Mushy Peas)"
    );
    // The till got the choice as the app sends it.
    let sent = h.server.received_requests().await.unwrap();
    let body: Value = serde_json::from_slice(
        &sent
            .iter()
            .rev()
            .find(|r| r.url.path().ends_with("/validate"))
            .unwrap()
            .body,
    )
    .unwrap();
    let choice = &body["items"][0]["choices"]["10000152029"];
    assert_eq!(choice["name"], "Mushy Peas");
    assert_eq!(choice["checkout"]["choiceId"], 10000152029_u64);
}

#[tokio::test]
async fn a_gin_and_lemonade_or_neat() {
    let h = harness().await;
    let gin = product_id(&h, "gordons gin").await;
    // "tonic" is the plain tonic, not the elderflower; "potato"-style ties between two plain options still ask
    // (see choices::tests).
    for (said, expect) in [
        ("tonic", "Indian tonic"),
        ("elderflower tonic", "elderflower"),
        ("neat", "No mixer"),
    ] {
        let data = h
            .run(
                "mutation($p: ID!, $c: [String!]) { quote(pub: 254, table: 4, items: [{productId: $p, portion: \"double\", choices: $c}]) \
                   { success error quote { lines { description } } } }",
                json!({ "p": gin, "c": [said] }),
            )
            .await;
        assert_eq!(data["quote"]["success"], true, "{said}: {data}");
        let description = data["quote"]["quote"]["lines"][0]["description"]
            .as_str()
            .unwrap();
        assert!(
            description.contains("Double") && description.contains(expect),
            "{said}: {description}"
        );
    }
}

#[tokio::test]
async fn a_meal_comes_with_its_drink_and_the_deal() {
    let h = harness().await;
    let drinks = h
        .run(
            "query($p: ID!) { pub(ref: 254) { menu { product(id: $p) { includedDrinks(search: \"guinness\") { id name } } } } }",
            json!({ "p": WRAP }),
        )
        .await;
    let guinness = drinks["pub"]["menu"]["product"]["includedDrinks"][0]["id"].clone();
    assert_eq!(
        drinks["pub"]["menu"]["product"]["includedDrinks"][0]["name"],
        "Guinness"
    );
    let data = h
        .run(
            "mutation($p: ID!, $d: ID!) { basketAdd(pub: 254, table: 4, productId: $p, quantity: 2, \
                 choices: [\"chips\", \"grilled\"], includeDrink: {productId: $d, portion: \"pint\"}) \
               { success error basket { count lines { id name includedWith quantity } } } }",
            json!({ "p": WRAP, "d": guinness }),
        )
        .await;
    assert_eq!(data["basketAdd"]["success"], true, "{data}");
    let lines = data["basketAdd"]["basket"]["lines"].as_array().unwrap();
    assert_eq!(lines.len(), 2);
    assert_eq!(lines[1]["name"], "Guinness");
    assert_eq!(lines[1]["includedWith"], lines[0]["id"]);
    assert_eq!(lines[1]["quantity"], 2);

    let quoted = h
        .run(
            "mutation { quote { success error quote { summary savings { formatted } total { amount } lines { description deal } } } }",
            json!({}),
        )
        .await;
    let quote = &quoted["quote"]["quote"];
    assert_eq!(quoted["quote"]["success"], true, "{quoted}");
    assert_eq!(quote["savings"]["formatted"], "£3.92");
    assert_eq!(
        quote["lines"][1]["deal"],
        "included with Chicken & stuffing wrap"
    );
    assert!(
        quote["summary"]
            .as_str()
            .unwrap()
            .contains("Deals took off £3.92"),
        "{quote}"
    );

    // Fewer meals, fewer drinks; no meal, no drink.
    let meal = lines[0]["id"].as_str().unwrap();
    let fewer = h
        .run(
            "mutation($l: ID!) { a: basketSet(lineId: $l, quantity: 1) { basket { lines { quantity } } } \
               b: basketRemove(lineId: $l) { basket { count } } }",
            json!({ "l": meal }),
        )
        .await;
    assert_eq!(
        fewer["a"]["basket"]["lines"],
        json!([{ "quantity": 1 }, { "quantity": 1 }])
    );
    assert_eq!(fewer["b"]["basket"]["count"], 0);
}

#[tokio::test]
async fn same_again_keeps_the_options_and_the_drink() {
    let h = harness().await;
    let data = h
        .run(
            "mutation($p: ID!, $w: ID!) { quote(pub: 254, table: 4, items: [ \
                 {productId: $p, choices: [\"garden peas\"]}, \
                 {productId: $w, choices: [\"rice\", \"grilled\"], includeDrink: {productId: \"10000000311\"}}]) \
               { success error quote { id } } }",
            json!({ "p": FISH, "w": WRAP }),
        )
        .await;
    assert_eq!(data["quote"]["success"], true, "{data}");
    let again = h
        .run(
            "mutation($q: ID!) { basketFromQuote(quoteId: $q) { basket { ready lines { name options includedWith note } } } \
               quote { success error quote { lines { description } } } }",
            json!({ "q": data["quote"]["quote"]["id"] }),
        )
        .await;
    let lines = again["basketFromQuote"]["basket"]["lines"]
        .as_array()
        .unwrap();
    assert_eq!(lines.len(), 3, "{again}");
    assert_eq!(lines[0]["options"], json!(["Garden Peas"]));
    assert!(lines[2]["includedWith"].is_string());
    assert_eq!(again["quote"]["success"], true, "{again}");
}

/// What the app's sheet sends: options by group id and option id, and the drink with no size.
#[tokio::test]
async fn the_sheet_adds_by_ids_with_a_drink() {
    let h = harness().await;
    let menu = h
        .run(
            "query($p: ID!) { pub(ref: 254) { menu(table: 4) { product(id: $p) { \
               optionGroups { id name options { id label } } includedDrinks(search: \"guinness\") { id } } } } }",
            json!({ "p": FISH }),
        )
        .await;
    let product = &menu["pub"]["menu"]["product"];
    let group = &product["optionGroups"][0];
    let mushy = group["options"]
        .as_array()
        .unwrap()
        .iter()
        .find(|o| o["label"] == "Mushy Peas")
        .unwrap_or_else(|| panic!("{group}"))["id"]
        .clone();
    let data = h
        .run(
            "mutation($p: ID!, $o: [OptionPickInput!], $d: IncludedDrinkInput) { \
               basketAdd(pub: 254, productId: $p, portion: \"Standard\", options: $o, includeDrink: $d) \
               { success error basket { lines { name options includedWith } } } }",
            json!({ "p": FISH,
                    "o": [{ "group": group["id"], "options": [mushy] }],
                    "d": { "productId": product["includedDrinks"][0]["id"], "portion": null } }),
        )
        .await;
    assert_eq!(data["basketAdd"]["success"], true, "{data}");
    let lines = &data["basketAdd"]["basket"]["lines"];
    assert_eq!(lines[0]["options"], json!(["Mushy Peas"]), "{data}");
    assert_eq!(lines[1]["name"], "Guinness", "{data}");
}

/// The till names a guest ale by a cut-down id (`1000000111112124` comes back as `10000001111`); its line still
/// carries the till's price.
#[tokio::test]
async fn every_quote_line_carries_its_price() {
    let h = harness().await;
    let data = h
        .run(
            "mutation { quote(pub: 254, table: 4, items: [{productId: \"1000000111112124\", portion: \"Pint\"}, \
               {productId: \"1000000111112124\", portion: \"Half pint\"}]) \
               { success error quote { table total { amount } lines { portion tillPrice { amount } } } } }",
            json!({}),
        )
        .await;
    let quote = &data["quote"]["quote"];
    assert_eq!(data["quote"]["success"], true, "{data}");
    assert_eq!(quote["table"], 4);
    for line in quote["lines"].as_array().unwrap() {
        assert_eq!(line["tillPrice"]["amount"], 3.7, "{data}");
    }
}

/// Once a table is given, later orders in the conversation go to it without being asked again; another pub, or no
/// table ever given, still needs the user's word.
#[tokio::test]
async fn the_table_sticks_for_the_conversation() {
    let h = harness().await;
    let none = h
        .run(
            "mutation { quote(pub: 254, items: [{productId: \"1000000111112124\", portion: \"Pint\"}]) { success error } }",
            json!({}),
        )
        .await;
    assert_eq!(none["quote"]["success"], false);
    assert!(
        none["quote"]["error"]
            .as_str()
            .unwrap()
            .contains("no table"),
        "{none}"
    );

    let first = h
        .run(
            "mutation { quote(pub: 254, table: 4, items: [{productId: \"1000000111112124\", portion: \"Pint\"}]) { success } }",
            json!({}),
        )
        .await;
    assert_eq!(first["quote"]["success"], true, "{first}");
    let again = h
        .run(
            "mutation { quote(items: [{productId: \"1000000111112124\", portion: \"Half pint\"}]) \
               { success error quote { table summary } } }",
            json!({}),
        )
        .await;
    assert_eq!(again["quote"]["success"], true, "{again}");
    assert_eq!(again["quote"]["quote"]["table"], 4);
    assert!(
        again["quote"]["quote"]["summary"]
            .as_str()
            .unwrap()
            .contains("to table 4"),
        "{again}"
    );

    let elsewhere = h
        .run(
            "mutation { quote(pub: 1006, items: [{productId: \"1000000111112124\", portion: \"Pint\"}]) { success error } }",
            json!({}),
        )
        .await;
    assert_eq!(elsewhere["quote"]["success"], false, "{elsewhere}");
    assert!(
        elsewhere["quote"]["error"]
            .as_str()
            .unwrap()
            .contains("no table"),
        "{elsewhere}"
    );
}

/// "A markdown menu of the pub I'm in": with no ref or table, the conversation's pub and table.
#[tokio::test]
async fn the_menu_as_markdown_for_the_pub_im_in() {
    let h = harness().await;
    let nowhere = h
        .spoons
        .execute(caller(), "{ pub { name } }", None)
        .await
        .unwrap();
    assert!(
        nowhere.errors[0]
            .message
            .contains("no pub in this conversation"),
        "{:?}",
        nowhere.errors
    );
    h.run(
        "mutation { setTable(pub: 254, table: 4) { success } }",
        json!({}),
    )
    .await;
    let data = h
        .run("{ pub { name menu { table markdown } } }", json!({}))
        .await;
    assert_eq!(data["pub"]["name"], "The Watch House");
    assert_eq!(data["pub"]["menu"]["table"], 4);
    let md = data["pub"]["menu"]["markdown"].as_str().unwrap();
    assert!(md.starts_with("# The Watch House · table 4\n"), "{md}");
    assert!(md.contains("\n## Cask ales\n"), "{md}");
    assert!(md.contains("- **Greene King Abbot Ale** 5%"), "{md}");
    assert!(md.contains("_(guest)_"), "{md}");
    assert!(md.contains("_(asks: "), "{md}");

    let food = h
        .run(
            "{ pub { menu(kinds: [FOOD]) { markdown(byCategory: true) } } }",
            json!({}),
        )
        .await;
    let food = food["pub"]["menu"]["markdown"].as_str().unwrap();
    assert!(food.contains("\n## Pub classics\n"), "{food}");
    assert!(!food.contains("## Cask ales"), "{food}");
}
