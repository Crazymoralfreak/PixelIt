//! The MCP servers the assistant may use, stored in `mcp.json`.
//!
//! Environment variables of a server often hold access tokens, so they are
//! kept in the Windows Credential Manager and never reach the file.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use pixelit_ai::secrets;
use pixelit_core::{Error, Result};
use pixelit_engine::{store, Paths};

pub const MCP_VERSION: u32 = 1;

/// Longest tool name a model accepts.
const MAX_TOOL_NAME: usize = 64;

const MAX_ID: usize = 24;

/// What the assistant may do with a tool of an external server.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Permission {
    /// Run without asking.
    Allow,
    /// Ask the user before every call.
    #[default]
    Ask,
    /// Hide the tool from the assistant.
    Deny,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TransportKind {
    /// A program started by PixelIt, talking over its standard streams.
    #[default]
    Stdio,
    /// A server reached over HTTP.
    Http,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ServerConfig {
    /// Short unique name, also the prefix of the server's tools.
    pub id: String,
    pub name: String,
    pub enabled: bool,
    pub transport: TransportKind,
    /// Program of a `stdio` server.
    pub command: String,
    pub args: Vec<String>,
    /// Environment variables of a `stdio` server.
    pub env: BTreeMap<String, String>,
    /// Address of an `http` server.
    pub url: String,
    /// Permission of tools not listed in `tools`.
    pub default_permission: Permission,
    pub tools: BTreeMap<String, Permission>,
}

/// Lower-case letters, digits and hyphens of a name.
fn slug(name: &str) -> String {
    let mut slug = String::new();
    for character in name.trim().chars() {
        if character.is_ascii_alphanumeric() {
            slug.push(character.to_ascii_lowercase());
        } else if !slug.is_empty() && !slug.ends_with('-') {
            slug.push('-');
        }
        if slug.len() >= MAX_ID {
            break;
        }
    }
    let slug = slug.trim_matches('-');
    if slug.is_empty() {
        String::from("server")
    } else {
        slug.to_owned()
    }
}

impl ServerConfig {
    pub fn permission(&self, tool: &str) -> Permission {
        self.tools
            .get(tool)
            .copied()
            .unwrap_or(self.default_permission)
    }

    /// The name a tool of this server has for the model.
    pub fn tool_name(&self, tool: &str) -> String {
        let name: String = format!("{}__{tool}", self.id)
            .chars()
            .map(|character| {
                if character.is_ascii_alphanumeric() || matches!(character, '_' | '-') {
                    character
                } else {
                    '_'
                }
            })
            .collect();
        name.chars().take(MAX_TOOL_NAME).collect()
    }

    fn secret_name(&self) -> String {
        format!("mcp-{}", self.id)
    }

    fn check(&self) -> Result<()> {
        let missing = match self.transport {
            TransportKind::Stdio => self.command.trim().is_empty(),
            TransportKind::Http => self.url.trim().is_empty(),
        };
        if missing {
            Err(Error::new("mcpBadServer").with("name", &self.name))
        } else {
            Ok(())
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct McpConfig {
    pub servers: Vec<ServerConfig>,
}

impl McpConfig {
    /// The servers as stored in the file, without environment variables.
    pub fn load(paths: &Paths) -> Self {
        store::load_or_default(&paths.mcp(), MCP_VERSION, store::no_migration)
    }

    /// The servers with their environment variables, for editing and for
    /// starting them.
    pub fn load_with_secrets(paths: &Paths) -> Self {
        let mut config = Self::load(paths);
        for server in &mut config.servers {
            server.env = secrets::get(&server.secret_name())
                .and_then(|text| serde_json::from_str(&text).ok())
                .unwrap_or_default();
        }
        config
    }

    /// Saves the servers; environment variables go to the Credential Manager.
    pub fn save(&self, paths: &Paths) -> Result<()> {
        let mut plain = self.clone();
        for server in &mut plain.servers {
            let env = std::mem::take(&mut server.env);
            if env.is_empty() {
                secrets::delete(&server.secret_name());
            } else {
                secrets::set(&server.secret_name(), &serde_json::to_string(&env)?)?;
            }
        }
        store::save_json(&paths.mcp(), MCP_VERSION, &plain)
    }

    pub fn get(&self, id: &str) -> Option<&ServerConfig> {
        self.servers.iter().find(|server| server.id == id)
    }

    /// Adds a server or replaces the one with the same id. A new server gets
    /// an id made of its name.
    pub fn upsert(&mut self, mut server: ServerConfig) -> Result<ServerConfig> {
        server.name = server.name.trim().to_owned();
        server.check()?;
        if server.id.is_empty() {
            let base = slug(&server.name);
            let mut id = base.clone();
            let mut number = 2;
            while self.get(&id).is_some() {
                id = format!("{base}-{number}");
                number += 1;
            }
            server.id = id;
        }
        match self.servers.iter_mut().find(|known| known.id == server.id) {
            Some(known) => *known = server.clone(),
            None => self.servers.push(server.clone()),
        }
        Ok(server)
    }

    /// Removes a server with its stored environment variables.
    pub fn remove(&mut self, id: &str) -> bool {
        let Some(index) = self.servers.iter().position(|server| server.id == id) else {
            return false;
        };
        let server = self.servers.remove(index);
        secrets::delete(&server.secret_name());
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn server(name: &str) -> ServerConfig {
        ServerConfig {
            name: name.to_owned(),
            enabled: true,
            command: String::from("npx"),
            ..ServerConfig::default()
        }
    }

    #[test]
    fn ids_are_made_of_names_and_stay_unique() {
        let mut config = McpConfig::default();
        let first = config.upsert(server("File System!")).unwrap();
        let second = config.upsert(server("file system")).unwrap();
        let unnamed = config.upsert(server("Файлы")).unwrap();
        assert_eq!(first.id, "file-system");
        assert_eq!(second.id, "file-system-2");
        assert_eq!(unnamed.id, "server");
        let mut changed = first.clone();
        changed.args = vec![String::from("-y")];
        config.upsert(changed).unwrap();
        assert_eq!(config.servers.len(), 3);
        assert_eq!(config.get("file-system").unwrap().args, ["-y"]);
        assert!(config.remove("file-system-2"));
        assert!(!config.remove("file-system-2"));
    }

    #[test]
    fn incomplete_servers_are_refused() {
        let mut config = McpConfig::default();
        let mut http = server("Remote");
        http.transport = TransportKind::Http;
        assert_eq!(config.upsert(http).unwrap_err().code, "mcpBadServer");
        let mut stdio = server("Local");
        stdio.command = String::from("  ");
        assert_eq!(config.upsert(stdio).unwrap_err().code, "mcpBadServer");
    }

    #[test]
    fn permissions_fall_back_to_the_default() {
        let mut config = server("Files");
        config.id = String::from("files");
        assert_eq!(config.permission("read"), Permission::Ask);
        config.tools.insert(String::from("read"), Permission::Allow);
        config.default_permission = Permission::Deny;
        assert_eq!(config.permission("read"), Permission::Allow);
        assert_eq!(config.permission("write"), Permission::Deny);
        assert_eq!(config.tool_name("read file"), "files__read_file");
        assert_eq!(config.tool_name(&"x".repeat(100)).len(), MAX_TOOL_NAME);
    }

    #[test]
    fn servers_without_variables_round_trip_through_the_file() {
        let folder = tempfile::tempdir().unwrap();
        let paths = Paths::at(folder.path());
        let mut config = McpConfig::default();
        config.upsert(server("Files")).unwrap();
        config.save(&paths).unwrap();
        assert_eq!(McpConfig::load(&paths), config);
        let text = std::fs::read_to_string(paths.mcp()).unwrap();
        assert!(text.contains("\"env\": {}"));
    }
}
