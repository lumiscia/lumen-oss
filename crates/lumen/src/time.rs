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

#[cfg(test)]
mod tests {
    use super::Rational;

    #[test]
    fn as_f32_handles_zero_denominator() {
        assert_eq!(Rational::new(10, 0).as_f32(), 0.0);
        assert!((Rational::new(30000, 1001).as_f32() - 29.97003).abs() < 0.0001);
    }
}
