use serde::{Deserialize, Deserializer};

/// An ID the API sends as a number to some clients and as a string to others, depending on the app version asked for.
pub fn id<'de, D: Deserializer<'de>>(deserializer: D) -> Result<u64, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Id {
        Number(u64),
        Text(String),
    }
    match Id::deserialize(deserializer)? {
        Id::Number(id) => Ok(id),
        Id::Text(id) => id.parse().map_err(serde::de::Error::custom),
    }
}

/// An ID that isn't always a number, as a string either way.
pub fn key<'de, D: Deserializer<'de>>(deserializer: D) -> Result<String, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Key {
        Number(u64),
        Text(String),
    }
    Ok(match Key::deserialize(deserializer)? {
        Key::Number(id) => id.to_string(),
        Key::Text(id) => id,
    })
}

#[cfg(test)]
mod tests {
    #[derive(serde::Deserialize)]
    struct Thing {
        #[serde(deserialize_with = "super::id")]
        id: u64,
    }

    #[test]
    fn takes_either() {
        assert_eq!(serde_json::from_str::<Thing>(r#"{"id":34597}"#).unwrap().id, 34597);
        assert_eq!(serde_json::from_str::<Thing>(r#"{"id":"34597"}"#).unwrap().id, 34597);
    }
}
