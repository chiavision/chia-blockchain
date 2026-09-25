//! Local BIP-39 validation so a mistyped recovery phrase is caught before it
//! ever leaves the process. Uses the same English list as `chia/util/english.txt`.

use std::sync::OnceLock;

use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

use crate::secret::Mnemonic;

const WORDLIST_RAW: &str = include_str!("bip39_english.txt");
pub const VALID_LENGTHS: [usize; 5] = [12, 15, 18, 21, 24];

fn wordlist() -> &'static [&'static str] {
    static LIST: OnceLock<Vec<&'static str>> = OnceLock::new();
    LIST.get_or_init(|| {
        let v: Vec<&str> = WORDLIST_RAW.lines().map(str::trim).filter(|w| !w.is_empty()).collect();
        assert_eq!(v.len(), 2048, "BIP-39 wordlist must have 2048 words");
        v
    })
}

pub fn word_index(word: &str) -> Option<u16> {
    wordlist().binary_search(&word).ok().map(|i| i as u16)
}

pub fn is_word(word: &str) -> bool {
    word_index(word).is_some()
}

/// Up to `limit` words beginning with `prefix`.
pub fn suggestions(prefix: &str, limit: usize) -> Vec<&'static str> {
    if prefix.is_empty() {
        return Vec::new();
    }
    let list = wordlist();
    let start = list.partition_point(|w| *w < prefix);
    list[start..]
        .iter()
        .take_while(|w| w.starts_with(prefix))
        .take(limit)
        .copied()
        .collect()
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum MnemonicError {
    #[error("a recovery phrase has 12, 15, 18, 21 or 24 words (found {0})")]
    WrongLength(usize),
    #[error("word {position} (\"{word}\") is not in the BIP-39 list")]
    UnknownWord { position: usize, word: String },
    #[error("checksum mismatch — a word is wrong or out of order")]
    BadChecksum,
}

/// Encode 16–32 bytes of entropy (multiple of 4) as a mnemonic.
pub fn from_entropy(entropy: &[u8]) -> Mnemonic {
    assert!(entropy.len().is_multiple_of(4) && (16..=32).contains(&entropy.len()));
    let checksum_bits = entropy.len() * 8 / 32;
    let hash = Sha256::digest(entropy);
    let bit = |i: usize| -> u16 {
        let byte = if i < entropy.len() * 8 {
            entropy[i / 8]
        } else {
            hash[(i - entropy.len() * 8) / 8]
        };
        u16::from((byte >> (7 - (i % 8))) & 1)
    };
    let total = entropy.len() * 8 + checksum_bits;
    let list = wordlist();
    let words = (0..total / 11)
        .map(|w| {
            let idx = (0..11).fold(0u16, |acc, b| (acc << 1) | bit(w * 11 + b));
            list[idx as usize].to_owned()
        })
        .collect();
    Mnemonic::from_words(words)
}

/// Validate word membership, length and the BIP-39 checksum.
pub fn validate(mnemonic: &Mnemonic) -> Result<(), MnemonicError> {
    let words = mnemonic.words();
    for (i, w) in words.iter().enumerate() {
        if !is_word(w) {
            return Err(MnemonicError::UnknownWord {
                position: i + 1,
                word: w.clone(),
            });
        }
    }
    if !VALID_LENGTHS.contains(&words.len()) {
        return Err(MnemonicError::WrongLength(words.len()));
    }

    // Concatenate the 11-bit indices into a bit string.
    let total_bits = words.len() * 11;
    let checksum_bits = total_bits / 33;
    let entropy_bits = total_bits - checksum_bits;
    let mut bits = Zeroizing::new(Vec::with_capacity(total_bits));
    for w in words {
        let idx = word_index(w).expect("checked above");
        for b in (0..11).rev() {
            bits.push((idx >> b) & 1 == 1);
        }
    }
    let mut entropy = Zeroizing::new(vec![0u8; entropy_bits / 8]);
    for (i, bit) in bits[..entropy_bits].iter().enumerate() {
        if *bit {
            entropy[i / 8] |= 0x80 >> (i % 8);
        }
    }
    let hash = Sha256::digest(entropy.as_slice());
    for i in 0..checksum_bits {
        let expected = (hash[i / 8] >> (7 - (i % 8))) & 1 == 1;
        if bits[entropy_bits + i] != expected {
            return Err(MnemonicError::BadChecksum);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wordlist_is_sorted_and_complete() {
        let l = wordlist();
        assert_eq!(l.len(), 2048);
        assert!(l.windows(2).all(|w| w[0] < w[1]));
        assert_eq!(word_index("abandon"), Some(0));
        assert_eq!(word_index("zoo"), Some(2047));
    }

    #[test]
    fn standard_vectors() {
        // Official BIP-39 test vectors (all-zero entropy).
        let twelve = Mnemonic::parse(&format!("{} about", "abandon ".repeat(11)));
        assert_eq!(validate(&twelve), Ok(()));
        let twenty_four = Mnemonic::parse(&format!("{} art", "abandon ".repeat(23)));
        assert_eq!(validate(&twenty_four), Ok(()));
        let zoo = Mnemonic::parse(&format!("{} vote", "zoo ".repeat(23)));
        assert_eq!(validate(&zoo), Ok(()));
    }

    #[test]
    fn detects_errors() {
        let bad_sum = Mnemonic::parse(&"abandon ".repeat(24));
        assert_eq!(validate(&bad_sum), Err(MnemonicError::BadChecksum));
        let short = Mnemonic::parse("abandon about");
        assert_eq!(validate(&short), Err(MnemonicError::WrongLength(2)));
        let typo = Mnemonic::parse(&format!("{} abuot", "abandon ".repeat(11)));
        assert_eq!(
            validate(&typo),
            Err(MnemonicError::UnknownWord {
                position: 12,
                word: "abuot".into()
            })
        );
    }

    #[test]
    fn entropy_round_trip() {
        assert_eq!(
            from_entropy(&[0; 32]).words().join(" "),
            format!("{}art", "abandon ".repeat(23))
        );
        assert_eq!(
            from_entropy(&[0xff; 16]).words().join(" "),
            format!("{}wrong", "zoo ".repeat(11))
        );
        let m = from_entropy(&[0x7f; 32]);
        assert_eq!(m.len(), 24);
        assert_eq!(validate(&m), Ok(()));
    }

    #[test]
    fn suggests() {
        assert_eq!(suggestions("zo", 5), vec!["zone", "zoo"]);
        assert!(suggestions("", 5).is_empty());
        assert_eq!(suggestions("aba", 1), vec!["abandon"]);
    }
}
