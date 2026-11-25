use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;
use core::fmt;

use serde::de::{self, Visitor};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

pub mod decimal {
    use super::{
        Deserializer, Serialize, Serializer, String, Visitor, de, fmt,
    };

    pub fn serialize<S, T>(value: &T, out: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
        T: fmt::Display + Serialize,
    {
        if out.is_human_readable() {
            out.collect_str(value)
        } else {
            value.serialize(out)
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(
        input: D,
    ) -> Result<u64, D::Error> {
        if input.is_human_readable() {
            input.deserialize_str(Decimal)
        } else {
            <u64 as serde::Deserialize>::deserialize(input)
        }
    }

    struct Decimal;

    impl Visitor<'_> for Decimal {
        type Value = u64;

        fn expecting(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
            out.write_str("a u64 as decimal digits")
        }

        fn visit_str<E: de::Error>(self, text: &str) -> Result<u64, E> {
            text.bytes()
                .all(|byte| byte.is_ascii_digit())
                .then(|| text.parse().ok())
                .flatten()
                .ok_or_else(|| {
                    E::invalid_value(de::Unexpected::Str(text), &self)
                })
        }

        fn visit_string<E: de::Error>(self, text: String) -> Result<u64, E> {
            self.visit_str(&text)
        }
    }
}

pub mod pairs {
    use super::{BTreeMap, Deserialize, Deserializer, Serializer, Vec, de};

    pub fn serialize<S, K, V>(
        map: &BTreeMap<K, V>,
        out: S,
    ) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
        K: serde::Serialize,
        V: serde::Serialize,
    {
        out.collect_seq(map)
    }

    pub fn deserialize<'de, D, K, V>(
        input: D,
    ) -> Result<BTreeMap<K, V>, D::Error>
    where
        D: Deserializer<'de>,
        K: Deserialize<'de> + Ord,
        V: Deserialize<'de>,
    {
        let pairs = Vec::<(K, V)>::deserialize(input)?;
        let len = pairs.len();
        let map = pairs.into_iter().collect::<BTreeMap<K, V>>();
        if map.len() == len {
            Ok(map)
        } else {
            Err(de::Error::custom("a key is given twice"))
        }
    }
}
