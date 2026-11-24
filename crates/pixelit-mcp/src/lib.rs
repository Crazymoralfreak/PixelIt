//! PixelIt and the Model Context Protocol, in both directions.
//!
//! [`server`] lets external agents use PixelIt (`PixelIt.exe mcp`);
//! [`Hub`] lets the built-in assistant use the tools of external servers.

pub mod client;
pub mod config;
pub mod protocol;
pub mod server;

use std::collections::BTreeMap;
use std::sync::{Arc, PoisonError, RwLock};

use serde::Serialize;
use serde_json::Value;

use pixelit_ai::{Net, ToolSpec};
use pixelit_core::{CancelToken, Error, Result};
use pixelit_engine::Paths;
use pixelit_tools::ToolSource;

pub use crate::client::{Client, RemoteTool};
pub use crate::config::{McpConfig, Permission, ServerConfig, TransportKind};

/// A tool of a connected server with its permission.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolStatus {
    pub name: String,
    pub description: String,
    pub permission: Permission,
}

/// The state of a configured server, for the interface.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerStatus {
    pub id: String,
    pub connected: bool,
    /// Name and version the server reports about itself.
    pub server: String,
    /// Why the last attempt to connect failed.
    pub error: Option<Error>,
    pub tools: Vec<ToolStatus>,
}

/// The connections to external MCP servers.
pub struct Hub {
    paths: Paths,
    net: Net,
    clients: RwLock<Vec<Arc<Client>>>,
    errors: RwLock<BTreeMap<String, Error>>,
}

impl Hub {
    pub fn new(paths: Paths, net: Net) -> Self {
        Self {
            paths,
            net,
            clients: RwLock::new(Vec::new()),
            errors: RwLock::new(BTreeMap::new()),
        }
    }

    fn clients(&self) -> Vec<Arc<Client>> {
        self.clients
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// Drops the connection to a server; its program is stopped.
    pub fn disconnect(&self, id: &str) {
        self.clients
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .retain(|client| client.id != id);
    }

    /// Connects to a server, replacing an earlier connection to it.
    pub fn connect(
        &self,
        server: &ServerConfig,
        allow_remote: bool,
        cancel: &CancelToken,
    ) -> Result<()> {
        self.disconnect(&server.id);
        let log = self.paths.logs().join(format!("mcp-{}.log", server.id));
        let outcome = Client::connect(server, &self.net, allow_remote, Some(&log), cancel);
        let mut errors = self.errors.write().unwrap_or_else(PoisonError::into_inner);
        match outcome {
            Ok(client) => {
                errors.remove(&server.id);
                tracing::info!(server = %server.id, tools = client.tools.len(), "MCP server connected");
                self.clients
                    .write()
                    .unwrap_or_else(PoisonError::into_inner)
                    .push(Arc::new(client));
                Ok(())
            }
            Err(error) => {
                tracing::warn!(server = %server.id, "MCP server failed: {error}");
                errors.insert(server.id.clone(), error.clone());
                Err(error)
            }
        }
    }

    /// Brings the connections in line with the configuration: enabled servers
    /// are connected, the rest is dropped. Failures are kept for [`Hub::status`].
    pub fn sync(&self, allow_remote: bool, cancel: &CancelToken) {
        let config = McpConfig::load_with_secrets(&self.paths);
        let connected: Vec<String> = self
            .clients()
            .iter()
            .map(|client| client.id.clone())
            .collect();
        for id in &connected {
            if !config.get(id).is_some_and(|server| server.enabled) {
                self.disconnect(id);
            }
        }
        self.errors
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .retain(|id, _| config.get(id).is_some_and(|server| server.enabled));
        for server in config.servers.iter().filter(|server| server.enabled) {
            if cancel.is_cancelled() {
                break;
            }
            if !connected.contains(&server.id) {
                let _ = self.connect(server, allow_remote, cancel);
            }
        }
    }

    pub fn status(&self) -> Vec<ServerStatus> {
        let config = McpConfig::load(&self.paths);
        let clients = self.clients();
        let errors = self.errors.read().unwrap_or_else(PoisonError::into_inner);
        config
            .servers
            .iter()
            .map(|server| {
                let client = clients.iter().find(|client| client.id == server.id);
                ServerStatus {
                    id: server.id.clone(),
                    connected: client.is_some(),
                    server: client.map_or_default(|client| client.server.clone()),
                    error: errors.get(&server.id).cloned(),
                    tools: client.map_or_default(|client| {
                        client
                            .tools
                            .iter()
                            .map(|tool| ToolStatus {
                                name: tool.name.clone(),
                                description: tool.description.clone(),
                                permission: server.permission(&tool.name),
                            })
                            .collect()
                    }),
                }
            })
            .collect()
    }

    /// The connection, the tool and its permission behind a model-facing name.
    fn resolve(&self, name: &str) -> Option<(Arc<Client>, String, Permission)> {
        let config = McpConfig::load(&self.paths);
        for client in self.clients() {
            let Some(server) = config.get(&client.id) else {
                continue;
            };
            if let Some(tool) = client
                .tools
                .iter()
                .find(|tool| server.tool_name(&tool.name) == name)
            {
                let permission = server.permission(&tool.name);
                return Some((Arc::clone(&client), tool.name.clone(), permission));
            }
        }
        None
    }
}

impl ToolSource for Hub {
    fn specs(&self) -> Vec<ToolSpec> {
        let config = McpConfig::load(&self.paths);
        let mut specs = Vec::new();
        for client in self.clients() {
            let Some(server) = config.get(&client.id).filter(|server| server.enabled) else {
                continue;
            };
            for tool in &client.tools {
                if server.permission(&tool.name) == Permission::Deny {
                    continue;
                }
                specs.push(ToolSpec {
                    name: server.tool_name(&tool.name),
                    description: format!("[{}] {}", server.name, tool.description),
                    parameters: tool.schema.clone(),
                });
            }
        }
        specs
    }

    fn needs_approval(&self, name: &str) -> bool {
        self.resolve(name)
            .is_none_or(|(_, _, permission)| permission != Permission::Allow)
    }

    fn call(&self, name: &str, arguments: &Value, cancel: &CancelToken) -> Result<Value> {
        let (client, tool, permission) = self
            .resolve(name)
            .ok_or_else(|| Error::new("unknownTool").with("name", name))?;
        if permission == Permission::Deny {
            return Err(Error::new("toolDeclined").with("name", name));
        }
        client.call_tool(&tool, arguments, cancel)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_failed_connection_is_remembered() {
        let folder = tempfile::tempdir().unwrap();
        let paths = Paths::at(folder.path());
        let mut config = McpConfig::default();
        let server = config
            .upsert(ServerConfig {
                name: String::from("Ghost"),
                enabled: true,
                command: String::from("pixelit-no-such-program"),
                ..ServerConfig::default()
            })
            .unwrap();
        config.save(&paths).unwrap();
        let hub = Hub::new(paths, Net::new());
        hub.sync(false, &CancelToken::new());
        let status = hub.status();
        assert_eq!(status.len(), 1);
        assert_eq!(status[0].id, server.id);
        assert!(!status[0].connected);
        assert_eq!(status[0].error.as_ref().unwrap().code, "mcpStartFailed");
        assert_eq!(hub.specs(), [] as [pixelit_ai::ToolSpec; 0]);
        assert!(hub.needs_approval("ghost__anything"));
        let error = hub
            .call("ghost__anything", &Value::Null, &CancelToken::new())
            .unwrap_err();
        assert_eq!(error.code, "unknownTool");
    }
}
