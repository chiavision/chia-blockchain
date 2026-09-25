//! Containers for secret material that wipe themselves on drop and never
//! print their contents through `Debug`.

use std::fmt;

use serde::Deserialize;
use zeroize::{Zeroize, ZeroizeOnDrop, Zeroizing};

/// A string that is zeroed when dropped and redacted in logs.
#[derive(Clone, Default, Zeroize, ZeroizeOnDrop, Deserialize)]
#[serde(transparent)]
pub struct SecretString(String);

impl SecretString {
    pub fn new(s: String) -> Self {
        Self(s)
    }

    /// Borrow the secret. Keep the borrow short; don't clone it into
    /// long-lived non-zeroizing storage.
    pub fn expose(&self) -> &str {
        &self.0
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl fmt::Debug for SecretString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SecretString(<redacted>)")
    }
}

/// A mnemonic phrase: a list of words, wiped on drop.
#[derive(Clone, Default, Zeroize, ZeroizeOnDrop, Deserialize)]
#[serde(transparent)]
pub struct Mnemonic(Vec<String>);

impl Mnemonic {
    pub fn from_words(words: Vec<String>) -> Self {
        Self(words)
    }

    /// Split a pasted phrase on any whitespace, lower-casing each word.
    pub fn parse(phrase: &str) -> Self {
        Self(phrase.split_whitespace().map(|w| w.to_ascii_lowercase()).collect())
    }

    pub fn words(&self) -> &[String] {
        &self.0
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Serialise to a JSON array directly into zeroizing memory, so the phrase
    /// never passes through a non-wiping `serde_json::Value`.
    pub fn to_json_array(&self) -> Zeroizing<String> {
        let mut out = Zeroizing::new(String::with_capacity(self.0.len() * 10));
        out.push('[');
        for (i, w) in self.0.iter().enumerate() {
            if i > 0 {
                out.push(',');
            }
            out.push('"');
            // BIP-39 words are plain ASCII letters; refuse anything else
            // rather than implement JSON escaping for secrets.
            for ch in w.chars().filter(|c| c.is_ascii_alphabetic()) {
                out.push(ch);
            }
            out.push('"');
        }
        out.push(']');
        out
    }
}

impl fmt::Debug for Mnemonic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Mnemonic(<{} words redacted>)", self.0.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_is_redacted() {
        let s = SecretString::new("hunter2".into());
        assert!(!format!("{s:?}").contains("hunter2"));
        let m = Mnemonic::parse("Abandon  ABOUT\nzoo");
        assert_eq!(m.words(), ["abandon", "about", "zoo"]);
        assert!(!format!("{m:?}").contains("abandon"));
    }

    #[test]
    fn json_array_is_minimal_and_safe() {
        let m = Mnemonic::from_words(vec!["abandon".into(), "zo\"o".into()]);
        assert_eq!(m.to_json_array().as_str(), r#"["abandon","zoo"]"#);
    }
}
