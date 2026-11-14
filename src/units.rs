//! Sizes and counts as the original writes them, for the window and the
//! command line alike.

/// A count with its thousands apart: "12 345" (a narrow no-break space).
pub fn grouped(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push('\u{202f}');
        }
        out.push(c);
    }
    out
}

/// A size as the original writes it, in either language: "19 MB".
pub fn megabytes(bytes: u64) -> String {
    if bytes >= 1 << 30 {
        format!("{:.1} GB", bytes as f64 / (1u64 << 30) as f64)
    } else {
        format!("{} MB", (bytes >> 20).max(1))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thousands_stand_apart() {
        assert_eq!(grouped(7), "7");
        assert_eq!(grouped(999), "999");
        assert_eq!(grouped(1000), "1\u{202f}000");
        assert_eq!(grouped(1234567), "1\u{202f}234\u{202f}567");
    }

    #[test]
    fn sizes_are_megabytes_until_a_gigabyte() {
        // Anything under a megabyte is still one: nothing reads as "0 MB".
        assert_eq!(megabytes(0), "1 MB");
        assert_eq!(megabytes(19 << 20), "19 MB");
        assert_eq!(megabytes((1 << 30) - 1), "1023 MB");
        assert_eq!(megabytes(3 << 29), "1.5 GB");
    }
}
