//! Strict Chia address handling (bech32m over a 32-byte puzzle hash).
//!
//! The backend's `decode_puzzle_hash` ignores the human-readable prefix and the
//! payload length, so a testnet address would be accepted on mainnet. The old
//! GUI only checked that an address didn't contain the word "colour". We check
//! the checksum variant, the prefix and the length before anything is sent.

use bech32::primitives::decode::CheckedHrpstring;
use bech32::{Bech32m, Hrp};

pub const PUZZLE_HASH_LEN: usize = 32;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AddressError {
    #[error("enter an address")]
    Empty,
    #[error("not a valid address (checksum or characters are wrong)")]
    Malformed,
    #[error("this is a {found} address, but this wallet is on {expected}")]
    WrongNetwork { expected: String, found: String },
    #[error("address has the wrong length")]
    WrongLength,
    #[error("invalid network prefix")]
    BadPrefix,
}

/// Encode a puzzle hash as a bech32m address with the given prefix (`xch`, `txch`).
pub fn encode_puzzle_hash(puzzle_hash: &[u8; PUZZLE_HASH_LEN], prefix: &str) -> Result<String, AddressError> {
    let hrp = Hrp::parse(prefix).map_err(|_| AddressError::BadPrefix)?;
    bech32::encode::<Bech32m>(hrp, puzzle_hash).map_err(|_| AddressError::BadPrefix)
}

/// Decode and fully validate an address for the network with `expected_prefix`.
pub fn decode_address(input: &str, expected_prefix: &str) -> Result<[u8; PUZZLE_HASH_LEN], AddressError> {
    let s = input.trim();
    if s.is_empty() {
        return Err(AddressError::Empty);
    }
    // Only bech32m is valid for Chia; `CheckedHrpstring` also rejects mixed case.
    let checked = CheckedHrpstring::new::<Bech32m>(s).map_err(|_| AddressError::Malformed)?;
    let hrp = checked.hrp().to_lowercase();
    if hrp != expected_prefix.to_ascii_lowercase() {
        return Err(AddressError::WrongNetwork {
            expected: expected_prefix.to_string(),
            found: hrp,
        });
    }
    let bytes: Vec<u8> = checked.byte_iter().collect();
    bytes.try_into().map_err(|_| AddressError::WrongLength)
}

/// Split an address into short groups so a human can compare it against
/// another copy group by group (clipboard hijackers usually only match the
/// first and last few characters).
pub fn chunk_address(address: &str, group: usize) -> Vec<String> {
    let chars: Vec<char> = address.chars().collect();
    chars.chunks(group.max(1)).map(|c| c.iter().collect()).collect()
}

/// Decode a `0x…` hex puzzle hash as returned by the RPC.
pub fn parse_hex32(s: &str) -> Option<[u8; 32]> {
    let raw = s.strip_prefix("0x").unwrap_or(s);
    let bytes = hex::decode(raw).ok()?;
    bytes.try_into().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    // Vectors produced by chia/util/bech32m.py:encode_puzzle_hash.
    const ZERO_XCH: &str = "xch1qqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqq2u30kz";
    const SEQ_TXCH: &str = "txch1qqqsyqcyq5rqwzqfpg9scrgwpugpzysnzs23v9ccrydpk8qarc0sw0amhg";
    const FF_XCH: &str = "xch1lllllllllllllllllllllllllllllllllllllllllllllllllllsgzj7l4";

    fn seq() -> [u8; 32] {
        std::array::from_fn(|i| i as u8)
    }

    #[test]
    fn matches_chia_python_encoder() {
        assert_eq!(encode_puzzle_hash(&[0; 32], "xch").unwrap(), ZERO_XCH);
        assert_eq!(encode_puzzle_hash(&seq(), "txch").unwrap(), SEQ_TXCH);
        assert_eq!(encode_puzzle_hash(&[0xff; 32], "xch").unwrap(), FF_XCH);
    }

    #[test]
    fn decodes_valid() {
        assert_eq!(decode_address(ZERO_XCH, "xch"), Ok([0; 32]));
        assert_eq!(decode_address(&format!("  {SEQ_TXCH}\n"), "txch"), Ok(seq()));
        assert_eq!(decode_address(&FF_XCH.to_uppercase(), "xch"), Ok([0xff; 32]));
    }

    #[test]
    fn rejects_wrong_network() {
        assert_eq!(
            decode_address(SEQ_TXCH, "xch"),
            Err(AddressError::WrongNetwork {
                expected: "xch".into(),
                found: "txch".into()
            })
        );
    }

    #[test]
    fn rejects_corruption() {
        // Flip one character.
        let mut bad = ZERO_XCH.to_string();
        bad.replace_range(10..11, "p");
        assert_eq!(decode_address(&bad, "xch"), Err(AddressError::Malformed));
        // Mixed case.
        let mixed = format!("XCH{}", &ZERO_XCH[3..]);
        assert_eq!(decode_address(&mixed, "xch"), Err(AddressError::Malformed));
        assert_eq!(decode_address("", "xch"), Err(AddressError::Empty));
        assert_eq!(
            decode_address("chia_addr://xch1abc", "xch"),
            Err(AddressError::Malformed)
        );
    }

    #[test]
    fn rejects_bech32_non_m_and_wrong_length() {
        let classic = bech32::encode::<bech32::Bech32>(Hrp::parse("xch").unwrap(), &[0u8; 32]).unwrap();
        assert_eq!(decode_address(&classic, "xch"), Err(AddressError::Malformed));
        let short = bech32::encode::<Bech32m>(Hrp::parse("xch").unwrap(), &[0u8; 20]).unwrap();
        assert_eq!(decode_address(&short, "xch"), Err(AddressError::WrongLength));
    }

    #[test]
    fn chunks() {
        assert_eq!(chunk_address("abcdefghij", 4), vec!["abcd", "efgh", "ij"]);
        assert_eq!(parse_hex32(&format!("0x{}", "11".repeat(32))), Some([0x11; 32]));
        assert_eq!(parse_hex32("0x11"), None);
    }
}
