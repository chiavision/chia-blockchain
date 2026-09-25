//! Exact, integer-only amount handling.
//!
//! The Electron GUI converted amounts through JavaScript floats, which silently
//! loses precision above 2^53 mojo and can emit `1e+21`. Here every amount is a
//! `u64` count of mojos from the moment the user types it until it hits the
//! wire, and every step is checked.

use std::fmt;

/// 1 XCH = 10^12 mojo.
pub const MOJO_PER_XCH: u64 = 1_000_000_000_000;
/// 1 coloured-coin unit = 1000 mojo (matches `chia/cmds/units.py`).
pub const MOJO_PER_CC: u64 = 1_000;

/// A denomination: how many decimal places separate the display unit from mojos.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Denom {
    Xch,
    ColouredCoin,
}

impl Denom {
    pub const fn decimals(self) -> u32 {
        match self {
            Denom::Xch => 12,
            Denom::ColouredCoin => 3,
        }
    }

    pub const fn scale(self) -> u64 {
        match self {
            Denom::Xch => MOJO_PER_XCH,
            Denom::ColouredCoin => MOJO_PER_CC,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AmountError {
    #[error("enter an amount")]
    Empty,
    #[error("only digits and a single decimal point are allowed")]
    InvalidCharacter,
    #[error("at most {0} decimal places")]
    TooPrecise(u32),
    #[error("amount is too large")]
    Overflow,
}

/// Parse a user-entered decimal string into mojos.
///
/// Accepts `12`, `12.5`, `.5`, `0.000000000001`. Rejects signs, exponents,
/// separators, whitespace inside the number and more decimals than the
/// denomination supports — rather than rounding, which would send a
/// different amount from the one the user typed.
pub fn parse_amount(input: &str, denom: Denom) -> Result<u64, AmountError> {
    let s = input.trim();
    if s.is_empty() || s == "." {
        return Err(AmountError::Empty);
    }
    let (int_part, frac_part) = match s.split_once('.') {
        Some((i, f)) => (i, f),
        None => (s, ""),
    };
    if !int_part.bytes().all(|b| b.is_ascii_digit()) || !frac_part.bytes().all(|b| b.is_ascii_digit()) {
        return Err(AmountError::InvalidCharacter);
    }
    let decimals = denom.decimals();
    if frac_part.len() > decimals as usize {
        return Err(AmountError::TooPrecise(decimals));
    }

    let mut whole: u64 = 0;
    for b in int_part.bytes() {
        whole = whole
            .checked_mul(10)
            .and_then(|v| v.checked_add(u64::from(b - b'0')))
            .ok_or(AmountError::Overflow)?;
    }
    let mut frac: u64 = 0;
    for b in frac_part.bytes() {
        frac = frac * 10 + u64::from(b - b'0');
    }
    // Pad the fraction out to the full number of decimals.
    let pad = decimals - frac_part.len() as u32;
    frac *= 10u64.pow(pad);

    whole
        .checked_mul(denom.scale())
        .and_then(|v| v.checked_add(frac))
        .ok_or(AmountError::Overflow)
}

/// Format mojos exactly, trimming trailing zeros but keeping at least
/// `min_decimals` places. Thousands in the integer part are grouped with `,`.
pub fn format_amount(mojos: u64, denom: Denom, min_decimals: u32) -> String {
    let scale = denom.scale();
    let whole = mojos / scale;
    let frac = mojos % scale;
    let decimals = denom.decimals() as usize;

    let mut frac_str = format!("{frac:0decimals$}");
    let keep = (min_decimals as usize).min(decimals);
    while frac_str.len() > keep && frac_str.ends_with('0') {
        frac_str.pop();
    }

    let grouped = group_thousands(whole);
    if frac_str.is_empty() {
        grouped
    } else {
        format!("{grouped}.{frac_str}")
    }
}

/// Plain machine form (no grouping) — what goes back into an input field.
pub fn format_amount_plain(mojos: u64, denom: Denom) -> String {
    format_amount(mojos, denom, 0).replace(',', "")
}

fn group_thousands(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, ch) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

/// A mojo amount paired with its denomination, for display.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Amount {
    pub mojos: u64,
    pub denom: Denom,
}

impl fmt::Display for Amount {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&format_amount(self.mojos, self.denom, 2))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_exact_values() {
        assert_eq!(parse_amount("1", Denom::Xch), Ok(MOJO_PER_XCH));
        assert_eq!(parse_amount("0.000000000001", Denom::Xch), Ok(1));
        assert_eq!(parse_amount(".5", Denom::Xch), Ok(MOJO_PER_XCH / 2));
        assert_eq!(parse_amount(" 12.25 ", Denom::Xch), Ok(12_250_000_000_000));
        assert_eq!(parse_amount("5.", Denom::Xch), Ok(5 * MOJO_PER_XCH));
        assert_eq!(parse_amount("1.5", Denom::ColouredCoin), Ok(1_500));
        // Above 2^53 mojo, where the Electron GUI lost precision.
        assert_eq!(parse_amount("9007.199254740993", Denom::Xch), Ok(9_007_199_254_740_993));
    }

    #[test]
    fn rejects_ambiguous_input() {
        assert_eq!(parse_amount("", Denom::Xch), Err(AmountError::Empty));
        assert_eq!(parse_amount(".", Denom::Xch), Err(AmountError::Empty));
        assert_eq!(parse_amount("-1", Denom::Xch), Err(AmountError::InvalidCharacter));
        assert_eq!(parse_amount("1e3", Denom::Xch), Err(AmountError::InvalidCharacter));
        assert_eq!(parse_amount("1,000", Denom::Xch), Err(AmountError::InvalidCharacter));
        assert_eq!(parse_amount("1.2.3", Denom::Xch), Err(AmountError::InvalidCharacter));
        assert_eq!(parse_amount("1 000", Denom::Xch), Err(AmountError::InvalidCharacter));
        assert_eq!(
            parse_amount("0.0000000000001", Denom::Xch),
            Err(AmountError::TooPrecise(12))
        );
        assert_eq!(
            parse_amount("1.2345", Denom::ColouredCoin),
            Err(AmountError::TooPrecise(3))
        );
        assert_eq!(parse_amount("99999999", Denom::Xch), Err(AmountError::Overflow));
        assert_eq!(
            parse_amount("99999999999999999999999", Denom::Xch),
            Err(AmountError::Overflow)
        );
    }

    #[test]
    fn formats_exactly() {
        assert_eq!(format_amount(0, Denom::Xch, 2), "0.00");
        assert_eq!(format_amount(1, Denom::Xch, 2), "0.000000000001");
        assert_eq!(
            format_amount(1_234_567 * MOJO_PER_XCH + 5, Denom::Xch, 0),
            "1,234,567.000000000005"
        );
        assert_eq!(format_amount(1_500_000_000_000, Denom::Xch, 2), "1.50");
        assert_eq!(format_amount(u64::MAX, Denom::Xch, 0), "18,446,744.073709551615");
        assert_eq!(format_amount(1_000, Denom::ColouredCoin, 0), "1");
        assert_eq!(format_amount_plain(1_234_000 * MOJO_PER_XCH, Denom::Xch), "1234000");
    }

    #[test]
    fn round_trips() {
        for m in [0, 1, 999, MOJO_PER_XCH, 123_456_789_012_345, u64::MAX] {
            let s = format_amount_plain(m, Denom::Xch);
            assert_eq!(parse_amount(&s, Denom::Xch), Ok(m), "{s}");
        }
    }
}
