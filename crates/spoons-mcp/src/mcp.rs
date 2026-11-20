//! MCP: two tools, as the sibling servers have — the schema, and a query.
//!
//! The schema stays behind a tool call rather than in the always-loaded tool descriptions, so a session that never
//! mentions the pub pays almost nothing for having this connected. Each MCP session gets its own basket; the
//! service underneath (menus, the pub list, quotes on disk) is shared by all of them.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use rmcp::handler::server::{router::tool::ToolRouter, wrapper::Parameters};
use rmcp::model::{
    CallToolResult, ContentBlock, Implementation, ListResourcesResult, PaginatedRequestParams,
    ProtocolVersion, ReadResourceRequestParams, ReadResourceResponse, ReadResourceResult, Resource,
    ResourceContents, ServerCapabilities, ServerConfig,
};
use rmcp::service::{RequestContext, RoleServer};
use rmcp::{
    ErrorData as McpError, ServerHandler, ServiceExt, schemars, tool, tool_handler, tool_router,
};
use spoons_api_client::{Card, Client};

use crate::graphql::{self, Caller, SpoonsSchema};
use crate::orders::Store;
use crate::service::Service;
use crate::session::Session;

type ToolResult = std::result::Result<CallToolResult, McpError>;

/// Over HTTP, where the receipt goes, per request: the gateway in front injects each user's own.
const EMAIL_HEADER: &str = "x-spoons-email";
/// Over HTTP, the card `pay` enters on the payment page: all four, or none, in which case the user pays on the page.
const CARD_HEADERS: [&str; 4] = [
    "x-spoons-card-number",
    "x-spoons-card-expiry",
    "x-spoons-card-cvc",
    "x-spoons-card-name",
];

pub const INSTRUCTIONS: &str = "Wetherspoon's order-and-pay, as a GraphQL API: find a pub, read its menu and order \
food and drink to a table. Read the schema once with `spoons_schema`, then query and mutate with `spoons`. Variables \
go as a JSON string.\n\n\
Ordering: find the pub (`pubs(query:)`, or `pubs(near:)` nearest first), read what's on \
(`pub(ref:) { menu(table:) { sections { label products { id name abv portions { name price { formatted } } } } } }`; \
`guestAles` for the guest casks, which are named \"Brewery - Beer\" and have no \"ale\" in the name; \
`products(search:)` to find one thing, `categories` and `products(category:)` for the pub's own menu sections), build the basket (`basketAdd`, `basketSet`, `basketRemove`, `setTable`), \
then `quote`, which prices it at the till and changes nothing. Show the user the lines, table and total. \"4 pints \
of Abbot to table 4\" is a search, one `basketAdd(pub:, table: 4, productId:, portion: \"Pint\", quantity: 4)` and a \
`quote`.\n\n\
Paying is yours to do: when the user says yes to the quote (\"yes\", \"pay\", \"go on\"), or asked to pay \
with the order (\"and pay\"), call `pay` with PREVIEW and then straight away CONFIRM with its token, and tell them \
it's done; don't ask again, and don't send them to the app to pay. The user has set this server up to pay for them: \
the saved card is configured on the server, which enters it on the payment page itself, so you never see or type \
card details and there's nothing for you to fill in. Without a saved card, CONFIRM returns a payment page link to \
give the user. Just don't pay without a yes, and give `cancelOrder` and `sendDuplicate` the same yes first. The bank may ask them to approve it in their banking app: follow \
with `order(quoteId:, waitSecs: 30)`. If `pay` asks a question (a repeat of a recent order to the same table), ask \
the user and do nothing else until they answer.\n\n\
Some products ask a question first (`optionGroups`): a spirit's mixer, a meal's side or peas. Pass the \
user's words as `choices` (\"chips\", \"mushy peas\", \"lemonade\", \"neat\"); if one's missing or could be two \
things, the error lists the options, so ask the user rather than picking for them. A meal whose `deals` include a \
drink comes with one from `includedDrinks`: offer it, and pass the user's pick as `includeDrink`; the till takes \
the deal off, shown as the quote's `savings`. Multi-buys (\"Any 2 for £6.60\") apply by themselves.\n\n\
Tables: a drink is carried to whichever table is given, so never make one up. Ask only when no table has been \
given in this conversation at all. Once the user has given one, it sticks: the server keeps the pub and table \
(`setTable`, `basketAdd(table:)` or a quote's), and `quote` uses them when `table` is left out, so reuse them for \
every later order without asking. Check the table again only on a clear sign it has changed: the user says they've \
moved or names another table, the order is for another pub, or hours have passed (`now` against the last quote's \
`createdAt`) so it's plausibly a new visit. People order by voice in a noisy pub: snatches of other conversations \
(\"I've got a different number\") are noise, not a move, so don't stop to ask about them. A garden or another part \
of the pub can have its own menu and prices, so read the menu for the table.\n\n\
For the menu as a document (\"send me the menu\", \"a markdown menu of the pub I'm in\"), \
`pub { menu { markdown } }`: with no `ref` or `table` it is the conversation's pub and table; `kinds`, `search` \
and `byCategory` narrow or regroup it. Give it to the user as it comes.\n\n\
Search forgives what dictation mangles: pass what the user said (\"broccoli barge\", \"sumo\", \"a mocktail\", \
\"alcohol free lager\") rather than guessing names; results come best first, and `closeMatches` says when none \
had every word, so say they're the nearest. Page long lists with `first` and `after`.\n\n\
The basket belongs to this conversation, and the server keeps no history between conversations, so take the pub, \
table and usual order from this one. For \"another\" or \"same again\", `basketFromQuote` with the last quote \
(`quotes` lists this conversation's), then `quote`, to the same table.\n\n\
The till's price is what's charged; when it differs from the menu (`differsFromMenu`), say so. A pub's menu is read \
once per area per day, after which searching and filtering it are free; field descriptions say what the rest costs. \
`now` gives the time in London for free.\n\n\
Hosts that show apps draw a Wetherspoon's app beside `spoons` results: pubs, the menu, the basket, a quote or an \
order, whichever the result is about. Steer it with the tool's optional `ui` argument (`view: COMPACT` for a quick \
card, `FULL`, or `NONE` for nothing; `focus` a kind such as `CASK_ALE` or a product id; `highlight` product ids). \
The user can also build the basket and pay there, with the quote's \"Confirm payment\" button; that's a \
shortcut, not the only way, and a yes in the chat is just as good. What they do there reaches you as context: if \
they've paid in the app, don't pay that quote again, follow it with `order`. Keep the reply readable \
without the app: not every host shows it.";

#[derive(Clone)]
pub struct Spoons {
    service: Arc<Service>,
    session: Arc<Session>,
    schema: SpoonsSchema,
    /// Over stdio, where the receipt goes.
    email: Option<String>,
    #[allow(dead_code)] // read by the #[tool_handler] expansion
    tool_router: ToolRouter<Self>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct GraphqlRequest {
    /// The GraphQL query or mutation.
    pub query: String,
    /// Variables, as a JSON object encoded in a string.
    pub variables: Option<String>,
    /// How hosts that show the Wetherspoon's app should present this result. Optional; leave it out unless the
    /// user wants something particular on screen.
    #[serde(default, deserialize_with = "lenient_hints")]
    pub ui: Option<crate::ui::Hints>,
}

/// `ui` as an object, or as a JSON string the way `variables` goes, which models do by habit. Hints only steer
/// the app, so ones that can't be read are dropped rather than failing the call.
fn lenient_hints<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<crate::ui::Hints>, D::Error> {
    let value = <serde_json::Value as serde::Deserialize>::deserialize(deserializer)?;
    let value = match value {
        serde_json::Value::String(text) if text.trim().is_empty() => return Ok(None),
        serde_json::Value::String(text) => match serde_json::from_str(&text) {
            Ok(parsed) => parsed,
            Err(_) => return Ok(None),
        },
        other => other,
    };
    if value.is_null() {
        return Ok(None);
    }
    Ok(serde_json::from_value(value).ok())
}

impl Spoons {
    pub fn new(service: Arc<Service>, schema: SpoonsSchema, email: Option<String>) -> Self {
        Self {
            service,
            session: Arc::default(),
            schema,
            email,
            tool_router: Self::tool_router(),
        }
    }

    /// The same server for a new MCP session: shared service, a basket of its own.
    pub fn for_new_session(&self) -> Self {
        Self {
            session: Arc::default(),
            ..self.clone()
        }
    }

    /// The basket a call works on: its own MCP session's, or the one an app names, if it belongs to the caller.
    fn session_for(&self, caller: &Caller, hints: Option<&crate::ui::Hints>) -> Arc<Session> {
        self.service
            .sessions
            .register(&self.session, caller.tenant());
        hints
            .and_then(|h| h.session.as_deref())
            .and_then(|id| self.service.sessions.find(id, caller.tenant()))
            .unwrap_or_else(|| self.session.clone())
    }

    /// Where this request's receipt goes. Over HTTP only its `X-Spoons-Email` header counts, since the server is
    /// shared and the gateway in front injects each user's own; over stdio, `SPOONS_EMAIL`.
    fn caller(&self, ctx: &RequestContext<RoleServer>) -> Caller {
        let Some(parts) = ctx.extensions.get::<http::request::Parts>() else {
            return Caller {
                email: self.email.clone(),
                card: Ok(None),
            };
        };
        caller_from_headers(&parts.headers)
    }

    #[cfg(test)]
    pub async fn execute(
        &self,
        caller: Caller,
        query: &str,
        variables: Option<&str>,
    ) -> Result<async_graphql::Response, String> {
        let variables = parse_variables(variables)?;
        Ok(self
            .execute_in(self.session.clone(), caller, query, &variables)
            .await)
    }

    async fn execute_in(
        &self,
        session: Arc<Session>,
        caller: Caller,
        query: &str,
        variables: &async_graphql::Variables,
    ) -> async_graphql::Response {
        let request = graphql::request(query, self.service.clone(), session, caller)
            .variables(variables.clone());
        self.schema.execute(request).await
    }

    /// A tool result: the response as text for everyone, and as structured content with the screen that fits it
    /// for hosts showing the app.
    pub async fn run_tool(&self, caller: Caller, req: &GraphqlRequest) -> CallToolResult {
        let variables = match parse_variables(req.variables.as_deref()) {
            Ok(v) => v,
            Err(error) => return CallToolResult::error(vec![ContentBlock::text(error)]),
        };
        let session = self.session_for(&caller, req.ui.as_ref());
        let response = self
            .execute_in(session.clone(), caller, &req.query, &variables)
            .await;
        let body = serde_json::to_value(&response).unwrap_or_default();
        let hints = req.ui.clone().unwrap_or_default();
        // The app's own calls answer it once, compactly: a menu is hundreds of kilobytes, and hosts stop handing
        // large results to apps. The model's calls read as before, and carry for the app only what screen fits,
        // since the structured copy isn't shown to the model and the app reads its own data.
        let for_app = hints.view == Some(crate::ui::ViewMode::None);
        let text = if for_app {
            serde_json::to_string(&body)
        } else {
            serde_json::to_string_pretty(&body)
        }
        .unwrap_or_else(|e| format!("{{\"error\": \"{e}\"}}"));
        let data = body.get("data").cloned().unwrap_or_default();
        let mut view = crate::ui::view(&req.query, &variables, &data, &hints);
        // `pub` without a ref is the conversation's pub: the app needs its ref all the same.
        if let Some(view) = view
            .as_mut()
            .filter(|v| v.get("pub").is_some_and(serde_json::Value::is_null))
            && let Some(here) = session.basket().pub_ref
        {
            view["pub"] = serde_json::json!(here.venue_ref);
        }
        // A menu opened without a table is the conversation's table, when it's at that pub: the app adds to it.
        if let Some(view) = view
            .as_mut()
            .filter(|v| v["screen"] == "menu" && v["table"].is_null())
        {
            let basket = session.basket();
            if let (Some(here), Some(table)) = (&basket.pub_ref, basket.table)
                && view["pub"] == serde_json::json!(here.venue_ref)
            {
                view["table"] = serde_json::json!(table);
            }
        }
        let structured = serde_json::json!({
            "view": view,
            "ui": hints,
            "session": session.id,
        });
        let mut result = CallToolResult::success(vec![ContentBlock::text(text)]);
        result.structured_content = Some(structured);
        result
    }
}

fn parse_variables(variables: Option<&str>) -> Result<async_graphql::Variables, String> {
    match variables.filter(|v| !v.trim().is_empty()) {
        None => Ok(async_graphql::Variables::default()),
        Some(vars) => match serde_json::from_str::<serde_json::Value>(vars) {
            Ok(v @ serde_json::Value::Object(_)) => Ok(async_graphql::Variables::from_json(v)),
            Ok(_) => Err("variables must be a JSON object".to_owned()),
            Err(e) => Err(format!("invalid variables JSON: {e}")),
        },
    }
}

/// The caller a request's headers describe. A card configured in part is refused rather than quietly falling back
/// to the payment page.
pub fn caller_from_headers(headers: &http::HeaderMap) -> Caller {
    let get = |name: &str| {
        headers
            .get(name)
            .and_then(|value| value.to_str().ok())
            .map(str::trim)
            .filter(|value| !value.is_empty())
    };
    let card = match CARD_HEADERS.map(get) {
        [None, None, None, None] => Ok(None),
        [Some(number), Some(expiry), Some(cvc), Some(name)] => Card::new(number, expiry, cvc, name)
            .map(Some)
            .map_err(|error| format!("the saved card is unusable: {error}")),
        _ => Err("the saved card is missing some of its number, expiry, CVC and name".to_owned()),
    };
    Caller {
        email: get(EMAIL_HEADER).map(str::to_owned),
        card,
    }
}

#[tool_router]
impl Spoons {
    #[tool(
        name = "spoons_schema",
        title = "Wetherspoon's schema",
        description = "The Wetherspoon's ordering API's GraphQL schema. Call once before the first `spoons` query.",
        annotations(read_only_hint = true)
    )]
    async fn spoons_schema(&self) -> ToolResult {
        Ok(CallToolResult::success(vec![ContentBlock::text(
            graphql::sdl(),
        )]))
    }

    #[tool(
        name = "spoons",
        title = "Wetherspoon's",
        description = "Run a GraphQL query or mutation against Wetherspoon's order-and-pay: find a pub, read its full \
                       menu (real and guest ales, lager, cider, wine, spirits, alcohol-free, soft and hot drinks, \
                       food) with prices for a table, keep a basket, price it at the till, and pay, cancel or track \
                       an order. To pay, on the user's yes, call `pay` PREVIEW then CONFIRM: the server enters \
                       the saved card itself, so you never handle card details. The pub and table stick for the conversation once given. Get the schema from `spoons_schema` \
                       first. Variables are a JSON string.",
        // Not marked destructive: most calls read the menu or change the conversation's own basket, and hosts
        // ask the user before every call to a destructive tool. What does reach the pub (pay, cancelOrder,
        // sendDuplicate) is PREVIEW then CONFIRM with a token, on the user's say-so in the chat.
        annotations(destructive_hint = false, open_world_hint = true),
        meta = crate::ui::tool_meta()
    )]
    async fn spoons(
        &self,
        ctx: RequestContext<RoleServer>,
        Parameters(req): Parameters<GraphqlRequest>,
    ) -> ToolResult {
        Ok(self.run_tool(self.caller(&ctx), &req).await)
    }
}

#[tool_handler]
impl ServerHandler for Spoons {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(
            ServerCapabilities::builder()
                .enable_tools()
                .enable_resources()
                .build(),
        )
        .with_protocol_version(ProtocolVersion::LATEST)
        .with_server_info(
            Implementation::new("spoons", env!("CARGO_PKG_VERSION"))
                .with_title("Wetherspoon's")
                .with_website_url("https://github.com/radiosilence/spoons-mcp"),
        )
        .with_instructions(INSTRUCTIONS)
    }

    async fn list_resources(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> std::result::Result<ListResourcesResult, McpError> {
        let resource = Resource::new(crate::ui::URI, "Wetherspoon's")
            .with_title("Wetherspoon's ordering")
            .with_description(
                "Pubs, the menu, the basket, quotes and orders, for hosts that show MCP apps.",
            )
            .with_mime_type(crate::ui::MIME)
            .with_meta(crate::ui::resource_meta());
        Ok(ListResourcesResult::with_all_items(vec![resource]))
    }

    async fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> std::result::Result<ReadResourceResponse, McpError> {
        if request.uri != crate::ui::URI {
            return Err(McpError::resource_not_found(
                format!("no resource {}", request.uri),
                None,
            ));
        }
        let contents = ResourceContents::text(crate::ui::html(), crate::ui::URI)
            .with_mime_type(crate::ui::MIME)
            .with_meta(crate::ui::resource_meta());
        Ok(ReadResourceResult::new(vec![contents]).into())
    }
}

pub async fn serve(http: Option<&str>, state: Option<PathBuf>) -> Result<()> {
    let store = match state {
        Some(dir) => Store::at(dir)?,
        None => Store::open()?,
    };
    if let Err(error) = store.prune() {
        tracing::warn!(%error, "couldn't prune old quotes");
    }
    let client = Client::discover()
        .await?
        .with_interval(Duration::from_millis(100));
    let payments = client.payments(&store.device_id()?)?;
    let service = Arc::new(Service::new(client, payments, store));
    let email = std::env::var("SPOONS_EMAIL")
        .ok()
        .filter(|email| !email.is_empty());
    let server = Spoons::new(service, graphql::schema(), email);
    match http {
        None => {
            server
                .serve(rmcp::transport::stdio())
                .await?
                .waiting()
                .await?;
        }
        Some(addr) => {
            use rmcp::transport::streamable_http_server::{
                StreamableHttpServerConfig, StreamableHttpService,
                session::local::LocalSessionManager,
            };
            // The Host allowlist guards a browser reaching a server on localhost; behind a proxy it only rejects
            // the internal Host the proxy forwards. Each MCP session gets a basket of its own.
            let service = StreamableHttpService::new(
                move || Ok(server.for_new_session()),
                Arc::new(LocalSessionManager::default()),
                StreamableHttpServerConfig::default().disable_allowed_hosts(),
            );
            let router = axum::Router::new().nest_service("/mcp", service);
            let listener = tokio::net::TcpListener::bind(addr).await?;
            tracing::info!("MCP over streamable HTTP on http://{addr}/mcp");
            axum::serve(listener, router).await?;
        }
    }
    Ok(())
}
