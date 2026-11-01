//! The payment API, which takes orders and their payments. It is a separate service from the proxy: it takes no
//! bearer token, and the app adds its platform and a device id to every body instead.
//!
//! Preparing a card payment returns a page on BNP Paribas Axepta where the card is entered; the order goes to the
//! pub once that page has taken the payment. Nothing here is retried that could place an order twice: a request to
//! prepare that gets no answer may still have succeeded, and is reported as unconfirmed rather than sent again.

use std::time::Duration;

use reqwest::header::{HeaderMap, HeaderValue, ORIGIN, REFERER};
use reqwest::{Method, StatusCode, redirect};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::time::sleep;

use crate::client::{APP, Error, USER_AGENT};

pub(crate) const DEFAULT_BASE: &str = "https://api.jdwpayments.net/api/0.1";

#[derive(Clone)]
pub struct Payments {
    http: reqwest::Client,
    base: String,
    device_id: String,
}

/// What to charge for, from a validated basket.
#[derive(Debug, Clone)]
pub struct CardOrder<'a> {
    pub table: u32,
    pub currency: &'a str,
    pub amount: f64,
    /// Where the receipt goes.
    pub email: &'a str,
    /// From [`crate::BasketValidation::token`].
    pub token: &'a str,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Prepared {
    pub transaction_id: String,
    /// The Axepta page that takes the card. It cancels the order if the customer backs out.
    pub url: String,
    /// The same basket was ordered to the same table recently; paying again would order it twice.
    #[serde(default)]
    pub duplicate_order: bool,
}

/// Where an order's payment has got to.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "status", rename_all = "camelCase")]
pub enum PaymentStatus {
    /// Paid for and sent to the pub.
    Placed {
        order_ref: String,
        paid_by: Option<String>,
        placed_at: Option<String>,
    },
    /// No card has been entered yet. The only state in which cancelling is safe.
    Pending,
    /// The card is being authorised, 3-D Secure included.
    InProgress,
    /// Paid for, and not yet sent to the pub.
    Paid,
    /// Paid for and held back as a likely repeat of a recent order. The app asks whether to send it anyway.
    Duplicate,
    Cancelled,
    Other {
        order_status: Option<String>,
        payment_status: Option<String>,
        message: Option<String>,
    },
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct StatusData {
    unique_ref: Option<Value>,
    paid_by: Option<String>,
    placed_at: Option<String>,
    order_status: Option<String>,
    payment_status: Option<String>,
    message: Option<String>,
}

#[derive(Deserialize)]
struct Answer<T> {
    #[serde(default)]
    success: bool,
    data: Option<T>,
    message: Option<String>,
}

impl Payments {
    pub(crate) fn new(base: &str, app_version: &str, device_id: &str) -> Result<Self, Error> {
        let mut headers = HeaderMap::new();
        headers.insert(ORIGIN, HeaderValue::from_static(APP));
        headers.insert(
            REFERER,
            HeaderValue::from_static("https://order.jdwetherspoon.com/"),
        );
        headers.insert(
            "app-version",
            HeaderValue::from_str(app_version).map_err(|_| Error::NoKey)?,
        );
        let http = reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .default_headers(headers)
            .redirect(redirect::Policy::none())
            .timeout(Duration::from_secs(30))
            .build()?;
        Ok(Self {
            http,
            base: base.trim_end_matches('/').to_owned(),
            device_id: device_id.to_owned(),
        })
    }

    fn url(&self, franchise: &str, venue: u64, route: &str) -> String {
        format!("{}/{franchise}/venues/{venue}/{route}", self.base)
    }

    fn body(&self, mut body: Value) -> Value {
        body["platform"] = json!("web");
        body["user_device_identifier"] = json!(self.device_id);
        body
    }

    /// Starts a card payment. Sent once: on [`Error::Unconfirmed`] the order may exist, and sending it again would
    /// make a second one.
    pub async fn prepare_card(
        &self,
        franchise: &str,
        venue: u64,
        order: &CardOrder<'_>,
    ) -> Result<Prepared, Error> {
        let route = "orders/prepare/card";
        let callback = format!(
            "{APP}/venue/{franchise}/{venue}/tables/{}/checkout/callback",
            order.table
        );
        let body = self.body(json!({
            "venue_ref": venue,
            "table": order.table,
            "currency": order.currency,
            "amount": order.amount,
            "description": "",
            "user_email": order.email,
            "token": order.token,
            "callback_url": callback,
        }));
        let response = self
            .http
            .post(self.url(franchise, venue, route))
            .json(&body)
            .send()
            .await
            .map_err(|source| Error::Unconfirmed {
                path: route.to_owned(),
                source,
            })?;
        let status = response.status();
        let bytes = response
            .bytes()
            .await
            .map_err(|source| Error::Unconfirmed {
                path: route.to_owned(),
                source,
            })?;
        let answer: Answer<Prepared> =
            serde_json::from_slice(&bytes).map_err(|_| Error::Status {
                status: status.as_u16(),
                path: route.to_owned(),
                body: String::from_utf8_lossy(&bytes).into_owned(),
            })?;
        match answer {
            Answer {
                success: true,
                data: Some(prepared),
                ..
            } if status.is_success() => Ok(prepared),
            Answer { message, .. } => Err(Error::Refused {
                path: route.to_owned(),
                message: message.unwrap_or_else(|| String::from_utf8_lossy(&bytes).into_owned()),
            }),
        }
    }

    pub async fn status(
        &self,
        franchise: &str,
        venue: u64,
        transaction: &str,
    ) -> Result<PaymentStatus, Error> {
        let route = format!("payment/{transaction}/status");
        let response = self
            .http
            .get(self.url(franchise, venue, &route))
            .send()
            .await?;
        let status = response.status();
        let bytes = response.bytes().await?;
        status_from(status, &bytes).ok_or_else(|| Error::Status {
            status: status.as_u16(),
            path: route,
            body: String::from_utf8_lossy(&bytes).into_owned(),
        })
    }

    /// Cancels a payment, as the Axepta page's back link does. Only safe while the status is
    /// [`PaymentStatus::Pending`]: once a card is being authorised, cancelling can leave a charge with no order.
    pub async fn cancel(
        &self,
        franchise: &str,
        venue: u64,
        transaction: &str,
    ) -> Result<(), Error> {
        let route = format!("payment/{transaction}/cancel");
        let mut wait = Duration::from_secs(1);
        for attempt in 0.. {
            let sent = self
                .http
                .request(Method::POST, self.url(franchise, venue, &route))
                .json(&self.body(json!({})))
                .send()
                .await;
            let retry = match sent {
                Ok(response) if response.status().is_success() => return Ok(()),
                Ok(response) if !response.status().is_server_error() || attempt == 2 => {
                    return Err(Error::Status {
                        status: response.status().as_u16(),
                        path: route,
                        body: response.text().await.unwrap_or_default(),
                    });
                }
                Err(error) if attempt == 2 => return Err(error.into()),
                Ok(response) => response.status().to_string(),
                Err(error) => error.to_string(),
            };
            tracing::warn!(%retry, route, "retrying in {wait:?}");
            sleep(wait).await;
            wait *= 2;
        }
        unreachable!()
    }
}

/// The status route answers 200 once the order is placed, and 400 with the order's state in the body until then.
fn status_from(status: StatusCode, body: &[u8]) -> Option<PaymentStatus> {
    let answer: Answer<StatusData> = serde_json::from_slice(body).ok()?;
    let data = answer.data?;
    if status.is_success() && answer.success {
        let order_ref = match data.unique_ref? {
            Value::String(text) => text,
            other => other.to_string(),
        };
        return Some(PaymentStatus::Placed {
            order_ref,
            paid_by: data.paid_by,
            placed_at: data.placed_at,
        });
    }
    if status != StatusCode::BAD_REQUEST {
        return None;
    }
    Some(
        match (data.order_status.as_deref(), data.payment_status.as_deref()) {
            (Some("cancelled"), _) => PaymentStatus::Cancelled,
            (Some("duplicate"), _) => PaymentStatus::Duplicate,
            (Some("paid"), _) => PaymentStatus::Paid,
            (Some("pending"), Some("inProgress")) => PaymentStatus::InProgress,
            (Some("pending"), Some("pending")) => PaymentStatus::Pending,
            _ => PaymentStatus::Other {
                order_status: data.order_status,
                payment_status: data.payment_status,
                message: data.message,
            },
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pending(order: &str, payment: &str) -> Vec<u8> {
        json!({ "success": false, "data": {
            "message": "There was a payment error",
            "paymentSuccess": false,
            "orderSuccess": false,
            "transactionId": "025420261101100256ACC62AC162F2",
            "paymentStatus": payment,
            "orderStatus": order,
        } })
        .to_string()
        .into_bytes()
    }

    #[test]
    fn reads_a_placed_order() {
        let body = json!({ "success": true, "data": {
            "uniqueRef": 4821, "paidBy": "card", "placedAt": "2026-11-01T10:05:00Z",
        } })
        .to_string();
        assert_eq!(
            status_from(StatusCode::OK, body.as_bytes()),
            Some(PaymentStatus::Placed {
                order_ref: "4821".into(),
                paid_by: Some("card".into()),
                placed_at: Some("2026-11-01T10:05:00Z".into()),
            })
        );
    }

    #[test]
    fn reads_each_state_before_the_order_is_placed() {
        let cases = [
            ("cancelled", "pending", PaymentStatus::Cancelled),
            ("duplicate", "paid", PaymentStatus::Duplicate),
            ("paid", "paid", PaymentStatus::Paid),
            ("pending", "inProgress", PaymentStatus::InProgress),
            ("pending", "pending", PaymentStatus::Pending),
        ];
        for (order, payment, expected) in cases {
            assert_eq!(
                status_from(StatusCode::BAD_REQUEST, &pending(order, payment)),
                Some(expected),
                "{order}/{payment}"
            );
        }
    }

    #[test]
    fn keeps_a_state_it_does_not_know() {
        assert_eq!(
            status_from(StatusCode::BAD_REQUEST, &pending("refunded", "refunded")),
            Some(PaymentStatus::Other {
                order_status: Some("refunded".into()),
                payment_status: Some("refunded".into()),
                message: Some("There was a payment error".into()),
            })
        );
    }

    #[test]
    fn treats_other_failures_as_errors() {
        assert_eq!(
            status_from(
                StatusCode::INTERNAL_SERVER_ERROR,
                &pending("pending", "pending")
            ),
            None
        );
        assert_eq!(status_from(StatusCode::BAD_REQUEST, b"<html>"), None);
    }
}
