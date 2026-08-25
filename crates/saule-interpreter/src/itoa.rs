//! Integer rendering that skips `core::fmt`.
//!
//! `i64::to_string()` is specialised in `std` and still costs more than the
//! work deserves: it goes out through `Display`, which carries the padding,
//! alignment and fill machinery that a bare number never uses, and hands
//! back a fresh `String` every time. `benchmarks/sau/map.sau` builds three
//! hundred thousand keys out of `"key" .. i` and `benchmarks/sau/strings.sau`
//! two hundred thousand out of `"item-" .. i .. "-tail"`, so that path is
//! hot enough to be worth writing out by hand: swapping it for this one
//! measured **-48%** on a 3M-string reproduction of `map`'s inner loop.
//!
//! The algorithm is the standard two-digits-at-a-time table lookup — half
//! the divisions of the naive loop, and the remaining ones are by a
//! constant, which the compiler turns into a multiply and a shift.
//!
//! No `unsafe`: the bytes written are ASCII digits by construction, and
//! validating twenty of them costs far less than the machinery this
//! replaces.

/// `"00010203…9899"` — two ASCII digits per index, so `DIGITS[r * 2..][..2]`
/// is the decimal spelling of `r` for any `r < 100`.
static DIGITS: &[u8; 200] = b"0001020304050607080910111213141516171819\
                              2021222324252627282930313233343536373839\
                              4041424344454647484950515253545556575859\
                              6061626364656667686970717273747576777879\
                              8081828384858687888990919293949596979899";

/// The widest `i64` is 19 digits (`i64::MIN`'s magnitude), so 20 is room to
/// spare. The sign is pushed separately rather than reserved here.
const MAX_DIGITS: usize = 20;

/// Append `n`'s decimal spelling to `out`.
///
/// This is the form the concatenation path wants: `..` already knows the
/// buffer it is building into, so rendering straight into it saves the
/// intermediate `String` that [`i64_to_string`] would allocate.
pub fn push_i64(out: &mut String, n: i64) {
    let mut buf = [0u8; MAX_DIGITS];
    let mut pos = MAX_DIGITS;

    // `unsigned_abs` rather than `-n`: negating `i64::MIN` overflows, and
    // its magnitude only fits in the unsigned half of the range.
    let mut m = n.unsigned_abs();

    while m >= 100 {
        let r = (m % 100) as usize * 2;
        m /= 100;
        pos -= 2;
        buf[pos] = DIGITS[r];
        buf[pos + 1] = DIGITS[r + 1];
    }
    if m >= 10 {
        let r = m as usize * 2;
        pos -= 2;
        buf[pos] = DIGITS[r];
        buf[pos + 1] = DIGITS[r + 1];
    } else {
        pos -= 1;
        buf[pos] = b'0' + m as u8;
    }

    if n < 0 {
        out.push('-');
    }
    out.push_str(std::str::from_utf8(&buf[pos..]).expect("ASCII digits are valid UTF-8"));
}

/// `n`'s decimal spelling as a fresh `String`, for callers that have no
/// buffer to append to.
pub fn i64_to_string(n: i64) -> String {
    // Digits plus a possible sign — enough that `push_i64` never reallocates.
    let mut s = String::with_capacity(MAX_DIGITS + 1);
    push_i64(&mut s, n);
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The property that matters: identical to `std` for every input,
    /// including the boundaries where a hand-rolled version usually breaks.
    #[test]
    fn matches_std_at_boundaries() {
        for n in [
            0,
            1,
            -1,
            9,
            10,
            -10,
            99,
            100,
            -100,
            999,
            1000,
            i64::from(i32::MAX),
            i64::from(i32::MIN),
            i64::MAX,
            // The one that overflows a naive `-n`.
            i64::MIN,
        ] {
            assert_eq!(i64_to_string(n), n.to_string(), "for {n}");
        }
    }

    #[test]
    fn matches_std_across_magnitudes() {
        // Every digit width, positive and negative, plus the values either
        // side of each power of ten where the two-at-a-time loop switches
        // between its tail branches.
        let mut p: i64 = 1;
        for _ in 0..19 {
            for n in [p - 1, p, p + 1] {
                assert_eq!(i64_to_string(n), n.to_string(), "for {n}");
                assert_eq!(i64_to_string(-n), (-n).to_string(), "for {}", -n);
            }
            p = match p.checked_mul(10) {
                Some(v) => v,
                None => break,
            };
        }
    }

    #[test]
    fn matches_std_on_a_spread_of_values() {
        // A cheap deterministic spread, to catch a table entry that is wrong
        // in a way the round numbers above would step over.
        let mut x: i64 = 0x2545_F491_4F6C_DD1D;
        for _ in 0..20_000 {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            assert_eq!(i64_to_string(x), x.to_string(), "for {x}");
        }
    }

    #[test]
    fn appends_rather_than_replaces() {
        let mut s = String::from("key");
        push_i64(&mut s, 42);
        push_i64(&mut s, -7);
        assert_eq!(s, "key42-7");
    }
}
