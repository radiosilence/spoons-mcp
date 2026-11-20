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
        let items: Vec<Value> = body["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|line| {
                let quantity = line["quantity"].as_u64().unwrap();
                json!({ "id": line["product"]["id"], "quantity": quantity,
                        "price": { "value": 3.70 * quantity as f64 } })
            })
            .collect();
        let total: f64 = items
            .iter()
            .map(|i| i["price"]["value"].as_f64().unwrap())
            .sum();
        ResponseTemplate::new(200).set_body_json(json!({ "data": {
            "id": "basket-1", "total": { "value": (total * 100.0).round() / 100.0, "currency": "GBP" },
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
    assert_eq!(pink["orderable"], false);
    assert!(pink["whyNot"].as_str().unwrap().contains("choice"));
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
