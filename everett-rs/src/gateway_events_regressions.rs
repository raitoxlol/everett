use super::*;
use std::{io::Write, net::TcpListener};

#[test]
fn terminal_status_survives_oversized_response() {
    for status in [200,410,413] {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let thread = std::thread::spawn(move || {
            let (mut stream,_) = listener.accept().unwrap();
            let mut buf = [0;4096]; let _ = stream.read(&mut buf);
            write!(stream,"HTTP/1.1 {status} Test\r\nContent-Length: 9000\r\nConnection: close\r\n\r\n{}", "x".repeat(9000)).unwrap();
        });
        let agent = ureq::Agent::config_builder().proxy(None).http_status_as_error(false).build().new_agent();
        let response = agent.get(format!("http://{addr}/")).call().unwrap();
        assert_eq!(finish_response(response).unwrap().0,status);
        thread.join().unwrap();
    }
}

#[test]
fn historical_inbox_over_eight_mib_still_delivers_fresh_message() {
    struct Echo(Mutex<Vec<Value>>);
    impl Webhook for Echo {
        fn post(&self, _: &str, _: &[(String,String)], body: &[u8]) -> Result<(u16,Vec<u8>), &'static str> {
            let v:Value=serde_json::from_slice(body).unwrap();
            self.0.lock().unwrap().push(v.clone());
            Ok((200,serde_json::to_vec(&json!({"challenge":v["challenge"]})).unwrap()))
        }
    }
    let home=tempfile::tempdir().unwrap();
    let transport=Arc::new(Echo(Mutex::new(vec![])));
    let events=Events::with_db(Connection::open_in_memory().unwrap(),home.path().into(),transport.clone()).unwrap();
    let p=Principal{owner:"owner".into(),agent:"dot-a".into(),token:None};
    let args=json!({"name":"message.created","arguments":{"session_id":"dot-a"},"delivery":{"mode":"webhook","url":"https://callback.example/","secret":format!("whsec_{}",STANDARD.encode([7;32]))}});
    events.subscribe(&p,&args).unwrap();
    fs::create_dir_all(home.path().join(".everett/inbox")).unwrap();
    let mut file=File::create(home.path().join(".everett/inbox/dot-a.jsonl")).unwrap();
    for n in 0..600 { writeln!(file,"{}",json!({"id":format!("old{n}"),"to":"dot-a","ts":1,"text":"x".repeat(16000)})).unwrap(); }
    writeln!(file,"{}",json!({"id":"fresh","to":"dot-a","ts":now() as f64/1000.})).unwrap();
    events.deliver(&events.leases().unwrap()[0]).unwrap();
    assert_eq!(transport.0.lock().unwrap()[1]["data"]["message_id"],"fresh");
}
