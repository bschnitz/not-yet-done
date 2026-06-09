use ratatui::style::Color;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt;
use std::str::FromStr;

/// A 24-bit colour stored as a `#rrggbb` hex string in YAML.
///
/// ```yaml
/// bg: "#121212"
/// primary: "#4db6ac"
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HexColor(pub u8, pub u8, pub u8);

impl HexColor {
    pub fn to_ratatui(&self) -> Color {
        Color::Rgb(self.0, self.1, self.2)
    }
}

impl FromStr for HexColor {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let s = s.trim().trim_start_matches('#');
        if s.len() != 6 {
            return Err(format!("expected 6 hex digits, got {:?}", s));
        }
        let r = u8::from_str_radix(&s[0..2], 16)
            .map_err(|e| format!("invalid red component: {}", e))?;
        let g = u8::from_str_radix(&s[2..4], 16)
            .map_err(|e| format!("invalid green component: {}", e))?;
        let b = u8::from_str_radix(&s[4..6], 16)
            .map_err(|e| format!("invalid blue component: {}", e))?;
        Ok(HexColor(r, g, b))
    }
}

impl fmt::Display for HexColor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "#{:02x}{:02x}{:02x}", self.0, self.1, self.2)
    }
}

impl Serialize for HexColor {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for HexColor {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(d)?;
        HexColor::from_str(&raw).map_err(serde::de::Error::custom)
    }
}
