use std::sync::OnceLock;
use regex::Regex;

macro_rules! native_harnesses {
    (native: [$($variant:ident => $name:literal),+], external: [$($external:literal),+]) => {
        #[derive(Clone, Copy)]
        pub(crate) enum Native { $($variant),+ }

        impl Native {
            pub fn parse(name: &str) -> Option<Self> {
                match name {
                    $($name => Some(Self::$variant)),+,
                    _ => None,
                }
            }
        }

        pub const NATIVE: &[&str] = &[$($name),+];
        pub const EXTERNAL: &[&str] = &[$($external),+];
        pub const ALL: &[&str] = &[$($name),+, $($external),+];
    };
}

native_harnesses!(
    native: [Claude => "claude", Codex => "codex", Omp => "omp", Pi => "pi", Hermes => "hermes", Grok => "grok", Devin => "devin"],
    external: ["openai-dot", "grok-bot"]
);

pub const SPAWNABLE: &[&str] = NATIVE;

/// Only harness binaries count as "running", not greps/scripts mentioning the id.
pub(crate) fn binary_pattern() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(&format!(r"(^|/)({})(\s|$)", crate::harness::NATIVE.join("|"))).unwrap())
}
