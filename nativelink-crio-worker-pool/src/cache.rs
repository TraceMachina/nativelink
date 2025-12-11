use nativelink_error::{Error, ResultExt};

use crate::config::CachePrimingConfig;
use crate::cri_client::CriClient;
use crate::warmup::render_command;

/// Hydrates remote execution and remote cache artifacts inside the worker.
#[derive(Debug, Clone)]
pub struct CachePrimingAgent {
    config: CachePrimingConfig,
}

impl CachePrimingAgent {
    #[must_use]
    pub const fn new(config: CachePrimingConfig) -> Self {
        Self { config }
    }

    pub const fn is_enabled(&self) -> bool {
        self.config.enabled
    }

    pub async fn prime(&self, cri: &CriClient, container_id: &str) -> Result<(), Error> {
        if !self.config.enabled {
            return Ok(());
        }
        for (idx, command) in self.config.commands.iter().enumerate() {
            let argv = render_command(command);
            let timeout = command.timeout(60);
            tracing::debug!(
                command_index = idx,
                timeout = timeout.as_secs(),
                ?argv,
                container_id,
                "executing cache priming command",
            );
            cri.exec(container_id, argv, timeout)
                .await
                .err_tip(|| format!("while running cache priming command #{idx}"))?;
        }
        Ok(())
    }
}
