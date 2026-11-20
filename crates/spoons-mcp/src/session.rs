//! What one MCP session (one conversation) is ordering: its basket, and the quotes it has made.
//!
//! Kept in memory, per session, so the model and anything rendering the session's results share one basket, and
//! nothing outlives the conversation: the server keeps no history between conversations. Quotes themselves are on
//! disk ([`crate::orders::Store`]), since a payment must be checkable after a restart; only the list of which ones
//! this session made lives here.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, Weak};

use tokio::sync::watch;

use crate::catalogue::Kind;
use crate::service::PubRef;

/// One product in one size.
#[derive(Clone, Debug)]
pub struct Item {
    pub product_id: u64,
    pub name: String,
    /// The size's label as the menu has it, which is what the till matches.
    pub portion: String,
    pub quantity: u32,
    /// One of it as the menu prices it at the basket's table, or the pub's first sales area before there is one.
    /// `None` when it isn't on that table's menu.
    pub unit_price: Option<f64>,
    pub kind: Kind,
    pub colour: Option<crate::catalogue::Colour>,
    /// Why it can't be quoted as it stands, such as being off the menu for the table.
    pub note: Option<String>,
}

impl Item {
    pub fn id(&self) -> String {
        line_id(self.product_id, &self.portion)
    }
}

pub fn line_id(product_id: u64, portion: &str) -> String {
    format!("{product_id}:{}", portion.to_lowercase())
}

#[derive(Clone, Debug, Default)]
pub struct Basket {
    pub pub_ref: Option<PubRef>,
    pub table: Option<u32>,
    /// The sales area the prices are from.
    pub sales_area: Option<u64>,
    pub items: Vec<Item>,
    /// Goes up with every change, so a view can tell whether what it shows is current.
    pub version: u64,
}

impl Basket {
    pub fn count(&self) -> u32 {
        self.items.iter().map(|item| item.quantity).sum()
    }

    /// The menu's total, when every line has a menu price. The till's is the quote's.
    pub fn total(&self) -> Option<f64> {
        self.items
            .iter()
            .map(|item| {
                item.unit_price
                    .map(|price| price * f64::from(item.quantity))
            })
            .sum::<Option<f64>>()
            .map(|total| (total * 100.0).round() / 100.0)
    }
}

#[derive(Debug, Default)]
pub struct State {
    pub basket: Basket,
    /// Quote ids made in this session, oldest first.
    pub quotes: Vec<String>,
}

pub struct Session {
    /// Lets the app's own calls reach this basket even if the host gives them an MCP session of their own.
    pub id: String,
    state: Mutex<State>,
    version: watch::Sender<u64>,
}

impl Default for Session {
    fn default() -> Self {
        Self {
            id: uuid::Uuid::new_v4().simple().to_string(),
            state: Mutex::default(),
            version: watch::channel(0).0,
        }
    }
}

/// Live sessions by id, each with the receipt address it belongs to, so an app's call can name its conversation's
/// basket and nobody else's.
#[derive(Default)]
pub struct Registry {
    sessions: Mutex<HashMap<String, (String, Weak<Session>)>>,
}

impl Registry {
    pub fn register(&self, session: &Arc<Session>, tenant: &str) {
        let mut sessions = self.sessions.lock().unwrap();
        sessions.retain(|_, (_, s)| s.strong_count() > 0);
        sessions
            .entry(session.id.clone())
            .or_insert_with(|| (tenant.to_owned(), Arc::downgrade(session)));
    }

    /// The session with this id, if it's alive and the caller's own.
    pub fn find(&self, id: &str, tenant: &str) -> Option<Arc<Session>> {
        let sessions = self.sessions.lock().unwrap();
        let (owner, session) = sessions.get(id)?;
        (owner == tenant).then(|| session.upgrade()).flatten()
    }
}

impl Session {
    pub fn basket(&self) -> Basket {
        self.state.lock().unwrap().basket.clone()
    }

    pub fn quotes(&self) -> Vec<String> {
        self.state.lock().unwrap().quotes.clone()
    }

    pub fn remember_quote(&self, quote_id: &str) {
        self.state.lock().unwrap().quotes.push(quote_id.to_owned());
    }

    /// Changes the basket, all or nothing: an error leaves it as it was.
    pub fn change<T>(
        &self,
        change: impl FnOnce(&mut Basket) -> Result<T, String>,
    ) -> Result<T, String> {
        let mut state = self.state.lock().unwrap();
        let mut basket = state.basket.clone();
        let result = change(&mut basket)?;
        basket.version = state.basket.version + 1;
        let version = basket.version;
        state.basket = basket;
        drop(state);
        self.version.send_replace(version);
        Ok(result)
    }

    /// Notified on every change, for a view that waits for one.
    pub fn watch(&self) -> watch::Receiver<u64> {
        self.version.subscribe()
    }
}
