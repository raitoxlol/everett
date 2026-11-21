//! One-shot backend for the gateway. Its wire protocol contains only the
//! operations a binding can grant and is carried over the child's stdio.
use std::{
    io::{self, Read},
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    process::Child,
};

use super::{Binding, BACKEND_ERROR, MAX_INBOX, MAX_RESULT};

const MAX_REQUEST: usize = MAX_INBOX + super::MAX_BODY;

pub(super) fn is_builtin(command: &[String]) -> bool {
    std::env::current_exe()
        .ok()
        .and_then(|path| path.canonicalize().ok())
        .is_some_and(|current| mcp_executable(command).as_ref() == Some(&current))
}

fn mcp_executable(command: &[String]) -> Option<PathBuf> {
    match command {
        [executable, mode] if mode == "mcp" => Path::new(executable).canonicalize().ok(),
        _ => None,
    }
}

#[derive(Clone)]
pub(super) struct PinnedWorker {
    executable: PathBuf,
    device: u64,
    inode: u64,
}

impl PinnedWorker {
    pub(super) fn new(command: &[String]) -> Option<Self> {
        let executable = std::env::current_exe().ok()?.canonicalize().ok()?;
        let metadata = executable.metadata().ok()?;
        let worker = Self {
            executable,
            device: metadata.dev(),
            inode: metadata.ino(),
        };
        worker.path_for(command)?;
        Some(worker)
    }

    pub(super) fn path_for(&self, command: &[String]) -> Option<&Path> {
        if mcp_executable(command).as_ref() != Some(&self.executable) {
            return None;
        }
        let metadata = self.executable.metadata().ok()?;
        (metadata.dev() == self.device && metadata.ino() == self.inode)
            .then_some(self.executable.as_path())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, os::unix::fs::symlink};

    #[test]
    fn pinned_worker_does_not_launch_through_a_configured_alias() {
        let dir = tempfile::tempdir().unwrap();
        let executable = dir.path().canonicalize().unwrap().join("everett");
        let alias = dir.path().join("alias");
        let replacement = dir.path().join("replacement");
        fs::write(&executable, "original executable").unwrap();
        fs::write(&replacement, "different executable").unwrap();
        symlink(&executable, &alias).unwrap();
        let metadata = executable.metadata().unwrap();
        let worker = PinnedWorker {
            executable,
            device: metadata.dev(),
            inode: metadata.ino(),
        };
        let command = [alias.to_string_lossy().into_owned(), "mcp".into()];
        let verified = worker.path_for(&command).unwrap();

        fs::remove_file(&alias).unwrap();
        symlink(&replacement, &alias).unwrap();
        assert_eq!(fs::read_to_string(verified).unwrap(), "original executable");
        assert!(worker.path_for(&command).is_none());

        fs::remove_file(&alias).unwrap();
        symlink(verified, &alias).unwrap();
        fs::rename(verified, dir.path().join("previous")).unwrap();
        fs::rename(&replacement, verified).unwrap();
        assert!(worker.path_for(&command).is_none());
    }
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) enum Call {
    OwnerAgent {
        name: String,
        arguments: Map<String, Value>,
    },
    Whoami,
    Core {
        project: String,
    },
    Inbox {
        peek: bool,
    },
    InboxMessage {
        peek: bool,
        message_id: String,
    },
    Card {
        what: String,
        state: String,
        next: String,
    },
    Send {
        to: String,
        text: String,
    },
    Queue {
        to: String,
        text: String,
    },
    Reply {
        message: Map<String, Value>,
        text: String,
    },
}

impl Call {
    /// Called only after the authenticated connection's binding validated the
    /// tool name and arguments. Typed variants pin the operation in the worker.
    pub(super) fn authorized(
        binding: &Binding,
        name: &str,
        args: &Map<String, Value>,
    ) -> Result<Self, String> {
        let string = |key: &str| {
            args.get(key)
                .and_then(Value::as_str)
                .map(str::to_owned)
                .ok_or_else(|| BACKEND_ERROR.to_string())
        };
        Ok(match name {
            "everett_whoami" => Self::Whoami,
            "everett_core" => Self::Core {
                project: binding.project.clone(),
            },
            "everett_inbox" => {
                let peek = args.get("peek").and_then(Value::as_bool).unwrap_or(false);
                if args.contains_key("message_id") {
                    Self::InboxMessage {
                        peek,
                        message_id: string("message_id")?,
                    }
                } else {
                    Self::Inbox { peek }
                }
            }
            "everett_card" => Self::Card {
                what: string("what")?,
                state: string("state")?,
                next: string("next")?,
            },
            "everett_send" if args.contains_key("to") => {
                let to = string("to")?;
                let text = string("text")?;
                if binding.conversation_identity {
                    Self::Queue { to, text }
                } else {
                    Self::Send { to, text }
                }
            }
            "everett_send" => Self::Reply {
                message: binding
                    .reply_message(&string("reply_to")?)?
                    .as_object()
                    .ok_or(BACKEND_ERROR)?
                    .clone(),
                text: string("text")?,
            },
            _ => return Err("tool is not allowed".into()),
        })
    }

    fn execute(self, session: &str, harness: &str) -> Result<Value, String> {
        let identity = Map::from_iter([
            ("session_id".into(), json!(session)),
            ("harness".into(), json!(harness)),
            ("source".into(), json!("env EVERETT_SESSION_ID")),
        ]);
        let args = |value: Value| value.as_object().expect("argument object").clone();
        use crate::mcp;
        let data = match self {
            Self::Whoami => mcp::tool_whoami(&identity),
            Self::Core { project } => mcp::tool_core(&args(json!({"project": project}))),
            Self::Inbox { peek } => mcp::tool_inbox(&args(json!({"peek": peek})), &identity),
            Self::InboxMessage { peek, message_id } if crate::inbox::valid_id(&message_id) => {
                mcp::tool_inbox(
                    &args(json!({"peek": peek, "message_id": message_id})),
                    &identity,
                )
            }
            Self::Card { what, state, next } => mcp::tool_card(
                &args(json!({"what": what, "state": state, "next": next})),
                &identity,
            ),
            Self::Send { to, text } if super::valid_destination(&to) => mcp::tool_send(
                &args(json!({"mode":"inbox", "to":to, "text":text})),
                &identity,
                true,
            ),
            Self::Queue { to, text } if super::valid_destination(&to) => {
                crate::send::queue_conversation_inbox(&to, &text, session, harness).map_err(mcp::ToolFailure::from)
            }
            Self::Reply { message, text }
                if message.get("to").and_then(Value::as_str) == Some(session)
                    && message
                        .get("id")
                        .and_then(Value::as_str)
                        .is_some_and(|id| !id.is_empty() && id.chars().count() <= 128)
                    && message
                        .get("from")
                        .and_then(Value::as_str)
                        .is_some_and(super::valid_destination)
                    && message
                        .get("text")
                        .and_then(Value::as_str)
                        .is_some_and(|text| !text.is_empty())
                    && !message.contains_key("relay_device")
                    && !message.contains_key("peer_client") =>
            {
                crate::send::reply_to_authorized_local_message(&message, &text, session, harness)
                    .map(|mut data| {
                        data.extend([
                            ("delivered".into(), json!(false)),
                            ("queued".into(), json!(true)),
                            ("wake".into(), json!(false)),
                        ]);
                        data
                    })
                    .map_err(mcp::ToolFailure::from)
            }
            _ => return Err("backend rejected the tool call".into()),
        }
        .map_err(|_| "backend rejected the tool call")?;
        Ok(Value::Object(data))
    }
}

pub(super) async fn dispatch(call: Call, child: &mut Child) -> Result<Value, String> {
    let stdout = child.stdout.take().ok_or(BACKEND_ERROR)?;
    let mut stdin = child.stdin.take().ok_or(BACKEND_ERROR)?;
    let request = serde_json::to_vec(&call).map_err(|_| BACKEND_ERROR)?;
    if request.len() > MAX_REQUEST {
        return Err(BACKEND_ERROR.into());
    }
    stdin.write_all(&request).await.map_err(|_| BACKEND_ERROR)?;
    drop(stdin);
    let mut response = Vec::new();
    stdout
        .take((MAX_RESULT + 1) as u64)
        .read_to_end(&mut response)
        .await
        .map_err(|_| BACKEND_ERROR)?;
    if response.len() > MAX_RESULT {
        return Err("backend result exceeds 256 KiB".into());
    }
    let data: Option<Value> = serde_json::from_slice(&response).map_err(|_| BACKEND_ERROR)?;
    data.ok_or_else(|| "backend rejected the tool call".into())
}

fn binding() -> Option<(String, String)> {
    let session = std::env::var("EVERETT_SESSION_ID").ok()?;
    let harness = std::env::var("EVERETT_HARNESS_NAME").ok()?;
    (crate::adapters::external::valid_id(&session) && crate::adapters::external::is_external(&harness))
        .then_some((session, harness))
}

fn mcp_binding() -> Option<(String, String)> {
    if std::env::var("EVERETT_GATEWAY_WORKER").as_deref() == Ok("1") {
        binding()
    } else {
        None
    }
}

pub(crate) fn mcp_enabled() -> bool {
    mcp_binding().is_some()
}

pub(crate) fn mcp_call(params: &Map<String, Value>) -> Result<Value, String> {
    let (session, harness) = mcp_binding().ok_or(BACKEND_ERROR)?;
    let request = params.get("request").ok_or(BACKEND_ERROR)?;
    let bytes = serde_json::to_vec(request).map_err(|_| BACKEND_ERROR)?;
    if bytes.len() > MAX_REQUEST {
        return Err(BACKEND_ERROR.into());
    }
    let call: Call = serde_json::from_slice(&bytes).map_err(|_| BACKEND_ERROR)?;
    call.execute(&session, &harness)
}

#[doc(hidden)]
pub fn run() -> i32 {
    let Some((session, harness)) = binding() else {
        return 2;
    };
    let mut input = Vec::new();
    if io::stdin()
        .take((MAX_REQUEST + 1) as u64)
        .read_to_end(&mut input)
        .is_err()
        || input.len() > MAX_REQUEST
    {
        return 2;
    }
    let Ok(call) = serde_json::from_slice::<Call>(&input) else {
        return 2;
    };
    let data = match call {
        Call::OwnerAgent { name, arguments }
            if std::env::var("EVERETT_GATEWAY_OWNER_AGENT").as_deref() == Ok("1") =>
        {
            let response = crate::mcp::handle(&json!({
                "jsonrpc": "2.0", "id": 1, "method": "tools/call",
                "params": {"name": name, "arguments": arguments}
            }), None);
            response.map(|mut response| {
                if let Some(message) = response["error"]["message"].as_str() {
                    json!({"isError":true,"content":[{"type":"text","text":message}]})
                } else {
                    response["result"].take()
                }
            })
        }
        call => call.execute(&session, &harness).ok(),
    };
    if serde_json::to_writer(io::stdout().lock(), &data).is_err() {
        2
    } else {
        0
    }
}
