use serde::{Deserialize, Deserializer, Serialize, Serializer};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Rational {
    pub num: u32,
    pub den: u32,
}

impl Rational {
    pub const fn new(num: u32, den: u32) -> Self {
        Self { num, den }
    }

    pub fn as_f32(self) -> f32 {
        if self.den == 0 {
            return 0.0;
        }
        self.num as f32 / self.den as f32
    }
}

impl Serialize for Rational {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        [self.num, self.den].serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for Rational {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        use serde::de;

        struct RationalVisitor;

        impl<'de> de::Visitor<'de> for RationalVisitor {
            type Value = Rational;

            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("a [num, den] array or {\"num\": N, \"den\": D} object")
            }

            fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
            where
                A: de::SeqAccess<'de>,
            {
                let num = seq
                    .next_element::<u32>()?
                    .ok_or_else(|| de::Error::invalid_length(0, &self))?;
                let den = seq
                    .next_element::<u32>()?
                    .ok_or_else(|| de::Error::invalid_length(1, &self))?;
                Ok(Rational { num, den })
            }

            fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
            where
                A: de::MapAccess<'de>,
            {
                let mut num = None;
                let mut den = None;
                while let Some(key) = map.next_key::<&str>()? {
                    match key {
                        "num" => num = Some(map.next_value()?),
                        "den" => den = Some(map.next_value()?),
                        _ => {
                            let _ = map.next_value::<de::IgnoredAny>()?;
                        }
                    }
                }
                let num = num.ok_or_else(|| de::Error::missing_field("num"))?;
                let den = den.ok_or_else(|| de::Error::missing_field("den"))?;
                Ok(Rational { num, den })
            }
        }

        deserializer.deserialize_any(RationalVisitor)
    }
}

#[cfg(test)]
mod tests {
    use super::Rational;

    #[test]
    fn as_f32_handles_zero_denominator() {
        assert_eq!(Rational::new(10, 0).as_f32(), 0.0);
        assert!((Rational::new(30000, 1001).as_f32() - 29.97003).abs() < 0.0001);
    }

    #[test]
    fn serde_round_trip() {
        let value = Rational::new(24, 1);
        let json = serde_json::to_string(&value).expect("serialize rational");
        assert_eq!(json, "[24,1]");

        let restored: Rational = serde_json::from_str(json.as_str()).expect("deserialize rational");
        assert_eq!(restored, value);
    }

    #[test]
    fn serde_object_form() {
        let json = r#"{"num": 30000, "den": 1001}"#;
        let value: Rational = serde_json::from_str(json).expect("deserialize object rational");
        assert_eq!(value, Rational::new(30000, 1001));
    }
}
