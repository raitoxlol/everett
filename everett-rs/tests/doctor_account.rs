mod common;

use common::{fixture, Fixture};
use serde_json::json;
use std::process::Command;

const ISSUER: &str = "https://everett.example";

struct AuthProviderEnv(Option<std::ffi::OsString>);

impl Drop for AuthProviderEnv {
    fn drop(&mut self) {
        match &self.0 {
            Some(value) => std::env::set_var("EVERETT_AUTH_PROVIDER", value),
            None => std::env::remove_var("EVERETT_AUTH_PROVIDER"),
        }
    }
}

fn cloudflare(f: &Fixture, expires_at: u64) {
    f.write(
        ".everett/cloudflare-auth.json",
        &json!({
            "issuer": ISSUER,
            "agent_id": "fixture-agent",
            "owner": "fixture-owner",
            "device_name": "fixture-mac",
            "token": "do-not-print-cloudflare-token",
            "expires_at": expires_at
        })
        .to_string(),
    );
}

fn oidc(f: &Fixture) {
    f.write(
        ".everett/auth.json",
        &json!({
            "issuer": "https://oidc.example",
            "client_id": "fixture-client",
            "sub": "fixture-subject",
            "email": "oidc@example.com",
            "device_name": "oidc-mac",
            "access_token": "do-not-print-oidc-token",
            "expires_at": 9999999999_u64
        })
        .to_string(),
    );
}

fn doctor(f: &Fixture, provider: &str) -> String {
    f.write(".everett/config.toml", &format!("[auth]\nprovider = {provider:?}\n"));
    let output = Command::new(env!("CARGO_BIN_EXE_everett"))
        .args(["doctor"])
        .env("HOME", f.home())
        .env("EVERETT_HOME", f.home())
        .env("PATH", common::HERMETIC_PATH)
        .env_remove("EVERETT_AUTH_PROVIDER")
        .env_remove("EVERETT_AUTH_ISSUER")
        .env_remove("EVERETT_CONVEX_URL")
        .env_remove("TYPESAFE_API_KEY")
        .output()
        .unwrap();
    let text = f.stdout(&output);
    assert!(!text.contains("do-not-print"), "{text}");
    text
}

#[test]
fn doctor_reports_selected_cloudflare_account_without_claiming_live_verification() {
    let f = fixture();
    cloudflare(&f, u64::MAX);
    oidc(&f);
    let text = doctor(&f, "cloudflare");
    assert!(text.contains("account: Cloudflare fixture-owner via https://everett.example (fixture-mac; stored credential, not verified online)"), "{text}");
    assert!(!text.contains("account: oidc@example.com"), "{text}");
    assert!(!text.contains("account: not signed in"), "{text}");
    assert!(text.contains("device revocations: Cloudflare SSH revocation check unavailable; SSH authorizations unchanged"), "{text}");
}

#[test]
fn doctor_reports_missing_expired_and_invalid_cloudflare_credentials() {
    let f = fixture();
    oidc(&f);
    let text = doctor(&f, "cloudflare");
    assert!(text.contains("account: not signed in to Cloudflare"), "{text}");
    assert!(!text.contains("account: oidc@example.com"), "{text}");

    cloudflare(&f, 1);
    let text = doctor(&f, "cloudflare");
    assert!(text.contains("account: Cloudflare credential expired; run `everett login`"), "{text}");

    f.write(".everett/cloudflare-auth.json", "invalid do-not-print");
    let text = doctor(&f, "cloudflare");
    assert!(text.contains("account: stored Cloudflare sign-in is invalid"), "{text}");
}

#[test]
fn doctor_uses_oidc_when_cloudflare_credentials_also_exist() {
    let f = fixture();
    cloudflare(&f, u64::MAX);
    oidc(&f);
    let text = doctor(&f, "oidc");
    assert!(text.contains("account: oidc@example.com"), "{text}");
    assert!(!text.contains("account: Cloudflare"), "{text}");
}

#[test]
fn doctor_does_not_treat_an_unknown_provider_as_oidc() {
    let f = fixture();
    oidc(&f);
    let text = doctor(&f, "unknown");
    assert!(text.contains("account: auth.provider must be oidc or cloudflare"), "{text}");
    assert!(!text.contains("account: oidc@example.com"), "{text}");
}

#[test]
fn cloudflare_diagnostics_do_not_fetch_oidc_revocations_or_modify_ssh() {
    let home = common::lib_home();
    let _provider = AuthProviderEnv(std::env::var_os("EVERETT_AUTH_PROVIDER"));
    std::env::remove_var("EVERETT_AUTH_PROVIDER");
    let f = fixture();
    oidc(&f);
    std::fs::create_dir_all(home.path().join(".everett")).unwrap();
    std::fs::copy(f.home().join(".everett/auth.json"), home.path().join(".everett/auth.json")).unwrap();
    std::fs::write(home.path().join(".everett/config.toml"), "[auth]\nprovider = \"cloudflare\"\n").unwrap();
    std::fs::create_dir_all(home.path().join(".ssh")).unwrap();
    let path = home.path().join(".ssh/authorized_keys");
    let before = b"ssh-ed25519 fixture # everett-peer:fixture\n";
    std::fs::write(&path, before).unwrap();
    let checks = everett::doctor::device_revocation_checks(|| panic!("Cloudflare must not fetch OIDC revocations"));
    assert_eq!(checks, vec![(None, "device revocations: Cloudflare SSH revocation check unavailable; SSH authorizations unchanged".into())]);
    assert_eq!(std::fs::read(path).unwrap(), before);
}
