//! Quotes and their payments, kept on disk so they survive the server restarting and are shared by every server
//! process: cancelling a payment needs the device id that started it, and a payment must never be prepared twice.

use std::fs::{self, File, OpenOptions};
use std::io::{ErrorKind, Write};
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

/// Order files that never reached the payment API, or were cancelled, are removed after this long.
const KEEP: Duration = Duration::from_secs(24 * 60 * 60);

pub struct Store {
    dir: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Order {
    pub quote_id: String,
    pub created_at: u64,
    pub franchise: String,
    pub venue_ref: u64,
    pub venue_name: String,
    pub sales_area: u64,
    pub table: u32,
    pub lines: Vec<Line>,
    pub total: f64,
    pub currency: String,
    pub token: String,
    pub payment: Payment,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Line {
    pub product_id: u64,
    pub name: String,
    pub portion: String,
    pub quantity: u32,
    pub price: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum Payment {
    None,
    /// Written before the request to prepare is sent, so a server that dies mid-request leaves it marked as unknown.
    Preparing,
    /// The request to prepare got no clear answer: an order may exist.
    Unconfirmed {
        detail: String,
    },
    Prepared {
        transaction_id: String,
        url: String,
        duplicate_order: bool,
        url_opened: bool,
    },
    Cancelled,
}

pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs())
}

impl Store {
    /// `$XDG_STATE_HOME/spoons`, or `~/.local/state/spoons`.
    pub fn open() -> Result<Self> {
        let base = match std::env::var_os("XDG_STATE_HOME") {
            Some(dir) if !dir.is_empty() => PathBuf::from(dir),
            _ => PathBuf::from(std::env::var_os("HOME").context("HOME is not set")?)
                .join(".local/state"),
        };
        Self::at(base.join("spoons"))
    }

    pub fn at(dir: PathBuf) -> Result<Self> {
        fs::create_dir_all(dir.join("orders"))
            .with_context(|| format!("creating {}", dir.display()))?;
        Ok(Self { dir })
    }

    /// The id this install gives the payment API, made once.
    pub fn device_id(&self) -> Result<String> {
        let path = self.dir.join("device-id");
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(mut file) => {
                let id = uuid::Uuid::new_v4().to_string();
                file.write_all(id.as_bytes())?;
                Ok(id)
            }
            Err(error) if error.kind() == ErrorKind::AlreadyExists => {
                let id = fs::read_to_string(&path)?.trim().to_owned();
                if id.is_empty() {
                    bail!("{} is empty; remove it to make a new one", path.display());
                }
                Ok(id)
            }
            Err(error) => Err(error.into()),
        }
    }

    fn path(&self, quote_id: &str) -> Result<PathBuf> {
        if quote_id.is_empty() || !quote_id.chars().all(|c| c.is_ascii_alphanumeric()) {
            bail!("no such quote: {quote_id}");
        }
        Ok(self.dir.join("orders").join(format!("{quote_id}.json")))
    }

    /// Holds a quote for one caller at a time, across processes, until the returned file is dropped. The lock is on
    /// a file of its own, since saving replaces the order's file.
    pub fn lock(&self, quote_id: &str) -> Result<File> {
        let path = self.path(quote_id)?.with_extension("lock");
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&path)?;
        file.lock()?;
        Ok(file)
    }

    pub fn load(&self, quote_id: &str) -> Result<Order> {
        let path = self.path(quote_id)?;
        let bytes = match fs::read(&path) {
            Err(error) if error.kind() == ErrorKind::NotFound => bail!("no such quote: {quote_id}"),
            read => read?,
        };
        Ok(serde_json::from_slice(&bytes)?)
    }

    pub fn save(&self, order: &Order) -> Result<()> {
        let path = self.path(&order.quote_id)?;
        let partial = path.with_extension("json.partial");
        fs::write(&partial, serde_json::to_vec_pretty(order)?)?;
        fs::rename(&partial, &path)?;
        Ok(())
    }

    /// Removes old quotes that were never paid for, and old cancelled ones. Anything that reached the payment API is
    /// kept, since it may still be needed to check on or cancel.
    pub fn prune(&self) -> Result<()> {
        for entry in fs::read_dir(self.dir.join("orders"))? {
            let path = entry?.path();
            if path.extension().is_none_or(|extension| extension != "json") {
                continue;
            }
            let Some(quote_id) = path.file_stem().and_then(|stem| stem.to_str()) else {
                continue;
            };
            let _lock = self.lock(quote_id)?;
            let Ok(order) = self.load(quote_id) else {
                continue;
            };
            let old = now().saturating_sub(order.created_at) > KEEP.as_secs();
            if old && matches!(order.payment, Payment::None | Payment::Cancelled) {
                fs::remove_file(&path)?;
                fs::remove_file(path.with_extension("lock")).ok();
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn order(quote_id: &str, created_at: u64, payment: Payment) -> Order {
        Order {
            quote_id: quote_id.to_owned(),
            created_at,
            franchise: "jdw".into(),
            venue_ref: 254,
            venue_name: "The Watch House".into(),
            sales_area: 443,
            table: 17,
            lines: vec![],
            total: 4.22,
            currency: "GBP".into(),
            token: "t".into(),
            payment,
        }
    }

    fn store() -> (Store, PathBuf) {
        let dir = std::env::temp_dir().join(format!("spoons-test-{}", uuid::Uuid::new_v4()));
        (Store::at(dir.clone()).unwrap(), dir)
    }

    #[test]
    fn keeps_the_device_id() {
        let (store, dir) = store();
        let id = store.device_id().unwrap();
        assert_eq!(store.device_id().unwrap(), id);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn saves_and_loads_an_order() {
        let (store, dir) = store();
        let prepared = Payment::Prepared {
            transaction_id: "025420261101100256ACC62AC162F2".into(),
            url: "https://paymentpage.axepta.bnpparibas/payssl.aspx".into(),
            duplicate_order: false,
            url_opened: true,
        };
        store.save(&order("abc123", now(), prepared)).unwrap();
        let loaded = store.load("abc123").unwrap();
        assert!(matches!(
            loaded.payment,
            Payment::Prepared {
                url_opened: true,
                ..
            }
        ));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn refuses_quote_ids_that_are_paths() {
        let (store, dir) = store();
        assert!(store.load("../device-id").is_err());
        assert!(store.lock("").is_err());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn prunes_only_old_orders_that_never_reached_the_payment_api() {
        let (store, dir) = store();
        let old = now() - KEEP.as_secs() - 1;
        store.save(&order("oldnone", old, Payment::None)).unwrap();
        store
            .save(&order("oldcancelled", old, Payment::Cancelled))
            .unwrap();
        store
            .save(&order("oldpreparing", old, Payment::Preparing))
            .unwrap();
        store.save(&order("newnone", now(), Payment::None)).unwrap();
        store.prune().unwrap();
        assert!(store.load("oldnone").is_err());
        assert!(store.load("oldcancelled").is_err());
        assert!(store.load("oldpreparing").is_ok());
        assert!(store.load("newnone").is_ok());
        fs::remove_dir_all(dir).unwrap();
    }
}
