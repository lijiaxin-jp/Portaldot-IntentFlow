//! Typed configuration for Portaldot network access and guarded execution.

use serde::Deserialize;
use std::{env, fs, path::Path};
use thiserror::Error;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PortaldotConfig {
    pub network: NetworkConfig,
    pub contracts: ContractsConfig,
    pub solver: SolverConfig,
    pub execution: ExecutionConfig,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct NetworkConfig {
    pub name: String,
    pub websocket_url_env: String,
    pub ss58_format: u16,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ContractsConfig {
    pub token_a: String,
    pub token_b: String,
    pub pool_a: String,
    pub pool_b: String,
    pub intent_router: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct SolverConfig {
    pub max_hops: usize,
    pub max_price_impact_bps: u32,
    pub max_stale_blocks: u64,
    pub max_failure_risk_bps: u32,
    #[serde(deserialize_with = "deserialize_u128_string")]
    pub min_net_output: u128,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ExecutionConfig {
    pub enabled: bool,
    pub dry_run_required: bool,
    #[serde(deserialize_with = "deserialize_u128_string")]
    pub max_fee: u128,
    pub deadline_blocks: u64,
}

#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum ConfigError {
    #[error("configuration file could not be read")]
    Read,
    #[error("configuration TOML is invalid: {0}")]
    Parse(String),
    #[error("configuration field is invalid: {0}")]
    Invalid(&'static str),
    #[error("required environment variable is missing: {0}")]
    MissingEnvironmentVariable(String),
    #[error("WebSocket endpoint from {0} must use ws:// or wss://")]
    InvalidWebsocketEndpoint(String),
}

impl PortaldotConfig {
    pub fn from_file(path: impl AsRef<Path>) -> Result<Self, ConfigError> {
        let source = fs::read_to_string(path).map_err(|_| ConfigError::Read)?;
        Self::from_toml(&source)
    }

    pub fn from_toml(source: &str) -> Result<Self, ConfigError> {
        let config: Self =
            toml::from_str(source).map_err(|error| ConfigError::Parse(error.to_string()))?;
        config.validate()?;
        Ok(config)
    }

    pub fn websocket_url_from_env(&self) -> Result<String, ConfigError> {
        let value = env::var(&self.network.websocket_url_env).map_err(|_| {
            ConfigError::MissingEnvironmentVariable(self.network.websocket_url_env.clone())
        })?;
        if !is_websocket_endpoint(&value) {
            return Err(ConfigError::InvalidWebsocketEndpoint(
                self.network.websocket_url_env.clone(),
            ));
        }
        Ok(value)
    }

    pub fn validate(&self) -> Result<(), ConfigError> {
        if self.network.name.trim().is_empty() {
            return Err(ConfigError::Invalid("network.name"));
        }
        if self.network.websocket_url_env.trim().is_empty()
            || !self
                .network
                .websocket_url_env
                .chars()
                .all(|character| character.is_ascii_alphanumeric() || character == '_')
        {
            return Err(ConfigError::Invalid("network.websocket_url_env"));
        }
        if self.network.ss58_format > 16_383 {
            return Err(ConfigError::Invalid("network.ss58_format"));
        }
        if !(1..=2).contains(&self.solver.max_hops) {
            return Err(ConfigError::Invalid("solver.max_hops"));
        }
        if self.solver.max_price_impact_bps > 10_000 {
            return Err(ConfigError::Invalid("solver.max_price_impact_bps"));
        }
        if self.solver.max_failure_risk_bps > 10_000 {
            return Err(ConfigError::Invalid("solver.max_failure_risk_bps"));
        }
        if !self.execution.dry_run_required {
            return Err(ConfigError::Invalid("execution.dry_run_required"));
        }
        if self.execution.deadline_blocks == 0 {
            return Err(ConfigError::Invalid("execution.deadline_blocks"));
        }
        Ok(())
    }
}

fn deserialize_u128_string<'de, D>(deserializer: D) -> Result<u128, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = String::deserialize(deserializer)?;
    value.parse().map_err(serde::de::Error::custom)
}

fn is_websocket_endpoint(value: &str) -> bool {
    ["ws://", "wss://"].iter().any(|scheme| {
        value.strip_prefix(scheme).is_some_and(|remainder| {
            let authority = remainder.split(['/', '?', '#']).next().unwrap_or_default();
            !authority.is_empty() && !value.chars().any(char::is_whitespace)
        })
    })
}
