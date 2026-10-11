//! Finding the user's own `claude` and deciding what its environment may contain.
//!
//! Inkwell never touches Claude credentials. The child is the user's installed, unmodified Claude
//! Code, signed in the way the user signed in (their subscription, through `claude login`). Two
//! rules keep it that way and both live here, in pure functions the tests can drive:
//!
//! * a variable that would make Claude Code authenticate or route some other way (an API key, an
//!   auth token, another base URL, a pasted OAuth token, a cloud provider switch) is never passed
//!   on, and when the user's own environment sets one, Claude is refused with a message that names
//!   it: the answer would not be billed to their Claude plan;
//! * every other `CLAUDE*` / `ANTHROPIC*` variable is dropped too (an Inkwell started from a Claude
//!   Code terminal carries the session ids and the message socket of that session), except the
//!   config directory the user chose and the Git Bash path Claude Code needs on Windows.

use std::path::{Path, PathBuf};

/// Variables that carry or redirect authentication. Their presence refuses the provider.
const FORCING: &[&str] = &[
    "ANTHROPIC_API_KEY",
    "ANTHROPIC_AUTH_TOKEN",
    "ANTHROPIC_BASE_URL",
    "ANTHROPIC_FOUNDRY_API_KEY",
    "CLAUDE_CODE_OAUTH_TOKEN",
];

/// Switches to another backend (Bedrock, Vertex, Foundry); only a truthy value forces.
const BACKEND_SWITCHES: &[&str] = &["CLAUDE_CODE_USE_BEDROCK", "CLAUDE_CODE_USE_VERTEX", "CLAUDE_CODE_USE_FOUNDRY"];

/// What survives the strip.
const KEEP: &[&str] = &["CLAUDE_CONFIG_DIR", "CLAUDE_CODE_GIT_BASH_PATH"];

fn is_claudeish(name: &str) -> bool {
    let n = name.to_ascii_uppercase();
    n.starts_with("CLAUDE") || n.starts_with("ANTHROPIC")
}

fn truthy(v: &str) -> bool {
    let v = v.trim().to_ascii_lowercase();
    !(v.is_empty() || v == "0" || v == "false" || v == "no" || v == "off")
}

/// The names (never the values) of the variables in `vars` that force another authentication or
/// backend. Empty when the environment leaves the user's Claude login in charge.
pub fn forcing(vars: &[(String, String)]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for (k, v) in vars {
        let up = k.to_ascii_uppercase();
        let hit = (FORCING.contains(&up.as_str()) && !v.trim().is_empty())
            || (BACKEND_SWITCHES.contains(&up.as_str()) && truthy(v));
        if hit && !out.contains(&up) {
            out.push(up);
        }
    }
    out.sort();
    out
}

/// The sentence shown when `forcing` found something.
pub fn refusal(names: &[String]) -> String {
    format!(
        "Your environment sets {}, which would make Claude Code use that instead of your Claude plan. \
         Inkwell uses only the plan you sign in to with `claude login`, so it will not start Claude while \
         that is set. Remove it (or start Inkwell without it) and try again.",
        names.join(", ")
    )
}

/// The environment the child gets: everything of `vars` except what is claudeish, minus the forcing
/// names above, plus the keep-list.
pub fn child_env(vars: &[(String, String)]) -> Vec<(String, String)> {
    vars.iter()
        .filter(|(k, _)| {
            let up = k.to_ascii_uppercase();
            !is_claudeish(k) || KEEP.contains(&up.as_str())
        })
        .cloned()
        .collect()
}

// ---- finding the executable --------------------------------------------------------------------------

/// Where the CLI might be, best first. `override_path` is the user's own choice (a setting or
/// `INKWELL_CLAUDE`), `path` the PATH directories, `home` the user profile folder.
pub fn candidates(override_path: Option<&Path>, path: &[PathBuf], home: Option<&Path>, appdata: Option<&Path>) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = Vec::new();
    if let Some(p) = override_path {
        v.push(p.to_path_buf());
    }
    let names: &[&str] = if cfg!(windows) { &["claude.exe", "claude.cmd"] } else { &["claude"] };
    for dir in path {
        for n in names {
            v.push(dir.join(n));
        }
    }
    if let Some(h) = home {
        for n in names {
            v.push(h.join(".local").join("bin").join(n));
        }
    }
    if let Some(a) = appdata {
        // the npm global bin folder of a Windows install
        v.push(a.join("npm").join("claude.cmd"));
    }
    v
}

/// An npm shim (`claude.cmd`) next to `node_modules/@anthropic-ai/claude-code`: the real binary
/// the shim starts. Batch files are never run directly (they need a shell and its quoting rules).
pub fn real_exe_behind_shim(shim: &Path) -> Option<PathBuf> {
    let dir = shim.parent()?;
    let pkg = dir.join("node_modules").join("@anthropic-ai").join("claude-code");
    for rel in [&["bin", "claude.exe"][..], &["claude.exe"][..]] {
        let mut p = pkg.clone();
        for r in rel {
            p.push(r);
        }
        if p.is_file() {
            return Some(p);
        }
    }
    None
}

fn is_shim(p: &Path) -> bool {
    matches!(
        p.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase).as_deref(),
        Some("cmd" | "bat" | "ps1")
    )
}

/// The first usable executable among `list`, shims resolved to the binary they start.
pub fn first_usable(list: &[PathBuf]) -> Option<PathBuf> {
    for c in list {
        if !c.is_file() {
            continue;
        }
        if is_shim(c) {
            if let Some(real) = real_exe_behind_shim(c) {
                return Some(real);
            }
            continue;
        }
        return Some(c.clone());
    }
    None
}

/// Resolves from the real environment.
pub fn resolve(override_path: Option<&Path>) -> Option<PathBuf> {
    let path: Vec<PathBuf> = std::env::var_os("PATH").map(|p| std::env::split_paths(&p).collect()).unwrap_or_default();
    let home = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")).map(PathBuf::from);
    let appdata = std::env::var_os("APPDATA").map(PathBuf::from);
    let env_override = std::env::var_os("INKWELL_CLAUDE").map(PathBuf::from);
    let chosen = override_path.map(Path::to_path_buf).or(env_override);
    first_usable(&candidates(chosen.as_deref(), &path, home.as_deref(), appdata.as_deref()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vars(v: &[(&str, &str)]) -> Vec<(String, String)> {
        v.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    #[test]
    fn auth_forcing_variables_are_found_by_name_only() {
        let v = vars(&[
            ("PATH", "C:\\x"),
            ("anthropic_api_key", "sk-secret"),
            ("ANTHROPIC_BASE_URL", "https://proxy.example"),
            ("CLAUDE_CODE_OAUTH_TOKEN", "tok"),
            ("CLAUDE_CODE_USE_BEDROCK", "1"),
            ("CLAUDE_CODE_USE_VERTEX", "0"),
            ("ANTHROPIC_AUTH_TOKEN", ""),
        ]);
        let f = forcing(&v);
        assert_eq!(f, ["ANTHROPIC_API_KEY", "ANTHROPIC_BASE_URL", "CLAUDE_CODE_OAUTH_TOKEN", "CLAUDE_CODE_USE_BEDROCK"]);
        let msg = refusal(&f);
        assert!(msg.contains("ANTHROPIC_API_KEY") && !msg.contains("sk-secret"), "{msg}");
        assert!(forcing(&vars(&[("PATH", "x"), ("CLAUDE_CONFIG_DIR", "D:\\cfg")])).is_empty());
    }

    #[test]
    fn the_child_environment_has_no_claude_session_or_credential_variables() {
        let v = vars(&[
            ("PATH", "C:\\x"),
            ("USERPROFILE", "C:\\Users\\a"),
            ("ANTHROPIC_API_KEY", "sk"),
            ("ANTHROPIC_BASE_URL", "u"),
            ("CLAUDECODE", "1"),
            ("CLAUDE_CODE_SESSION_ID", "s"),
            ("CLAUDE_CODE_MESSAGING_TOKEN", "t"),
            ("CLAUDE_EFFORT", "high"),
            ("Claude_Config_Dir", "D:\\cfg"),
            ("CLAUDE_CODE_GIT_BASH_PATH", "C:\\git\\bash.exe"),
        ]);
        let e = child_env(&v);
        let names: Vec<&str> = e.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(names, ["PATH", "USERPROFILE", "Claude_Config_Dir", "CLAUDE_CODE_GIT_BASH_PATH"]);
    }

    #[test]
    fn candidates_come_in_the_order_override_path_home_npm() {
        let c = candidates(
            Some(Path::new("D:/mine/claude.exe")),
            &[PathBuf::from("C:/bin")],
            Some(Path::new("C:/Users/a")),
            Some(Path::new("C:/Users/a/AppData/Roaming")),
        );
        assert_eq!(c[0], Path::new("D:/mine/claude.exe"));
        assert!(c[1].starts_with("C:/bin"));
        assert!(c.iter().any(|p| p.ends_with(".local/bin/claude.exe") || p.ends_with(".local\\bin\\claude.exe") || p.ends_with(".local/bin/claude")));
        assert!(c.last().unwrap().to_string_lossy().contains("npm"));
    }

    #[test]
    fn an_npm_shim_resolves_to_the_binary_it_starts_and_a_bare_shim_is_skipped() {
        let root = std::env::temp_dir().join(format!("inkwell-claude-shim-{}", std::process::id()));
        let bin = root.join("node_modules").join("@anthropic-ai").join("claude-code").join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::write(root.join("claude.cmd"), "@echo off").unwrap();
        // no exe yet: the shim alone is not usable
        assert_eq!(first_usable(&[root.join("claude.cmd")]), None);
        std::fs::write(bin.join("claude.exe"), "MZ").unwrap();
        assert_eq!(first_usable(&[root.join("claude.cmd")]), Some(bin.join("claude.exe")));
        let _ = std::fs::remove_dir_all(&root);
    }
}
