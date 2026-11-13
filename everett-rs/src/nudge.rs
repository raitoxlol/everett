use crate::error::{EverettError, Result};

pub(crate) fn nudge_commands(
    session_id: &str,
    message_id: &str,
    harness: &str,
) -> Result<(String, String)> {
    let command = NudgeCommand::resolve()?.prefix();
    Ok((
        format!("{command} inbox --session {session_id} --message {message_id}"),
        format!("EVERETT_SESSION_ID={session_id} EVERETT_HARNESS_NAME={harness} {command} reply {message_id} '<your concise result>'"),
    ))
}

pub(crate) struct NudgeCommand {
    pub executable: std::path::PathBuf,
    pub everett_home: std::path::PathBuf,
    pub auth_provider: String,
}

impl NudgeCommand {
    pub fn resolve() -> Result<Self> {
        Ok(Self {
            executable: std::env::current_exe()
                .map_err(|e| EverettError::new(2, format!("cannot resolve Everett executable: {e}")))?,
            everett_home: std::path::absolute(crate::session::home())
                .map_err(|e| EverettError::new(2, format!("cannot resolve Everett profile against sender cwd: {e}")))?,
            auth_provider: crate::config::get("auth.provider", Some("EVERETT_AUTH_PROVIDER"), "cloudflare"),
        })
    }

    pub fn prefix(&self) -> String {
        format!("{}{}", command_env_prefix(&self.everett_home.to_string_lossy(), &self.auth_provider),
            shell_quote(&self.executable.to_string_lossy()))
    }
}

fn command_env_prefix(everett_home: &str, auth_provider: &str) -> String {
    format!(
        "EVERETT_HOME={} EVERETT_AUTH_PROVIDER={} ",
        shell_quote(everett_home),
        shell_quote(auth_provider)
    )
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}


#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn nudge_commands_propagate_quoted_profile_and_provider_without_changing_home() {
        let prefix = command_env_prefix("/tmp/Everett profile/'quoted'", "custom 'provider'");
        assert_eq!(
            prefix,
            "EVERETT_HOME='/tmp/Everett profile/'\\''quoted'\\''' EVERETT_AUTH_PROVIDER='custom '\\''provider'\\''' "
        );
        assert!(!prefix.contains(" HOME="));
        assert!(!prefix.contains("TOKEN"));
    }

}
