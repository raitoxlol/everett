use crate::config;
use std::process::Command;

pub fn issuer() -> String {
    config::get("auth.issuer", Some("EVERETT_AUTH_ISSUER"), "")
}

pub fn base_url() -> String {
    config::get("auth.base_url", Some("EVERETT_AUTH_BASE_URL"), "")
}

pub fn hostname() -> String {
    let mut buf = vec![0u8; 256];
    // SAFETY: buf is a valid writable buffer; gethostname writes a NUL-terminated name.
    let rc = unsafe { libc::gethostname(buf.as_mut_ptr() as *mut _, buf.len()) };
    if rc == 0 {
        let end = buf.iter().position(|b| *b == 0).unwrap_or(buf.len());
        let name = String::from_utf8_lossy(&buf[..end]).to_string();
        if !name.is_empty() {
            return name;
        }
    }
    Command::new("hostname")
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "this device".to_string())
}
