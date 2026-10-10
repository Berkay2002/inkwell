//! Finding the user's Codex CLI and starting `codex app-server` lean (no MCP servers, no skills,
//! no tools): the prompt shrinks from ~17k to ~7k tokens and nothing from the user's own setup
//! (notify hook, memories, MCP servers) fires for a PDF question.

use super::rpc::SpawnSpec;
use std::path::PathBuf;

/// codex 0.159 `-c key=value` overrides (same list Red Pen proved out). Unknown keys are ignored,
/// so this survives CLI updates.
const LEAN: &[&str] = &[
    "project_doc_max_bytes=0",
    "skills.include_instructions=false",
    "include_permissions_instructions=false",
    "include_apps_instructions=false",
    "include_collaboration_mode_instructions=false",
    "include_environment_context=false",
    "web_search=\"disabled\"",
    "history.persistence=\"none\"",
    "check_for_update_on_startup=false",
    "analytics.enabled=false",
    "notify=[]",
    "features.memories=false",
    "features.guardian_approval=false",
    "features.prevent_idle_sleep=false",
];

const OFF_FEATURES: &[&str] = &[
    "apps",
    "plugins",
    "remote_plugin",
    "hooks",
    "goals",
    "multi_agent",
    "skill_search",
    "tool_suggest",
    "browser_use",
    "browser_use_external",
    "computer_use",
    "image_generation",
    "sleep_tool",
    "view_image",
    "shell_snapshot",
    "workspace_dependencies",
    "worktrees",
    "code_mode_host",
    "realtime_conversation",
    "shell_tool",
    "unified_exec",
];

pub fn codex_home() -> PathBuf {
    match std::env::var_os("CODEX_HOME") {
        Some(h) if !h.is_empty() => PathBuf::from(h),
        _ => std::env::var_os("USERPROFILE")
            .or_else(|| std::env::var_os("HOME"))
            .map(|h| PathBuf::from(h).join(".codex"))
            .unwrap_or_else(|| PathBuf::from(".codex")),
    }
}

/// `INKWELL_CODEX`, then the per-user Windows install, then `codex.exe` on PATH.
pub fn resolve_binary() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("INKWELL_CODEX").map(PathBuf::from) {
        if p.is_file() {
            return Some(p);
        }
    }
    if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        let p = PathBuf::from(local).join(r"Programs\OpenAI\Codex\bin\codex.exe");
        if p.is_file() {
            return Some(p);
        }
    }
    let name = if cfg!(windows) { "codex.exe" } else { "codex" };
    std::env::var_os("PATH").and_then(|path| std::env::split_paths(&path).map(|d| d.join(name)).find(|p| p.is_file()))
}

/// Names of the servers declared as `[mcp_servers.NAME]` (or a sub-table of it) in config.toml.
/// `-c` can't empty a table, so each one is switched off by name.
pub fn mcp_server_names(config: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for line in config.lines() {
        let Some(rest) = line.trim().strip_prefix('[') else { continue };
        let Some(rest) = rest.strip_prefix("mcp_servers.") else { continue };
        let name = if let Some(q) = rest.strip_prefix('"') {
            match q.find('"') {
                Some(end) => format!("\"{}\"", &q[..end]),
                None => continue,
            }
        } else {
            let end = rest.find(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '-')).unwrap_or(rest.len());
            if end == 0 {
                continue;
            }
            rest[..end].to_string()
        };
        if !out.contains(&name) {
            out.push(name);
        }
    }
    out
}

pub fn server_args(config: &str) -> Vec<String> {
    let mut a = vec!["app-server".to_string()];
    let mut c = |v: String| {
        a.push("-c".into());
        a.push(v);
    };
    for o in LEAN {
        c((*o).to_string());
    }
    for f in OFF_FEATURES {
        c(format!("features.{f}=false"));
    }
    for n in mcp_server_names(config) {
        c(format!("mcp_servers.{n}.enabled=false"));
    }
    a
}

pub fn spawn_spec() -> Option<SpawnSpec> {
    let program = resolve_binary()?;
    let config = std::fs::read_to_string(codex_home().join("config.toml")).unwrap_or_default();
    Some(SpawnSpec { program, args: server_args(&config), cwd: std::env::temp_dir() })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_mcp_servers_including_quoted_and_subtables() {
        let cfg = r#"
model = "x"
[mcp_servers.github]
command = "npx"
[mcp_servers.github.env]
TOKEN = "a"
  [mcp_servers."my server"]
url = "http://x"
[mcp_servers.fs-tools_2]
[projects.'C:\a']
"#;
        assert_eq!(mcp_server_names(cfg), ["github", "\"my server\"", "fs-tools_2"]);
    }

    #[test]
    fn args_start_with_app_server_and_disable_every_server() {
        let a = server_args("[mcp_servers.a]\n[mcp_servers.b]\n");
        assert_eq!(a[0], "app-server");
        assert!(a.windows(2).any(|w| w[0] == "-c" && w[1] == "mcp_servers.a.enabled=false"));
        assert!(a.windows(2).any(|w| w[0] == "-c" && w[1] == "mcp_servers.b.enabled=false"));
        assert!(a.windows(2).any(|w| w[0] == "-c" && w[1] == "features.shell_tool=false"));
        assert!(a.windows(2).any(|w| w[0] == "-c" && w[1] == "notify=[]"));
    }
}
