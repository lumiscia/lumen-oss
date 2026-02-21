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
        let raw = <[u32; 2]>::deserialize(deserializer)?;
        Ok(Self {
            num: raw[0],
            den: raw[1],
        })
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
}
