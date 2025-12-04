use nativelink_error::{Error, ResultExt};

use crate::config::{WarmupCommand, WarmupConfig};
use crate::cri_client::CriClient;

/// Runs warmup routines and cleanup hooks for a worker container.
#[derive(Debug, Clone)]
pub struct WarmupController {
    config: WarmupConfig,
}

impl WarmupController {
    #[must_use]
    pub const fn new(config: WarmupConfig) -> Self {
        Self { config }
    }

    /// Executes the warmup and verification commands before a worker is marked ready.
    pub async fn run_full_warmup(&self, cri: &CriClient, container_id: &str) -> Result<(), Error> {
        self.run_command_group("warmup", cri, container_id, &self.config.commands)
            .await?;
        self.run_command_group("verification", cri, container_id, &self.config.verification)
            .await
    }

    /// Runs cleanup commands after a job is released.
    pub async fn post_job_cleanup(&self, cri: &CriClient, container_id: &str) -> Result<(), Error> {
        self.run_command_group(
            "post_job_cleanup",
            cri,
            container_id,
            &self.config.post_job_cleanup,
        )
        .await
    }

    async fn run_command_group(
        &self,
        label: &str,
        cri: &CriClient,
        container_id: &str,
        commands: &[WarmupCommand],
    ) -> Result<(), Error> {
        for (index, command) in commands.iter().enumerate() {
            let argv = render_command(command);
            let timeout = command.timeout(self.config.default_timeout_s);
            tracing::debug!(
                command_index = index,
                label,
                ?argv,
                timeout = timeout.as_secs(),
                container_id,
                "executing warmup command",
            );
            cri.exec(container_id, argv, timeout)
                .await
                .err_tip(|| format!("while running warmup {label} command #{index}"))?;
        }
        Ok(())
    }
}

/// Builds the command that should be executed inside the container.
pub(crate) fn render_command(command: &WarmupCommand) -> Vec<String> {
    if command.working_directory.is_some() {
        let mut script = String::new();
        if let Some(dir) = &command.working_directory {
            script.push_str("cd ");
            script.push_str(&shell_escape(dir));
            script.push_str(" && ");
        }
        for (key, value) in &command.env {
            script.push_str(key);
            script.push('=');
            script.push_str(&shell_escape(value));
            script.push(' ');
        }
        script.push_str(
            &command
                .argv
                .iter()
                .map(|arg| shell_escape(arg))
                .collect::<Vec<_>>()
                .join(" "),
        );
        return vec!["/bin/sh".to_string(), "-c".to_string(), script];
    }

    if command.env.is_empty() {
        return command.argv.clone();
    }

    let mut rendered = vec!["/usr/bin/env".to_string()];
    for (key, value) in &command.env {
        rendered.push(format!("{key}={value}"));
    }
    rendered.extend(command.argv.clone());
    rendered
}

fn shell_escape(segment: &str) -> String {
    if segment.is_empty() {
        return "''".to_string();
    }
    let mut escaped = String::with_capacity(segment.len() + 2);
    escaped.push('\'');
    for ch in segment.chars() {
        if ch == '\'' {
            escaped.push_str("'\\''");
        } else {
            escaped.push(ch);
        }
    }
    escaped.push('\'');
    escaped
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    #[test]
    fn render_command_includes_workdir_and_env() {
        let mut env = HashMap::new();
        env.insert("FOO".to_string(), "bar baz".to_string());
        let command = WarmupCommand {
            argv: vec!["echo".to_string(), "ready".to_string()],
            env,
            working_directory: Some("/tmp/warm".to_string()),
            timeout_s: Some(5),
        };

        let rendered = render_command(&command);
        assert_eq!(rendered[0], "/bin/sh");
        assert_eq!(rendered[1], "-c");
        let script = rendered.last().unwrap();
        assert!(
            script.contains("cd '/tmp/warm'"),
            "script missing working dir: {script}"
        );
        assert!(
            script.contains("FOO='bar baz'"),
            "script missing env assignment: {script}"
        );
        assert!(
            script.ends_with("'echo' 'ready'"),
            "script missing argv: {script}"
        );
    }

    #[test]
    fn render_command_without_env_returns_raw_args() {
        let command = WarmupCommand {
            argv: vec!["bazel".to_string(), "info".to_string()],
            env: HashMap::new(),
            working_directory: None,
            timeout_s: None,
        };

        let rendered = render_command(&command);
        assert_eq!(rendered, command.argv);
    }
}
