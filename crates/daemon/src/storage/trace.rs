use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

use serde::Deserialize;
use serde::de::value::{Error, U32Deserializer};
use serde::de::{
    self, DeserializeSeed, EnumAccess, Expected, IntoDeserializer, MapAccess,
    SeqAccess, VariantAccess, Visitor,
};

const DEPTH: usize = 64;
const PASSES: usize = 100_000;

type Variants = &'static [&'static str];
type EnumKey = (&'static str, Variants);

#[derive(Default)]
struct Explored {
    visited: BTreeSet<usize>,
    below: BTreeMap<usize, BTreeSet<EnumKey>>,
}

#[derive(Default)]
pub struct Trace {
    enums: BTreeMap<EnumKey, Explored>,
    samples: BTreeMap<String, &'static str>,
}

impl Trace {
    pub fn with_text(mut self, expecting: &str, sample: &'static str) -> Self {
        let _ = self.samples.insert(expecting.to_owned(), sample);
        self
    }

    pub fn of<T: Deserialize<'static>>(mut self) -> Self {
        for _ in 0..PASSES {
            let mut pass = Pass {
                trace: &mut self,
                path: Vec::new(),
            };
            let _ = T::deserialize(&mut pass);
            if self.complete() {
                break;
            }
        }
        self
    }

    pub fn complete(&self) -> bool {
        self.enums
            .keys()
            .all(|key| self.explored(*key, &mut BTreeSet::new()))
    }

    pub fn render(&self) -> String {
        let mut out = String::new();
        for (name, variants) in self.enums.keys() {
            let _ = writeln!(out, "{name}: {}", variants.join(" "));
        }
        out
    }

    fn explored(&self, key: EnumKey, seen: &mut BTreeSet<EnumKey>) -> bool {
        if !seen.insert(key) {
            return true;
        }
        self.enums.get(&key).is_some_and(|entry| {
            entry.visited.len() == key.1.len()
                && entry
                    .below
                    .values()
                    .flatten()
                    .all(|inner| self.explored(*inner, seen))
        })
    }

    fn choose(&self, key: EnumKey) -> usize {
        let found = self.enums.get(&key);
        let visited = |index: &usize| {
            found.is_some_and(|entry| entry.visited.contains(index))
        };
        (0..key.1.len())
            .find(|index| !visited(index))
            .or_else(|| {
                found.and_then(|entry| {
                    entry.below.iter().find_map(|(index, inner)| {
                        inner
                            .iter()
                            .any(|key| {
                                !self.explored(*key, &mut BTreeSet::new())
                            })
                            .then_some(*index)
                    })
                })
            })
            .unwrap_or(0)
    }
}

struct Pass<'trace> {
    trace: &'trace mut Trace,
    path: Vec<(EnumKey, usize)>,
}

impl Pass<'_> {
    fn deeper(&self) -> Result<(), Error> {
        if self.path.len() >= DEPTH {
            Err(de::Error::custom("too deep"))
        } else {
            Ok(())
        }
    }

    fn text(&self, visitor: &dyn Expected) -> &'static str {
        self.trace
            .samples
            .get(&visitor.to_string())
            .copied()
            .unwrap_or("")
    }
}

struct Items<'pass, 'trace> {
    pass: &'pass mut Pass<'trace>,
    left: usize,
}

impl<'de> SeqAccess<'de> for Items<'_, '_>
where
    'de: 'static,
{
    type Error = Error;

    fn next_element_seed<T: DeserializeSeed<'de>>(
        &mut self,
        seed: T,
    ) -> Result<Option<T::Value>, Error> {
        if self.left == 0 {
            return Ok(None);
        }
        self.left -= 1;
        seed.deserialize(&mut *self.pass).map(Some)
    }
}

impl<'de> MapAccess<'de> for Items<'_, '_>
where
    'de: 'static,
{
    type Error = Error;

    fn next_key_seed<K: DeserializeSeed<'de>>(
        &mut self,
        seed: K,
    ) -> Result<Option<K::Value>, Error> {
        if self.left == 0 {
            return Ok(None);
        }
        self.left -= 1;
        seed.deserialize(&mut *self.pass).map(Some)
    }

    fn next_value_seed<V: DeserializeSeed<'de>>(
        &mut self,
        seed: V,
    ) -> Result<V::Value, Error> {
        seed.deserialize(&mut *self.pass)
    }
}

struct Variant<'pass, 'trace> {
    pass: &'pass mut Pass<'trace>,
    index: usize,
}

impl<'de> EnumAccess<'de> for Variant<'_, '_>
where
    'de: 'static,
{
    type Error = Error;
    type Variant = Self;

    fn variant_seed<V: DeserializeSeed<'de>>(
        self,
        seed: V,
    ) -> Result<(V::Value, Self), Error> {
        let index = u32::try_from(self.index)
            .map_err(|_| <Error as de::Error>::custom("too many variants"))?;
        let deserializer: U32Deserializer<Error> = index.into_deserializer();
        Ok((seed.deserialize(deserializer)?, self))
    }
}

impl<'de> VariantAccess<'de> for Variant<'_, '_>
where
    'de: 'static,
{
    type Error = Error;

    fn unit_variant(self) -> Result<(), Error> {
        Ok(())
    }

    fn newtype_variant_seed<T: DeserializeSeed<'de>>(
        self,
        seed: T,
    ) -> Result<T::Value, Error> {
        seed.deserialize(&mut *self.pass)
    }

    fn tuple_variant<V: Visitor<'de>>(
        self,
        len: usize,
        visitor: V,
    ) -> Result<V::Value, Error> {
        visitor.visit_seq(Items {
            pass: self.pass,
            left: len,
        })
    }

    fn struct_variant<V: Visitor<'de>>(
        self,
        fields: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, Error> {
        visitor.visit_seq(Items {
            pass: self.pass,
            left: fields.len(),
        })
    }
}

impl<'de> de::Deserializer<'de> for &mut Pass<'_>
where
    'de: 'static,
{
    type Error = Error;

    fn deserialize_any<V: Visitor<'de>>(self, _: V) -> Result<V::Value, Error> {
        Err(de::Error::custom("postcard reads no self-described value"))
    }

    fn deserialize_bool<V: Visitor<'de>>(
        self,
        visitor: V,
    ) -> Result<V::Value, Error> {
        visitor.visit_bool(false)
    }

    fn deserialize_i8<V: Visitor<'de>>(
        self,
        visitor: V,
    ) -> Result<V::Value, Error> {
        visitor.visit_i8(1)
    }

    fn deserialize_i16<V: Visitor<'de>>(
        self,
        visitor: V,
    ) -> Result<V::Value, Error> {
        visitor.visit_i16(1)
    }

    fn deserialize_i32<V: Visitor<'de>>(
        self,
        visitor: V,
    ) -> Result<V::Value, Error> {
        visitor.visit_i32(1)
    }

    fn deserialize_i64<V: Visitor<'de>>(
        self,
        visitor: V,
    ) -> Result<V::Value, Error> {
        visitor.visit_i64(1)
    }

    fn deserialize_i128<V: Visitor<'de>>(
        self,
        visitor: V,
    ) -> Result<V::Value, Error> {
        visitor.visit_i128(1)
    }

    fn deserialize_u8<V: Visitor<'de>>(
        self,
        visitor: V,
    ) -> Result<V::Value, Error> {
        visitor.visit_u8(1)
    }

    fn deserialize_u16<V: Visitor<'de>>(
        self,
        visitor: V,
    ) -> Result<V::Value, Error> {
        visitor.visit_u16(1)
    }

    fn deserialize_u32<V: Visitor<'de>>(
        self,
        visitor: V,
    ) -> Result<V::Value, Error> {
        visitor.visit_u32(1)
    }

    fn deserialize_u64<V: Visitor<'de>>(
        self,
        visitor: V,
    ) -> Result<V::Value, Error> {
        visitor.visit_u64(1)
    }

    fn deserialize_u128<V: Visitor<'de>>(
        self,
        visitor: V,
    ) -> Result<V::Value, Error> {
        visitor.visit_u128(1)
    }

    fn deserialize_f32<V: Visitor<'de>>(
        self,
        visitor: V,
    ) -> Result<V::Value, Error> {
        visitor.visit_f32(1.0)
    }

    fn deserialize_f64<V: Visitor<'de>>(
        self,
        visitor: V,
    ) -> Result<V::Value, Error> {
        visitor.visit_f64(1.0)
    }

    fn deserialize_char<V: Visitor<'de>>(
        self,
        visitor: V,
    ) -> Result<V::Value, Error> {
        visitor.visit_char('a')
    }

    fn deserialize_str<V: Visitor<'de>>(
        self,
        visitor: V,
    ) -> Result<V::Value, Error> {
        let text = self.text(&visitor);
        visitor.visit_borrowed_str(text)
    }

    fn deserialize_string<V: Visitor<'de>>(
        self,
        visitor: V,
    ) -> Result<V::Value, Error> {
        self.deserialize_str(visitor)
    }

    fn deserialize_bytes<V: Visitor<'de>>(
        self,
        visitor: V,
    ) -> Result<V::Value, Error> {
        visitor.visit_borrowed_bytes(&[])
    }

    fn deserialize_byte_buf<V: Visitor<'de>>(
        self,
        visitor: V,
    ) -> Result<V::Value, Error> {
        self.deserialize_bytes(visitor)
    }

    fn deserialize_option<V: Visitor<'de>>(
        self,
        visitor: V,
    ) -> Result<V::Value, Error> {
        self.deeper()?;
        visitor.visit_some(self)
    }

    fn deserialize_unit<V: Visitor<'de>>(
        self,
        visitor: V,
    ) -> Result<V::Value, Error> {
        visitor.visit_unit()
    }

    fn deserialize_unit_struct<V: Visitor<'de>>(
        self,
        _: &'static str,
        visitor: V,
    ) -> Result<V::Value, Error> {
        visitor.visit_unit()
    }

    fn deserialize_newtype_struct<V: Visitor<'de>>(
        self,
        _: &'static str,
        visitor: V,
    ) -> Result<V::Value, Error> {
        self.deeper()?;
        visitor.visit_newtype_struct(self)
    }

    fn deserialize_seq<V: Visitor<'de>>(
        self,
        visitor: V,
    ) -> Result<V::Value, Error> {
        self.deeper()?;
        visitor.visit_seq(Items {
            pass: self,
            left: 1,
        })
    }

    fn deserialize_tuple<V: Visitor<'de>>(
        self,
        len: usize,
        visitor: V,
    ) -> Result<V::Value, Error> {
        self.deeper()?;
        visitor.visit_seq(Items {
            pass: self,
            left: len,
        })
    }

    fn deserialize_tuple_struct<V: Visitor<'de>>(
        self,
        _: &'static str,
        len: usize,
        visitor: V,
    ) -> Result<V::Value, Error> {
        self.deserialize_tuple(len, visitor)
    }

    fn deserialize_map<V: Visitor<'de>>(
        self,
        visitor: V,
    ) -> Result<V::Value, Error> {
        self.deeper()?;
        visitor.visit_map(Items {
            pass: self,
            left: 1,
        })
    }

    fn deserialize_struct<V: Visitor<'de>>(
        self,
        _: &'static str,
        fields: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, Error> {
        self.deserialize_tuple(fields.len(), visitor)
    }

    fn deserialize_enum<V: Visitor<'de>>(
        self,
        name: &'static str,
        variants: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, Error> {
        self.deeper()?;
        let key = (name, variants);
        if self.path.iter().any(|(on, _)| *on == key) {
            return Err(de::Error::custom("recursive"));
        }
        if let Some((parent, index)) = self.path.last() {
            let _ = self
                .trace
                .enums
                .entry(*parent)
                .or_default()
                .below
                .entry(*index)
                .or_default()
                .insert(key);
        }
        let index = self.trace.choose(key);
        let _ = self
            .trace
            .enums
            .entry(key)
            .or_default()
            .visited
            .insert(index);
        self.path.push((key, index));
        let value = visitor.visit_enum(Variant {
            pass: &mut *self,
            index,
        });
        let _ = self.path.pop();
        value
    }

    fn deserialize_identifier<V: Visitor<'de>>(
        self,
        _: V,
    ) -> Result<V::Value, Error> {
        Err(de::Error::custom("postcard reads no identifier"))
    }

    fn deserialize_ignored_any<V: Visitor<'de>>(
        self,
        _: V,
    ) -> Result<V::Value, Error> {
        Err(de::Error::custom("postcard skips no value"))
    }

    fn is_human_readable(&self) -> bool {
        false
    }
}
