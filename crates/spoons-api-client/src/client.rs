use std::sync::Arc;
use std::time::Duration;

use regex::Regex;
use reqwest::StatusCode;
use reqwest::header::{
    AUTHORIZATION, CACHE_CONTROL, HeaderMap, HeaderName, HeaderValue, ORIGIN, REFERER,
};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use tokio::sync::Mutex;
use tokio::time::{Instant, sleep, sleep_until};

use crate::models::{Ale, Menu, MenuSummary, Venue, VenueSummary, WaitTimes};

const APP: &str = "https://order.jdwetherspoon.com";
const DEFAULT_BASE: &str = "https://ca.jdw-apps.net/api/v0.1";
/// The API's firewall turns away anything that doesn't look like the order-and-pay site, which runs in a browser, so
/// requests carry what that site sends: a browser's User-Agent, its origin, and the app version it reports.
const USER_AGENT: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/26.0 Safari/605.1.15";
/// What the site reported when this was written; the live one is read from the bundle.
const APP_VERSION: &str = "6.9.1";
/// Every route but the venue list sits under the pub's franchise, and 404s under any other. Most pubs are `jdw`; the
/// rest are Wetherspoon bars run for someone else, on holiday parks and university campuses.
pub const FRANCHISE: &str = "jdw";

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("couldn't find the API key in the app's bundle; it may have moved")]
    NoKey,
    #[error(transparent)]
    Http(#[from] reqwest::Error),
    #[error("{status} from {path}: {body}")]
    Status {
        status: u16,
        path: String,
        body: String,
    },
    #[error("unexpected response from {path}: {source}")]
    Decode {
        path: String,
        source: serde_json::Error,
    },
}

/// Requests are paced rather than fired at will: this is somebody else's unofficial API, and a dump of every pub is
/// thousands of calls.
#[derive(Clone)]
pub struct Client {
    http: reqwest::Client,
    base: String,
    next: Arc<Mutex<Instant>>,
    interval: Duration,
}

#[derive(Deserialize)]
struct Envelope<T> {
    data: T,
}

impl Client {
    /// Reads the key from the app's bundle, or from `SPOONS_API_KEY` when set.
    pub async fn discover() -> Result<Self, Error> {
        if let Ok(key) = std::env::var("SPOONS_API_KEY") {
            return Self::new(&key, DEFAULT_BASE, APP_VERSION);
        }
        let http = reqwest::Client::builder().user_agent(USER_AGENT).build()?;
        let shell = http
            .get(APP)
            .send()
            .await?
            .error_for_status()?
            .text()
            .await?;
        let bundle = Regex::new(r"/assets/index-[\w-]+\.js")
            .expect("valid pattern")
            .find(&shell)
            .ok_or(Error::NoKey)?
            .as_str()
            .to_owned();
        let bundle = http
            .get(format!("{APP}{bundle}"))
            .send()
            .await?
            .error_for_status()?
            .text()
            .await?;
        let key = setting(&bundle, "VITE_API_PROXY_KEY").ok_or(Error::NoKey)?;
        let base = match (
            setting(&bundle, "VITE_API_PROXY_BASE_URL"),
            setting(&bundle, "VITE_API_PROXY_VERSION"),
        ) {
            (Some(url), Some(version)) => format!("{url}/{version}"),
            _ => DEFAULT_BASE.to_owned(),
        };
        let version = app_version(&bundle).unwrap_or_else(|| APP_VERSION.to_owned());
        Self::new(&key, &base, &version)
    }

    pub fn new(key: &str, base: &str, app_version: &str) -> Result<Self, Error> {
        let mut headers = HeaderMap::new();
        let bearer = HeaderValue::from_str(&format!("Bearer {key}")).map_err(|_| Error::NoKey)?;
        headers.insert(AUTHORIZATION, bearer);
        headers.insert(CACHE_CONTROL, HeaderValue::from_static("no-cache"));
        headers.insert(ORIGIN, HeaderValue::from_static(APP));
        headers.insert(
            REFERER,
            HeaderValue::from_static("https://order.jdwetherspoon.com/"),
        );
        headers.insert(
            HeaderName::from_static("app-version"),
            HeaderValue::from_str(app_version).unwrap_or(HeaderValue::from_static(APP_VERSION)),
        );
        let http = reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .default_headers(headers)
            .timeout(Duration::from_secs(60))
            .build()?;
        Ok(Self {
            http,
            base: base.trim_end_matches('/').to_owned(),
            next: Arc::new(Mutex::new(Instant::now())),
            interval: Duration::from_millis(500),
        })
    }

    /// The least time between two requests.
    pub fn with_interval(mut self, interval: Duration) -> Self {
        self.interval = interval;
        self
    }

    async fn pace(&self) {
        let mut next = self.next.lock().await;
        let now = Instant::now();
        if *next > now {
            sleep_until(*next).await;
        }
        *next = Instant::now() + self.interval;
    }

    /// The body as sent, for keeping. Rate limits and server errors are retried with a growing wait.
    pub async fn raw(&self, path: &str) -> Result<Vec<u8>, Error> {
        let url = format!("{}/{}", self.base, path);
        let mut wait = Duration::from_secs(2);
        for attempt in 0.. {
            self.pace().await;
            let response = self.http.get(&url).send().await?;
            let status = response.status();
            if status.is_success() {
                return Ok(response.bytes().await?.to_vec());
            }
            let retry = status == StatusCode::TOO_MANY_REQUESTS || status.is_server_error();
            if !retry || attempt == 4 {
                return Err(Error::Status {
                    status: status.as_u16(),
                    path: path.to_owned(),
                    body: response.text().await.unwrap_or_default(),
                });
            }
            tracing::warn!(%status, path, "retrying in {wait:?}");
            sleep(wait).await;
            wait *= 2;
        }
        unreachable!()
    }

    async fn get<T: DeserializeOwned>(&self, path: &str) -> Result<T, Error> {
        let bytes = self.raw(path).await?;
        serde_json::from_slice::<Envelope<T>>(&bytes)
            .map(|envelope| envelope.data)
            .map_err(|source| Error::Decode {
                path: path.to_owned(),
                source,
            })
    }

    pub async fn venues(&self) -> Result<Vec<VenueSummary>, Error> {
        self.get(&paths::venues()).await
    }

    pub async fn venue(&self, franchise: &str, venue: u64) -> Result<Venue, Error> {
        self.get(&paths::venue(franchise, venue)).await
    }

    /// The real ales on, and those coming soon.
    pub async fn ales(&self, franchise: &str, venue: u64) -> Result<Vec<Ale>, Error> {
        self.get(&paths::ales(franchise, venue)).await
    }

    /// How long an order is taking, in minutes. Live.
    pub async fn wait_times(&self, franchise: &str, venue: u64) -> Result<WaitTimes, Error> {
        self.get(&paths::wait_times(franchise, venue)).await
    }

    pub async fn menus(
        &self,
        franchise: &str,
        venue: u64,
        sales_area: u64,
    ) -> Result<Vec<MenuSummary>, Error> {
        self.get(&paths::menus(franchise, venue, sales_area)).await
    }

    pub async fn menu(
        &self,
        franchise: &str,
        venue: u64,
        sales_area: u64,
        menu: &str,
    ) -> Result<Menu, Error> {
        self.get(&paths::menu(franchise, venue, sales_area, menu))
            .await
    }
}

/// Route paths, relative to the API base, for fetching a body as it comes with [`Client::raw`].
pub mod paths {
    pub fn venues() -> String {
        "venues".to_owned()
    }

    pub fn venue(franchise: &str, venue: u64) -> String {
        format!("{franchise}/venues/{venue}")
    }

    pub fn ales(franchise: &str, venue: u64) -> String {
        format!("{franchise}/venues/{venue}/ales")
    }

    pub fn wait_times(franchise: &str, venue: u64) -> String {
        format!("{franchise}/venues/{venue}/wait-times")
    }

    pub fn menus(franchise: &str, venue: u64, sales_area: u64) -> String {
        format!("{franchise}/venues/{venue}/sales-areas/{sales_area}/menus")
    }

    pub fn menu(franchise: &str, venue: u64, sales_area: u64, menu: &str) -> String {
        format!("{franchise}/venues/{venue}/sales-areas/{sales_area}/menus/{menu}")
    }
}

/// The version the app reports. Its constant has a minified name that changes every build, so it's found by where
/// it's used: `app:{id:"jdw_customer_app",version:Xoe,…}` and then `Xoe="6.9.1"`.
fn app_version(bundle: &str) -> Option<String> {
    let name = Regex::new(r#"app:\{id:"jdw_customer_app",version:([\w$]+)"#)
        .expect("valid pattern")
        .captures(bundle)?[1]
        .to_owned();
    Regex::new(&format!(r#"[,;\s]{}="([^"]+)""#, regex::escape(&name)))
        .expect("valid pattern")
        .captures(bundle)
        .map(|captures| captures[1].to_owned())
}

/// A value from the bundle's baked-in environment, `NAME:"value"`.
fn setting(bundle: &str, name: &str) -> Option<String> {
    Regex::new(&format!(r#"{name}:"([^"]+)""#))
        .expect("valid pattern")
        .captures(bundle)
        .map(|captures| captures[1].to_owned())
}

#[cfg(test)]
mod tests {
    use super::{app_version, setting};

    #[test]
    fn finds_the_app_version_by_where_it_is_used() {
        let bundle = r#"var q=1,Xoe="6.9.1",zHe="60901002";const at={app:{id:"jdw_customer_app",version:Xoe,build:zHe}}"#;
        assert_eq!(app_version(bundle).as_deref(), Some("6.9.1"));
    }

    #[test]
    fn reads_a_setting_from_the_bundle() {
        let bundle = r#"DUe={BASE_URL:"/",VITE_API_PROXY_BASE_URL:"https://ca.jdw-apps.net/api",VITE_API_PROXY_KEY:"1|abc",VITE_API_PROXY_VERSION:"v0.1"}"#;
        assert_eq!(
            setting(bundle, "VITE_API_PROXY_KEY").as_deref(),
            Some("1|abc")
        );
        assert_eq!(
            setting(bundle, "VITE_API_PROXY_VERSION").as_deref(),
            Some("v0.1")
        );
        assert_eq!(setting(bundle, "VITE_NOPE"), None);
    }
}
