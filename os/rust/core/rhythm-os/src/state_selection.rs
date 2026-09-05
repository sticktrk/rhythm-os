//! Explicit state projections. An absent query retains the legacy full snapshot.

use serde::Serialize;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StateNodes {
    None,
    Controls,
    All,
}

#[derive(Clone, Debug, Serialize)]
pub struct StateSelection {
    pub schema_version: u8,
    pub included: Vec<&'static str>,
    pub nodes: StateNodes,
}

impl StateSelection {
    /// `None` means legacy. An explicit empty value is the base projection.
    pub fn parse(include: Option<&str>) -> Result<Option<Self>, &'static str> {
        let Some(include) = include else {
            return Ok(None);
        };
        if include.len() > 128 {
            return Err("include is too long");
        }
        let mut nodes = StateNodes::None;
        let mut configuration = false;
        for item in include.split(',').map(str::trim).filter(|s| !s.is_empty()) {
            match item {
                "base" => {}
                "controls" if nodes != StateNodes::All => nodes = StateNodes::Controls,
                "nodes" if nodes != StateNodes::Controls => nodes = StateNodes::All,
                "configuration" => configuration = true,
                "controls" | "nodes" => return Err("controls and nodes are mutually exclusive"),
                _ => return Err("unknown state include"),
            }
        }
        let mut included = vec!["base"];
        match nodes {
            StateNodes::None => {}
            StateNodes::Controls => included.push("controls"),
            StateNodes::All => included.push("nodes"),
        }
        if configuration {
            included.push("configuration");
        }
        Ok(Some(Self {
            schema_version: 1,
            included,
            nodes,
        }))
    }

    pub fn configuration(&self) -> bool {
        self.included.contains(&"configuration")
    }
}
