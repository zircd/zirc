//! Reading what a newer sender says, leniently: a value this reader doesn't know (a kind,
//! a level or a tool added since it was built) is left out, rather than failing the whole
//! thing it's in. A field it doesn't know is ignored already, as serde does by default; these
//! are for the values: `#[serde(with = "…")]`, each writing its value as it writes itself,
//! where ts-rs reads the type (`#[ts(as = "…")]`, as it warns of `deserialize_with`), and
//! `#[serde(deserialize_with = "….deserialize")]` elsewhere.
//!
//! So an older `zirc` still lists roles when the server's roles take a new kind of report,
//! and an older zircd still reads a wake that names a new state; what's left out is what the
//! reader couldn't have acted on anyway.

use serde::{Serialize, Serializer};

/// The value as it writes itself.
fn as_is<T: Serialize, S: Serializer>(value: &T, to: S) -> Result<S::Ok, S::Error> {
    value.serialize(to)
}

/// One value, none when this reader doesn't know it.
pub mod known {
    use serde::de::DeserializeOwned;
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    use serde_json::Value;

    /// As it writes itself.
    ///
    /// # Errors
    ///
    /// When the serializer fails.
    pub fn serialize<T: Serialize, S: Serializer>(
        value: &Option<T>,
        to: S,
    ) -> Result<S::Ok, S::Error> {
        super::as_is(value, to)
    }

    /// The value said, none when it's one this reader doesn't know.
    ///
    /// # Errors
    ///
    /// Only when what's said isn't a value at all.
    pub fn deserialize<'de, D, T>(from: D) -> Result<Option<T>, D::Error>
    where
        D: Deserializer<'de>,
        T: DeserializeOwned,
    {
        let said = Value::deserialize(from)?;
        Ok(serde_json::from_value(said).ok())
    }
}

/// A list, without the values this reader doesn't know.
pub mod known_ones {
    use serde::de::DeserializeOwned;
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    use serde_json::Value;

    /// As it writes itself.
    ///
    /// # Errors
    ///
    /// When the serializer fails.
    pub fn serialize<T: Serialize, S: Serializer>(value: &[T], to: S) -> Result<S::Ok, S::Error> {
        value.serialize(to)
    }

    /// The values said that this reader knows.
    ///
    /// # Errors
    ///
    /// Only when what's said isn't a list.
    pub fn deserialize<'de, D, T>(from: D) -> Result<Vec<T>, D::Error>
    where
        D: Deserializer<'de>,
        T: DeserializeOwned,
    {
        let said = Vec::<Value>::deserialize(from)?;
        Ok(said
            .into_iter()
            .filter_map(|value| serde_json::from_value(value).ok())
            .collect())
    }
}

/// A map, without the entries whose key or value this reader doesn't know.
pub mod known_keys {
    use std::collections::BTreeMap;

    use serde::de::DeserializeOwned;
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    use serde_json::Value;

    /// As it writes itself.
    ///
    /// # Errors
    ///
    /// When the serializer fails.
    pub fn serialize<K: Serialize, V: Serialize, S: Serializer>(
        value: &BTreeMap<K, V>,
        to: S,
    ) -> Result<S::Ok, S::Error> {
        super::as_is(value, to)
    }

    /// The entries said that this reader knows.
    ///
    /// # Errors
    ///
    /// Only when what's said isn't a map.
    pub fn deserialize<'de, D, K, V>(from: D) -> Result<BTreeMap<K, V>, D::Error>
    where
        D: Deserializer<'de>,
        K: DeserializeOwned + Ord,
        V: DeserializeOwned,
    {
        let said = serde_json::Map::<String, Value>::deserialize(from)?;
        Ok(said
            .into_iter()
            .filter_map(|(key, value)| {
                Some((
                    serde_json::from_value(Value::String(key)).ok()?,
                    serde_json::from_value(value).ok()?,
                ))
            })
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use serde::Deserialize;

    use crate::state::Level;

    #[derive(Debug, Deserialize)]
    struct Said {
        #[serde(default, with = "super::known")]
        level: Option<Level>,
        #[serde(default, with = "super::known_ones")]
        levels: Vec<Level>,
        #[serde(default, with = "super::known_keys")]
        by: std::collections::BTreeMap<Level, u8>,
    }

    #[test]
    fn leaves_out_what_it_doesnt_know() {
        let said: Said = serde_json::from_value(serde_json::json!({
            "level": "on-fire",
            "levels": ["ok", "on-fire", "broken"],
            "by": {"warning": 1, "on-fire": 2, "ok": "three"},
        }))
        .unwrap();
        assert_eq!(said.level, None);
        assert_eq!(said.levels, [Level::Ok, Level::Broken]);
        assert_eq!(
            said.by.into_iter().collect::<Vec<_>>(),
            [(Level::Warning, 1)]
        );
        let none: Said = serde_json::from_value(serde_json::json!({})).unwrap();
        assert!(none.level.is_none() && none.levels.is_empty() && none.by.is_empty());
        assert!(serde_json::from_value::<Said>(serde_json::json!({"levels": "ok"})).is_err());
    }
}
