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
    CallToolResult, ContentBlock, Implementation, ProtocolVersion, ServerCapabilities, ServerConfig,
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
`products(search:)` to find one thing), build the basket (`basketAdd`, `basketSet`, `basketRemove`, `setTable`), \
then `quote`, which prices it at the till and changes nothing. Show the user the lines, table and total. \"4 pints \
of Abbot to table 4\" is a search, one `basketAdd(pub:, table: 4, productId:, portion: \"Pint\", quantity: 4)` and a \
`quote`.\n\n\
Money: `pay` is PREVIEW then CONFIRM. Show the user the preview and CONFIRM only on their explicit yes, never on \
your own; the same goes for `cancelOrder` and `sendDuplicate`. With a saved card CONFIRM pays and reports; otherwise \
it returns a payment page link to give the user. The bank may ask them to approve it in their banking app: follow \
with `order(quoteId:, waitSecs: 30)`. If `pay` asks a question (a repeat of a recent order to the same table), ask \
the user and do nothing else until they answer.\n\n\
Never guess a table: a drink is carried to whichever table is given. Ask when the user hasn't said one in this \
conversation, and check again if they may have moved. A garden or another part of the pub can have its own menu and \
prices, so read the menu for the table.\n\n\
The basket belongs to this conversation, and the server keeps no history between conversations, so take the pub, \
table and usual order from this one. For \"another\" or \"same again\", `basketFromQuote` with the last quote \
(`quotes` lists this conversation's), check the table still stands, then `quote`.\n\n\
The till's price is what's charged; when it differs from the menu (`differsFromMenu`), say so. A pub's menu is read \
once per area per day, after which searching and filtering it are free; field descriptions say what the rest costs. \
`now` gives the time in London for free.";

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

    pub async fn execute(
        &self,
        caller: Caller,
        query: &str,
        variables: Option<&str>,
    ) -> Result<async_graphql::Response, String> {
        let mut request =
            graphql::request(query, self.service.clone(), self.session.clone(), caller);
        if let Some(vars) = variables.filter(|v| !v.trim().is_empty()) {
            match serde_json::from_str::<serde_json::Value>(vars) {
                Ok(v @ serde_json::Value::Object(_)) => {
                    request = request.variables(async_graphql::Variables::from_json(v));
                }
                Ok(_) => return Err("variables must be a JSON object".to_owned()),
                Err(e) => return Err(format!("invalid variables JSON: {e}")),
            }
        }
        Ok(self.schema.execute(request).await)
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
                       an order. Paying, cancelling and sending a held repeat order are PREVIEW then CONFIRM, and \
                       CONFIRM only ever follows the user's explicit yes. Get the schema from `spoons_schema` \
                       first. Variables are a JSON string.",
        annotations(destructive_hint = true, open_world_hint = true)
    )]
    async fn spoons(
        &self,
        ctx: RequestContext<RoleServer>,
        Parameters(req): Parameters<GraphqlRequest>,
    ) -> ToolResult {
        let caller = self.caller(&ctx);
        match self
            .execute(caller, &req.query, req.variables.as_deref())
            .await
        {
            Ok(response) => {
                let json = serde_json::to_string_pretty(&response)
                    .unwrap_or_else(|e| format!("{{\"error\": \"{e}\"}}"));
                Ok(CallToolResult::success(vec![ContentBlock::text(json)]))
            }
            Err(error) => Ok(CallToolResult::error(vec![ContentBlock::text(error)])),
        }
    }
}

#[tool_handler]
impl ServerHandler for Spoons {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_protocol_version(ProtocolVersion::LATEST)
            .with_server_info(
                Implementation::new("spoons", env!("CARGO_PKG_VERSION"))
                    .with_title("Wetherspoon's")
                    .with_website_url("https://github.com/radiosilence/spoons-mcp"),
            )
            .with_instructions(INSTRUCTIONS)
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
