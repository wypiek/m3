//! `workspaces.toml` — workspace count, names and layouts.

use super::{ConfigFile, check_range};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum LayoutKind {
    #[default]
    Floating,
    /// Master/stack tiling.
    Tiling,
    /// One maximized window at a time.
    Monocle,
}

impl LayoutKind {
    pub const ALL: [LayoutKind; 3] = [LayoutKind::Floating, LayoutKind::Tiling, LayoutKind::Monocle];

    pub fn name(self) -> &'static str {
        match self {
            LayoutKind::Floating => "floating",
            LayoutKind::Tiling => "tiling",
            LayoutKind::Monocle => "monocle",
        }
    }

    pub fn from_name(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|l| l.name() == s)
    }

    pub fn next(self) -> Self {
        match self {
            LayoutKind::Floating => LayoutKind::Tiling,
            LayoutKind::Tiling => LayoutKind::Monocle,
            LayoutKind::Monocle => LayoutKind::Floating,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct WorkspacesConfig {
    pub count: usize,
    /// Optional names; missing entries are shown as their number.
    pub names: Vec<String>,
    pub default_layout: LayoutKind,
    /// Each output has its own independent set of workspaces.
    pub per_output: bool,
    /// Moving past the last workspace wraps to the first.
    pub wrap_around: bool,
}

impl Default for WorkspacesConfig {
    fn default() -> Self {
        Self {
            count: 4,
            names: Vec::new(),
            default_layout: LayoutKind::Floating,
            per_output: false,
            wrap_around: false,
        }
    }
}

impl WorkspacesConfig {
    pub fn name_of(&self, index: usize) -> String {
        self.names
            .get(index.wrapping_sub(1))
            .filter(|n| !n.trim().is_empty())
            .cloned()
            .unwrap_or_else(|| index.to_string())
    }
}

impl ConfigFile for WorkspacesConfig {
    const FILE_NAME: &'static str = "workspaces.toml";
    const SCHEMA_VERSION: u32 = 1;

    fn validate(&self) -> Vec<String> {
        let mut p = Vec::new();
        check_range(&mut p, "count", self.count, 1, 32);
        if self.names.len() > self.count {
            p.push(format!("`names` has {} entries but there are only {} workspaces", self.names.len(), self.count));
        }
        p
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn names() {
        let c = WorkspacesConfig { names: vec!["Web".into(), " ".into()], ..Default::default() };
        assert_eq!(c.name_of(1), "Web");
        assert_eq!(c.name_of(2), "2");
        assert_eq!(c.name_of(3), "3");
    }
}
