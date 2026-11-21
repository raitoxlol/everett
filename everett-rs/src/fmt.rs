/// `f"{x:g}"` for the values Everett prints (durations, timeouts): Python's `%g` with
/// its default precision of 6 significant digits, trailing zeros removed, and
/// exponent notation when the rounded exponent is below -4 or at least 6.
pub fn g(x: f64) -> String {
    const P: i32 = 6;
    if !x.is_finite() {
        return if x.is_nan() { "nan".into() } else if x > 0.0 { "inf".into() } else { "-inf".into() };
    }
    if x == 0.0 {
        return if x.is_sign_negative() { "-0".into() } else { "0".into() };
    }
    // Round to P significant digits first; the exponent of the rounded value decides the form.
    let sci = format!("{:.*e}", (P - 1) as usize, x);
    let (mantissa, exp) = sci.split_once('e').unwrap_or((&sci, "0"));
    let exp: i32 = exp.parse().unwrap_or(0);
    if (-4..P).contains(&exp) {
        let fixed = format!("{:.*}", (P - 1 - exp) as usize, x);
        strip_zeros(&fixed).to_string()
    } else {
        format!("{}e{}{:02}", strip_zeros(mantissa), if exp < 0 { '-' } else { '+' }, exp.abs())
    }
}

fn strip_zeros(s: &str) -> &str {
    if s.contains('.') { s.trim_end_matches('0').trim_end_matches('.') } else { s }
}

#[cfg(test)]
mod tests {
    use super::g;

    #[test]
    fn matches_python_percent_g() {
        // Expected values from Python's f"{x:g}".
        for (x, want) in [
            (0.0, "0"),
            (1.0, "1"),
            (24.0, "24"),
            (0.5, "0.5"),
            (1800.0, "1800"),
            (1.234567891, "1.23457"),
            (0.1 + 0.2, "0.3"),
            (999999.0, "999999"),
            (999999.5, "1e+06"),
            (1e6, "1e+06"),
            (123456789.0, "1.23457e+08"),
            (0.0001, "0.0001"),
            (0.00001, "1e-05"),
            (0.000123456789, "0.000123457"),
            (-2.5, "-2.5"),
            (1e100, "1e+100"),
        ] {
            assert_eq!(g(x), want, "g({x:?})");
        }
    }
}
