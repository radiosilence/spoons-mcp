//! Confirmation tokens for the mutations that spend money or settle an order: PREVIEW issues one, CONFIRM spends
//! it, as the sibling servers do for sending mail.
//!
//! A token is bound to the receipt address it was issued for and to the exact arguments previewed, lasts fifteen
//! minutes and is spent on first use, right or wrong, so a CONFIRM that doesn't match what the user saw sends the
//! caller back to PREVIEW. Tokens live in memory: a restart only means previewing again.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

const TTL: Duration = Duration::from_secs(15 * 60);
/// Per receipt address. A preview without a confirm is the user changing their mind, not a leak.
const CAP: usize = 64;

struct Token {
    tenant: String,
    fingerprint: String,
    issued_at: Instant,
}

#[derive(Default)]
pub struct TokenStore {
    tokens: Mutex<HashMap<String, Token>>,
}

fn fingerprint(parts: &[&str]) -> String {
    parts
        .iter()
        .map(|part| format!("{}:{part}", part.len()))
        .collect::<Vec<_>>()
        .join("|")
}

impl TokenStore {
    pub fn issue(&self, tenant: &str, parts: &[&str]) -> String {
        let token = uuid::Uuid::new_v4().simple().to_string();
        let mut tokens = self.tokens.lock().unwrap();
        let now = Instant::now();
        tokens.retain(|_, t| now.duration_since(t.issued_at) < TTL);
        while tokens.values().filter(|t| t.tenant == tenant).count() >= CAP {
            let oldest = tokens
                .iter()
                .filter(|(_, t)| t.tenant == tenant)
                .min_by_key(|(_, t)| t.issued_at)
                .map(|(k, _)| k.clone());
            match oldest {
                Some(key) => tokens.remove(&key),
                None => break,
            };
        }
        tokens.insert(
            token.clone(),
            Token {
                tenant: tenant.to_owned(),
                fingerprint: fingerprint(parts),
                issued_at: now,
            },
        );
        token
    }

    pub fn spend(
        &self,
        tenant: &str,
        token: Option<&str>,
        parts: &[&str],
    ) -> Result<(), &'static str> {
        let token = token.ok_or(
            "CONFIRM needs the confirmationToken from a PREVIEW of the same call. Preview first, and confirm only \
             on the user's say-so: their yes, or for `pay`, \"and pay\" given with the order.",
        )?;
        let invalid =
            "That confirmationToken is unknown or already used. PREVIEW again and show the user.";
        let entry = {
            let mut tokens = self.tokens.lock().unwrap();
            // Another user's token is left alone, so learning one is no use.
            if tokens.get(token).is_none_or(|t| t.tenant != tenant) {
                return Err(invalid);
            }
            tokens.remove(token).ok_or(invalid)?
        };
        if Instant::now().duration_since(entry.issued_at) >= TTL {
            return Err("That confirmationToken has expired. PREVIEW again and show the user.");
        }
        if entry.fingerprint != fingerprint(parts) {
            return Err(
                "The arguments changed since the PREVIEW. PREVIEW again and show the user.",
            );
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spends_a_token_once_for_the_same_arguments() {
        let store = TokenStore::default();
        let token = store.issue("a@b", &["pay", "q1", "false"]);
        assert!(
            store
                .spend("a@b", Some(&token), &["pay", "q1", "false"])
                .is_ok()
        );
        assert!(
            store
                .spend("a@b", Some(&token), &["pay", "q1", "false"])
                .is_err()
        );
    }

    #[test]
    fn refuses_changed_arguments_and_burns_the_token() {
        let store = TokenStore::default();
        let token = store.issue("a@b", &["pay", "q1", "false"]);
        assert!(
            store
                .spend("a@b", Some(&token), &["pay", "q1", "true"])
                .is_err()
        );
        assert!(
            store
                .spend("a@b", Some(&token), &["pay", "q1", "false"])
                .is_err()
        );
    }

    #[test]
    fn refuses_another_tenants_token_without_burning_it() {
        let store = TokenStore::default();
        let token = store.issue("a@b", &["pay", "q1"]);
        assert!(store.spend("c@d", Some(&token), &["pay", "q1"]).is_err());
        assert!(store.spend("a@b", Some(&token), &["pay", "q1"]).is_ok());
    }

    #[test]
    fn refuses_a_missing_token() {
        assert!(TokenStore::default().spend("", None, &[]).is_err());
    }

    #[test]
    fn fingerprints_cannot_be_forged_by_moving_separators() {
        assert_ne!(fingerprint(&["a|b", "c"]), fingerprint(&["a", "b|c"]));
    }
}
