use std::{
    collections::BTreeSet,
    fs::OpenOptions,
    io::Read,
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::Path,
};

use serde::{Deserialize, Serialize};

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub(super) enum FullAccess {
    All,
}

#[derive(Deserialize, Serialize)]
#[serde(untagged)]
pub(super) enum Grant {
    All(FullAccess),
    Tools(BTreeSet<String>),
}

impl Grant {
    pub(super) fn load(path: &Path) -> Result<Self, String> {
        let file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(path)
            .map_err(|_| "cannot open agent tool permissions")?;
        let metadata = file.metadata().map_err(|_| "cannot inspect agent tool permissions")?;
        if !metadata.is_file() || metadata.permissions().mode() & 0o077 != 0 {
            return Err("agent tool permissions must be a private regular file".into());
        }
        let mut bytes = Vec::new();
        file.take((super::MAX_CONFIG + 1) as u64).read_to_end(&mut bytes)
            .map_err(|_| "cannot read agent tool permissions")?;
        if bytes.len() > super::MAX_CONFIG {
            return Err("agent tool permissions exceed size limit".into());
        }
        let grant: Self = serde_json::from_slice(&bytes).map_err(|_| "invalid agent tool permissions")?;
        grant.validate()?;
        Ok(grant)
    }

    fn known(name: &str) -> bool {
        crate::mcp::tools().iter().any(|tool| tool["name"] == name)
    }

    fn validate(&self) -> Result<(), String> {
        if let Self::Tools(tools) = self {
            if tools.iter().any(|name| !Self::known(name)) {
                return Err("agent tool permissions contain an unknown tool".into());
            }
        }
        Ok(())
    }

    pub(super) fn permits(&self, name: &str) -> bool {
        Self::known(name) && match self {
            Self::All(_) => true,
            Self::Tools(tools) => tools.contains(name),
        }
    }
}

pub fn run(path: &Path, all: bool, allow: Vec<String>, deny_all: bool) -> i32 {
    let execute = || -> Result<(), String> {
        if !path.is_absolute() {
            return Err("permission file path must be absolute".into());
        }
        let grant = if all {
            Grant::All(FullAccess::All)
        } else if deny_all || !allow.is_empty() {
            Grant::Tools(allow.into_iter().collect())
        } else {
            let grant = Grant::load(path)?;
            println!("{}", serde_json::to_string(&grant).expect("grant is serializable"));
            return Ok(());
        };
        grant.validate()?;
        let encoded = serde_json::to_vec(&grant).expect("grant is serializable");
        crate::storage::save(path, &encoded).map_err(|error| error.to_string())?;
        println!("{}", String::from_utf8_lossy(&encoded));
        Ok(())
    };
    match execute() {
        Ok(()) => 0,
        Err(error) => {
            eprintln!("everett.gateway-permissions: {error}");
            2
        }
    }
}
