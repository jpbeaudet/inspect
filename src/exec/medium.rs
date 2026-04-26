//! `source` label parser.
//!
//! Bible §9.2: the `source` label is the medium + locator. Every
//! selector must carry a `source` matcher. Examples:
//!
//! ```text
//! source="logs"
//! source="file:/etc/atlas.conf"
//! source="dir:/etc"
//! source="discovery"
//! source="state"
//! source="volume:milvus-data"
//! source="image"
//! source="network"
//! source="host:/var/log/syslog"
//! ```

use anyhow::{anyhow, Result};

/// What kind of medium a `source=...` value refers to.
///
/// `Medium` is matched against literally. Regex-style `source=~"..."` is
/// resolved by walking the catalogue of plausible mediums (logs, file,
/// dir, discovery, state, volume, image, network, host) and matching
/// each medium's stringified form. See [`Medium::all_well_known`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Medium {
    Logs,
    /// `file:<path>`
    File(String),
    /// `dir:<path>`
    Dir(String),
    Discovery,
    State,
    /// `volume:<name>` (empty name = all)
    Volume(Option<String>),
    Image,
    Network,
    /// `host:<path>` — read a host-level file (no container)
    Host(String),
}

impl Medium {
    /// Parse a `source` label value into a [`Medium`].
    pub fn parse(raw: &str) -> Result<Self> {
        let s = raw.trim();
        if s.is_empty() {
            return Err(anyhow!("empty source value"));
        }
        if let Some(rest) = s.strip_prefix("file:") {
            if rest.is_empty() {
                return Err(anyhow!("`file:` requires a path"));
            }
            return Ok(Medium::File(rest.to_string()));
        }
        if let Some(rest) = s.strip_prefix("dir:") {
            if rest.is_empty() {
                return Err(anyhow!("`dir:` requires a path"));
            }
            return Ok(Medium::Dir(rest.to_string()));
        }
        if let Some(rest) = s.strip_prefix("volume:") {
            return Ok(Medium::Volume(if rest.is_empty() {
                None
            } else {
                Some(rest.to_string())
            }));
        }
        if let Some(rest) = s.strip_prefix("host:") {
            if rest.is_empty() {
                return Err(anyhow!("`host:` requires a path"));
            }
            return Ok(Medium::Host(rest.to_string()));
        }
        match s {
            "logs" => Ok(Medium::Logs),
            "discovery" => Ok(Medium::Discovery),
            "state" => Ok(Medium::State),
            "image" => Ok(Medium::Image),
            "network" => Ok(Medium::Network),
            "volume" => Ok(Medium::Volume(None)),
            other => Err(anyhow!(
                "unknown source `{other}`. Known: logs, file:<p>, dir:<p>, discovery, state, volume[:<n>], image, network, host:<p>"
            )),
        }
    }

    /// Render back to `source=` value form (round-trip with [`parse`]).
    pub fn as_label(&self) -> String {
        match self {
            Medium::Logs => "logs".into(),
            Medium::File(p) => format!("file:{p}"),
            Medium::Dir(p) => format!("dir:{p}"),
            Medium::Discovery => "discovery".into(),
            Medium::State => "state".into(),
            Medium::Volume(None) => "volume".into(),
            Medium::Volume(Some(n)) => format!("volume:{n}"),
            Medium::Image => "image".into(),
            Medium::Network => "network".into(),
            Medium::Host(p) => format!("host:{p}"),
        }
    }

    /// Stable kind name (what readers register under).
    pub fn kind(&self) -> &'static str {
        match self {
            Medium::Logs => "logs",
            Medium::File(_) => "file",
            Medium::Dir(_) => "dir",
            Medium::Discovery => "discovery",
            Medium::State => "state",
            Medium::Volume(_) => "volume",
            Medium::Image => "image",
            Medium::Network => "network",
            Medium::Host(_) => "host",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_well_known() {
        assert_eq!(Medium::parse("logs").unwrap(), Medium::Logs);
        assert_eq!(Medium::parse("discovery").unwrap(), Medium::Discovery);
        assert_eq!(Medium::parse("state").unwrap(), Medium::State);
        assert_eq!(Medium::parse("image").unwrap(), Medium::Image);
        assert_eq!(Medium::parse("network").unwrap(), Medium::Network);
    }
    #[test]
    fn parses_locator_forms() {
        assert_eq!(
            Medium::parse("file:/etc/x.conf").unwrap(),
            Medium::File("/etc/x.conf".into())
        );
        assert_eq!(
            Medium::parse("dir:/etc").unwrap(),
            Medium::Dir("/etc".into())
        );
        assert_eq!(
            Medium::parse("host:/var/log/syslog").unwrap(),
            Medium::Host("/var/log/syslog".into())
        );
        assert_eq!(
            Medium::parse("volume:milvus").unwrap(),
            Medium::Volume(Some("milvus".into()))
        );
        assert_eq!(Medium::parse("volume").unwrap(), Medium::Volume(None));
    }
    #[test]
    fn rejects_empty_locators() {
        assert!(Medium::parse("file:").is_err());
        assert!(Medium::parse("dir:").is_err());
        assert!(Medium::parse("host:").is_err());
        assert!(Medium::parse("").is_err());
    }
    #[test]
    fn rejects_unknown() {
        assert!(Medium::parse("kafka").is_err());
    }
    #[test]
    fn round_trips() {
        for src in [
            "logs",
            "discovery",
            "state",
            "image",
            "network",
            "file:/etc/x",
            "dir:/etc",
            "host:/var/log/syslog",
            "volume:milvus",
            "volume",
        ] {
            assert_eq!(Medium::parse(src).unwrap().as_label(), src);
        }
    }
}
