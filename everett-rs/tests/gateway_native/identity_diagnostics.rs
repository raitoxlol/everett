use super::*;
use std::collections::BTreeMap;

fn files(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    let mut out = BTreeMap::new();
    if let Ok(entries) = std::fs::read_dir(root) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() { out.extend(files(&path)); }
            else { out.insert(path.clone(), std::fs::read(path).unwrap()); }
        }
    }
    out
}
fn diagnose(c: &Ctx, config: Option<&Path>, input: &str) -> (i32, Value, String) {
    let mut cmd = c.command(&["identity", "diagnose", "--json"]);
    if let Some(path) = config { cmd.args(["--gateway-config",path.to_str().unwrap(),"--credentials-stdin"]); }
    cmd.env("EVERETT_OAUTH_INTROSPECTION_SECRET", INTROSPECTION_SECRET);
    cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child=cmd.spawn().unwrap();
    child.stdin.take().unwrap().write_all(input.as_bytes()).unwrap();
    let output=child.wait_with_output().unwrap();
    let raw=format!("{}{}",String::from_utf8_lossy(&output.stdout),String::from_utf8_lossy(&output.stderr));
    let report=serde_json::from_slice(&output.stdout).unwrap_or_else(|_|panic!("diagnostic JSON missing: {raw}"));
    (output.status.code().unwrap(),report,raw)
}
fn unchanged(c: &Ctx, config: Option<&Path>, input: &str) -> (i32,Value,String) {
    let before=files(&c.home);
    let result=diagnose(c,config,input);
    assert_eq!(files(&c.home),before,"diagnostic mutated profile/config/inbox files");
    assert!(c.records().is_empty(),"diagnostic invoked backend");
    result
}

pub(super) fn native() {
    let c=ctx();
    let (code,report,_)=unchanged(&c,None,"");
    assert_eq!(code,0); assert_eq!(report["mapping"],"unmapped");
    assert_eq!(report["reason"],"native_not_signed_in");
    let listener=TcpListener::bind("127.0.0.1:0").unwrap();
    let issuer=format!("http://{}",listener.local_addr().unwrap());
    let worker=std::thread::spawn(move || {
        for status in [200,401] {
            let (mut stream,_)=listener.accept().unwrap(); let mut buf=[0;8192]; let _=stream.read(&mut buf);
            let body=if status==200 { json!({"result":{"owner":"private-owner","deviceId":"private-device","role":"member"}}) } else {json!({"error":"revoked"})}.to_string();
            write!(stream,"HTTP/1.1 {status} Test\r\nContent-Length: {}\r\nContent-Type: application/json\r\nConnection: close\r\n\r\n{body}",body.len()).unwrap();
        }
    });
    std::fs::create_dir_all(c.home.join(".everett")).unwrap();
    std::fs::write(c.home.join(".everett/cloudflare-auth.json"),json!({"issuer":issuer,"owner":"private-owner","agent_id":"private-device","device_name":"private-label","token":"private-native-token","expires_at":4102444800000_u64}).to_string()).unwrap();
    let (code,report,raw)=unchanged(&c,None,"");
    assert_eq!(code,0); assert_eq!(report["account"],"verified"); assert_eq!(report["mapping"],"unmapped");
    assert_eq!(report["reason"],"native_ownership_provenance_unavailable");
    assert!(report["session_id"].is_null());
    for secret in ["private-owner","private-device","private-label","private-native-token"] {assert!(!raw.contains(secret));}
    let (code,report,_)=unchanged(&c,None,"");
    assert_eq!(code,1);assert_eq!(report["reason"],"native_credential_unverified");
    worker.join().unwrap();
}

pub(super) fn gateway() {
    let c=ctx();let issuer=fake_issuer();
    issuer.grant(&access('a'),"legacy-chat",json!({"everett_account":"private-owner"}));
    let port=c.oauth_server(&issuer.url);
    let path=c.config_path(&json!({"oauth":{"issuer":issuer.url,"resource":RESOURCE}}));
    let one=success(&native_call(port,'a',"everett_identity",json!({"action":"create","registration_key":"x".repeat(32),"name":"private-display-name","kind":"chat"})));
    let two=success(&native_call(port,'a',"everett_identity",json!({"action":"create","registration_key":"y".repeat(32),"name":"private-display-name","kind":"chat"})));
    let input=json!({"access_token":access('a'),"identity_handle":one["identity_handle"]}).to_string();
    let (code,report,raw)=unchanged(&c,Some(&path),&input);
    assert_eq!(code,0);assert_eq!(report["mapping"],"mapped");assert_eq!(report["session_id"],one["session_id"]);
    assert_eq!(report["availability"],"unknown"); assert_eq!(report["dispatch"],"not_observed");
    assert_eq!(report["pickup"],"not_observed");assert_eq!(report["reply"],"not_observed");
    let again=unchanged(&c,Some(&path),&input).1;
    assert_eq!(again,report);
    let second=unchanged(&c,Some(&path),&json!({"access_token":access('a'),"identity_handle":two["identity_handle"]}).to_string()).1;
    assert_ne!(second["label"],report["label"]);
    for secret in [access('a'),one["identity_handle"].as_str().unwrap().into(),INTROSPECTION_SECRET.into(),"private-owner".into(),"private-display-name".into()] {assert!(!raw.contains(&secret));}
    for changes in [json!({"everett_account":"foreign-owner"}),json!({"client_id":"forged-client","everett_account":"private-owner"}),json!({"active":false})] {
        issuer.grant(&access('a'),"legacy-chat",changes);
        let (code,report,_)=unchanged(&c,Some(&path),&input);
        assert_eq!(code,1);assert_ne!(report["mapping"],"mapped");assert!(report["session_id"].is_null());
    }
    issuer.grant(&access('a'),"legacy-chat",json!({"everett_account":"private-owner"}));
    let (code,report,_)=unchanged(&c,Some(&path),&json!({"access_token":access('a'),"identity_handle":"f".repeat(64)}).to_string());
    assert_eq!(code,1);assert_eq!(report["reason"],"identity_unresolved");
    success(&native_call(port,'a',"everett_identity",json!({"action":"revoke","identity_handle":one["identity_handle"]})));
    assert_eq!(unchanged(&c,Some(&path),&input).1["reason"],"identity_unresolved");
    std::fs::remove_file(c.home.join(".everett/external").join(format!("{}.json",two["session_id"].as_str().unwrap()))).unwrap();
    assert_eq!(unchanged(&c,Some(&path),&json!({"access_token":access('a'),"identity_handle":two["identity_handle"]}).to_string()).1["reason"],"registration_unavailable");
}

pub(super) fn invalid_input() {
    let c=ctx();let path=c.config_path(&json!({}));
    for input in ["secret-malformed-json".to_string(),json!({"access_token":"secret-token","identity_handle":"secret-handle","account":"forged-owner"}).to_string(),"x".repeat(20000)] {
        let (code,report,raw)=unchanged(&c,Some(&path),&input);
        assert_eq!(code,2);assert_eq!(report["reason"],"invalid_input");
        for secret in ["secret-malformed-json","secret-token","secret-handle","forged-owner"] {assert!(!raw.contains(secret));}
    }
}
