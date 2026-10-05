/// `f"{x:g}"` for the values Everett prints (durations, timeouts): plain decimal with no
/// trailing zeros, like Rust's Display for f64 but switching to exponent notation only for
/// magnitudes Python's %g would too. Rust `{}` already gives the shortest round-trip form,
/// which matches %g for every practical value here except scientific notation extremes.
pub fn g(x: f64) -> String {
    if x.is_finite() && x != 0.0 && (x.abs() < 1e-4 || x.abs() >= 1e6) {
        let s = format!("{:e}", x);
        let mut parts = s.split('e');
        let mantissa = parts.next().unwrap_or("0").trim_end_matches('0').trim_end_matches('.');
        let exp: i32 = parts.next().and_then(|e| e.parse().ok()).unwrap_or(0);
        return format!("{}e{:+03}", mantissa, exp);
    }
    let s = format!("{}", x);
    s
}
