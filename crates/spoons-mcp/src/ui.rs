//! The MCP App (the `io.modelcontextprotocol/ui` extension, spec 2026-01-26): one HTML page, linked to the `spoons`
//! tool, that hosts render beside its results.
//!
//! Conversation comes first and the app is an enhancement. The tool's text result is the GraphQL response, whole
//! and readable without the app. For hosts that render apps, the result also carries `structuredContent`: that
//! response plus a `view` saying which screen fits it, worked out here from the operation that ran, so the app
//! needn't guess from whatever fields the model happened to select. The app then reads what it shows through the
//! same `spoons` tool, with the same GraphQL, and changes things the same way: there is no private path.
//!
//! The page is built from `crates/spoons-mcp/app` by `just app` into `assets/app.html`, one self-contained file
//! with no network access: its CSP below allows nothing beyond itself.

use async_graphql::parser::types::{
    DocumentOperations, ExecutableDocument, OperationType, Selection,
};
use async_graphql::{Name, Value as GqlValue, Variables};
use serde::Deserialize;
use serde_json::{Map, Value, json};

pub const URI: &str = "ui://spoons/app.html";
pub const MIME: &str = "text/html;profile=mcp-app";
pub const HTML: &str = include_str!("../assets/app.html");

/// The page: the one built in, or for working on the app, the file `SPOONS_APP_HTML` names, read on every request.
pub fn html() -> std::borrow::Cow<'static, str> {
    match std::env::var_os("SPOONS_APP_HTML").map(std::fs::read_to_string) {
        Some(Ok(html)) => html.into(),
        _ => HTML.into(),
    }
}

/// The tool's `_meta`: the modern `ui.resourceUri`, and the flat key older hosts read.
pub fn tool_meta() -> rmcp::model::MetaObject {
    let mut meta = Map::new();
    meta.insert("ui".into(), json!({ "resourceUri": URI }));
    meta.insert("ui/resourceUri".into(), json!(URI));
    rmcp::model::MetaObject(meta)
}

/// The resource's `_meta.ui`: no network, no frames, and a border left to the app's own cards.
pub fn resource_meta() -> rmcp::model::MetaObject {
    let mut meta = Map::new();
    meta.insert(
        "ui".into(),
        json!({
            "csp": { "connectDomains": [], "resourceDomains": [], "frameDomains": [] },
            "prefersBorder": false,
        }),
    );
    rmcp::model::MetaObject(meta)
}

/// How the model would like a result shown. Every part is optional and only a preference.
#[derive(Debug, Default, Clone, Deserialize, schemars::JsonSchema, serde::Serialize)]
pub struct Hints {
    /// `COMPACT` for a quick one-card view, `FULL` for the whole screen, `NONE` to show nothing for this call.
    /// Left out, the app chooses: compact for small results, full for menus and pub lists.
    pub view: Option<ViewMode>,
    /// What to bring into view: a menu kind such as `CASK_ALE`, or a product id.
    pub focus: Option<String>,
    /// Product ids to pick out, such as the ones the user asked about.
    pub highlight: Option<Vec<String>>,
    /// Set by the app on its own calls, to reach the conversation's basket. Leave it out.
    pub session: Option<String>,
}

#[derive(
    Debug, Clone, Copy, Deserialize, serde::Serialize, schemars::JsonSchema, PartialEq, Eq,
)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ViewMode {
    Compact,
    Full,
    None,
}

/// Which screen fits a result, in order of precedence: a result about money outranks the basket it came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Screen {
    Pubs,
    Menu,
    Basket,
    Quote,
    Order,
}

impl Screen {
    fn name(self) -> &'static str {
        match self {
            Screen::Pubs => "pubs",
            Screen::Menu => "menu",
            Screen::Basket => "basket",
            Screen::Quote => "quote",
            Screen::Order => "order",
        }
    }
}

fn const_value(value: &GqlValue) -> Value {
    value.clone().into_json().unwrap_or(Value::Null)
}

/// The view for an operation and what it returned, or `None` when there's nothing worth a screen (the time, the
/// schema, a failed parse).
pub fn view(query: &str, variables: &Variables, data: &Value, hints: &Hints) -> Option<Value> {
    if hints.view == Some(ViewMode::None) {
        return None;
    }
    let document: ExecutableDocument = async_graphql::parser::parse_query(query).ok()?;
    let operation = match &document.operations {
        DocumentOperations::Single(op) => &op.node,
        DocumentOperations::Multiple(ops) => &ops.values().next()?.node,
    };
    let mutation = operation.ty == OperationType::Mutation;
    let mut best: Option<(Screen, Value)> = None;
    for item in &operation.selection_set.node.items {
        let Selection::Field(field) = &item.node else {
            continue;
        };
        let field = &field.node;
        let name = field.name.node.as_str();
        let key = field.alias.as_ref().map_or(name, |a| a.node.as_str());
        let result = data.get(key).cloned().unwrap_or(Value::Null);
        let args: Map<String, Value> = field
            .arguments
            .iter()
            .filter_map(|(arg, value)| {
                let resolved = value
                    .node
                    .clone()
                    .into_const_with(|var: Name| variables.get(&var).cloned().ok_or(()))
                    .ok()?;
                Some((arg.node.to_string(), const_value(&resolved)))
            })
            .collect();
        let arg = |k: &str| args.get(k).cloned().unwrap_or(Value::Null);
        let found = match (mutation, name) {
            (true, "pay" | "cancelOrder" | "sendDuplicate") => Some((
                Screen::Quote,
                json!({ "quoteId": arg("quoteId"), "action": name, "result": result }),
            )),
            (true, "quote") => {
                let id = result.pointer("/quote/id").cloned().unwrap_or(Value::Null);
                Some((Screen::Quote, json!({ "quoteId": id, "result": result })))
            }
            (true, _) if name.starts_with("basket") || name == "setTable" => {
                Some((Screen::Basket, json!({ "result": result })))
            }
            (false, "order") => Some((Screen::Order, json!({ "quoteId": arg("quoteId") }))),
            (false, "quote") => Some((Screen::Quote, json!({ "quoteId": arg("id") }))),
            (false, "basket") => Some((Screen::Basket, json!({}))),
            (false, "pubs") => Some((
                Screen::Pubs,
                json!({ "query": arg("query"), "near": arg("near") }),
            )),
            (false, "pub") => Some(pub_view(field, &args, variables)),
            _ => None,
        };
        if let Some((screen, detail)) = found
            && best.as_ref().is_none_or(|(b, _)| screen > *b)
        {
            best = Some((screen, detail));
        }
    }
    let (screen, mut detail) = best?;
    detail["screen"] = json!(screen.name());
    Some(detail)
}

/// `pub(ref:)` with a `menu` below it is the menu; without, the pub on its own.
fn pub_view(
    field: &async_graphql::parser::types::Field,
    args: &Map<String, Value>,
    variables: &Variables,
) -> (Screen, Value) {
    let venue = args.get("ref").cloned().unwrap_or(Value::Null);
    for item in &field.selection_set.node.items {
        let Selection::Field(sub) = &item.node else {
            continue;
        };
        if sub.node.name.node.as_str() != "menu" {
            continue;
        }
        let mut menu = json!({ "pub": venue });
        for (arg, value) in &sub.node.arguments {
            if let Ok(resolved) = value
                .node
                .clone()
                .into_const_with(|var: Name| variables.get(&var).cloned().ok_or(()))
            {
                menu[arg.node.as_str()] = const_value(&resolved);
            }
        }
        return (Screen::Menu, menu);
    }
    (Screen::Pubs, json!({ "pub": venue }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vars(v: Value) -> Variables {
        Variables::from_json(v)
    }

    #[test]
    fn picks_the_menu_with_its_arguments() {
        let v = view(
            "query($t: Int) { pub(ref: 254) { name menu(table: $t, search: \"ale\") { totalCount } } }",
            &vars(json!({ "t": 4 })),
            &json!({}),
            &Hints::default(),
        )
        .unwrap();
        assert_eq!(
            v,
            json!({ "screen": "menu", "pub": 254, "table": 4, "search": "ale" })
        );
    }

    #[test]
    fn money_outranks_the_basket() {
        let v = view(
            "mutation { basketAdd(pub: 254, productId: \"1\") { success } quote { quote { id } } }",
            &Variables::default(),
            &json!({ "basketAdd": { "success": true }, "quote": { "quote": { "id": "abc" } } }),
            &Hints::default(),
        )
        .unwrap();
        assert_eq!(v["screen"], "quote");
        assert_eq!(v["quoteId"], "abc");
    }

    #[test]
    fn follows_aliases_and_pay() {
        let v = view(
            "mutation($q: ID!) { p: pay(quoteId: $q, action: PREVIEW) { preview } }",
            &vars(json!({ "q": "abc" })),
            &json!({ "p": { "preview": "Pay £3" } }),
            &Hints::default(),
        )
        .unwrap();
        assert_eq!(
            v,
            json!({ "screen": "quote", "quoteId": "abc", "action": "pay", "result": { "preview": "Pay £3" } })
        );
    }

    #[test]
    fn shows_nothing_for_the_time_or_when_asked_not_to() {
        assert_eq!(
            view(
                "{ now { time } }",
                &Variables::default(),
                &json!({}),
                &Hints::default()
            ),
            None
        );
        let none = Hints {
            view: Some(ViewMode::None),
            ..Hints::default()
        };
        assert_eq!(
            view(
                "{ basket { count } }",
                &Variables::default(),
                &json!({}),
                &none
            ),
            None
        );
    }
}
