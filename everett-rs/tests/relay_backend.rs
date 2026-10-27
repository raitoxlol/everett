mod common;

#[test]
fn cloudflare_is_the_only_hosted_provider_and_transport() {
    let f = common::fixture();
    for command in ["login", "whoami", "logout"] {
        let help = f.run(&[command, "--help"]);
        let text = f.stdout(&help);
        assert!(help.status.success());
        assert!(text.contains("cloudflare"));
        assert!(!text.contains("oidc"), "{text}");
        assert!(!f.run(&[command, "--provider", "oidc"]).status.success());
    }
    let help = f.run(&["peer", "remove", "--help"]);
    let text = f.stdout(&help);
    assert!(text.contains("cloudflare"), "{text}");
    assert!(!text.contains("convex"), "{text}");
}

#[test]
fn relay_defaults_to_cloudflare_and_never_reads_retired_state() {
    let f = common::fixture();
    f.write(".everett/relay/peers/retired.json", "{}");
    let peers = f.run(&["peer", "list"]);
    assert!(peers.status.success());
    assert!(!f.stdout(&peers).contains("retired"));
    let out = f.run(&["relay", "register"]);
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("cloudflare"));
}
