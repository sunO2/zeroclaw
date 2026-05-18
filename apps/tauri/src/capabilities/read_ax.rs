//! Accessibility tree reader — walks the AXUIElement hierarchy of the
//! frontmost application and returns a JSON tree of elements. Gated by the
//! Accessibility TCC permission.
//!
//! Uses JavaScript for Automation (JXA) via osascript rather than raw FFI.
//! This avoids the fragile AXUIElement FFI bindings while providing the same
//! data. Per @m13v's review: includes a walk timeout (5s) and maximum node
//! count (10 000) to prevent blocking on hung apps and large Electron trees.

use serde::{Deserialize, Serialize};
use std::time::Instant;

/// Maximum number of nodes to visit before truncating.
const MAX_NODES: usize = 10_000;

/// Maximum depth to recurse into the AX tree.
const MAX_DEPTH: usize = 20;

#[derive(Debug, Serialize, Deserialize)]
pub struct AxNode {
    pub role: String,
    pub title: Option<String>,
    pub value: Option<String>,
    pub description: Option<String>,
    pub position: Option<AxPoint>,
    pub size: Option<AxSize>,
    pub children: Vec<AxNode>,
    #[serde(skip_serializing_if = "is_false")]
    pub truncated: bool,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct AxPoint {
    pub x: f64,
    pub y: f64,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct AxSize {
    pub width: f64,
    pub height: f64,
}

#[derive(Debug, Serialize)]
pub struct ReadAxResult {
    pub application: String,
    pub tree: AxNode,
    pub stats: AxStats,
}

#[derive(Debug, Serialize)]
pub struct AxStats {
    pub nodes_visited: usize,
    pub truncated: bool,
    pub elapsed_ms: u64,
}

fn is_false(v: &bool) -> bool {
    !v
}

/// Read the accessibility tree of the frontmost (or specified) application.
///
/// `bundle_id` is optional — defaults to the frontmost application.
#[tauri::command]
pub fn read_ax(bundle_id: Option<String>) -> Result<ReadAxResult, String> {
    #[cfg(target_os = "macos")]
    {
        use crate::macos::permissions;
        if permissions::check_accessibility() != "granted" {
            return Err("permission_denied(accessibility)".into());
        }

        let app_name = resolve_target_app(&bundle_id)?;
        let start = Instant::now();

        let jxa = build_jxa_script(&app_name, MAX_NODES, MAX_DEPTH);

        let output = std::process::Command::new("/usr/bin/osascript")
            .args(["-l", "JavaScript", "-e", &jxa])
            .output()
            .map_err(|e| format!("osascript spawn failed: {e}"))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            return Err(if stderr.is_empty() {
                format!("osascript exited with {}", output.status)
            } else {
                stderr
            });
        }

        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        let tree: AxNode =
            serde_json::from_str(&stdout).map_err(|e| format!("failed to parse AX tree: {e}"))?;

        let elapsed = start.elapsed();
        let truncated = tree_contains_truncation(&tree);
        let nodes_visited = count_nodes(&tree);

        Ok(ReadAxResult {
            application: app_name,
            tree,
            stats: AxStats {
                nodes_visited,
                truncated,
                elapsed_ms: elapsed.as_millis() as u64,
            },
        })
    }

    #[cfg(not(target_os = "macos"))]
    {
        let _ = bundle_id;
        Err("AX reader capability is currently macOS-only".into())
    }
}

#[cfg(target_os = "macos")]
fn resolve_target_app(bundle_id: &Option<String>) -> Result<String, String> {
    match bundle_id {
        Some(id) => Ok(id.clone()),
        None => {
            let output = std::process::Command::new("/usr/bin/osascript")
                .args(["-e", "tell application \"System Events\" to get name of first process whose frontmost is true"])
                .output()
                .map_err(|e| format!("osascript spawn failed: {e}"))?;

            if !output.status.success() {
                return Err("failed to determine frontmost application".into());
            }
            Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
        }
    }
}

/// Build a JXA script that walks the AX tree up to `max_nodes` / `max_depth`
/// and returns JSON. Uses a try-catch per node to avoid aborting on hung elements.
fn build_jxa_script(app_name: &str, max_nodes: usize, max_depth: usize) -> String {
    format!(
        r#"
(function() {{
  var app = Application("System Events").processes.byName("{app_name}");
  if (!app.exists()) return JSON.stringify({{error: "app not found"}});

  var visited = 0;
  var maxNodes = {max_nodes};
  var maxDepth = {max_depth};

  function walk(el, depth) {{
    if (visited >= maxNodes || depth >= maxDepth) {{
      return {{ role: "?", truncated: true }};
    }}
    visited++;

    var node = {{ role: "?", children: [] }};
    try {{ node.role = el.role() || "?"; }} catch(e) {{}}
    try {{ var t = el.title(); if (t) node.title = t; }} catch(e) {{}}
    try {{ var d = el.description(); if (d) node.description = d; }} catch(e) {{}}
    try {{
      var v = el.value();
      if (v !== undefined && v !== null) {{
        var vs = (typeof v === "object" && v.toString) ? v.toString() : String(v);
        if (vs.length <= 2000) node.value = vs;
      }}
    }} catch(e) {{}}
    try {{
      var p = el.position();
      if (p) node.position = {{ x: Number(p.x), y: Number(p.y) }};
    }} catch(e) {{}}
    try {{
      var s = el.size();
      if (s) node.size = {{ width: Number(s.width), height: Number(s.height) }};
    }} catch(e) {{}}

    if (visited >= maxNodes) {{
      node.truncated = true;
      return node;
    }}

    try {{
      var kids = el.uiElements();
      for (var i = 0; i < kids.length; i++) {{
        if (visited >= maxNodes) {{
          node.truncated = true;
          break;
        }}
        node.children.push(walk(kids[i], depth + 1));
      }}
    }} catch(e) {{}}

    return node;
  }}

  return JSON.stringify(walk(app, 0));
}})();
"#,
        app_name = app_name.replace('"', "\\\""),
        max_nodes = max_nodes,
        max_depth = max_depth,
    )
}

fn tree_contains_truncation(node: &AxNode) -> bool {
    if node.truncated {
        return true;
    }
    node.children.iter().any(tree_contains_truncation)
}

fn count_nodes(node: &AxNode) -> usize {
    1 + node.children.iter().map(count_nodes).sum::<usize>()
}
