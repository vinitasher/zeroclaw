//! A2A discovery surface: the well-known catalog card and per-alias agent
//! cards.
//!
//! A2A (Agent2Agent, Linux Foundation) assumes one agent per origin: a single
//! spec-conforming `AgentCard` at `/.well-known/agent-card.json`. ZeroClaw
//! hosts N agents per install, so the origin root serves a ZeroClaw discovery
//! catalog card (skills empty) enumerating the published aliases and each
//! one's per-alias endpoint and card URL. The catalog card is NOT a runnable
//! A2A agent; each published alias is, at its own endpoint.
//!
//! Cards are built on demand from the canonical `[agents.<alias>]` config (no
//! stored second agent list). Skills resolve through the same `SkillsService`
//! the dashboard uses, then narrow through the alias's `exposed_skills`
//! filter; the skill bundles stay the single source of truth.
//!
//! The card types here are serde-native and serialize to the A2A v1.0
//! protobuf-JSON wire shape. We roll them ourselves rather than depend on
//! `a2a-rs`, whose `AgentCard` is a one-agent-per-origin protobuf type and
//! pulls a ConnectRPC/prost/protoc build footprint that fights the
//! single-static-binary directive. The vendored proto at
//! `tests/fixtures/a2a-v1.proto` is the conformance reference.

use axum::{Json, Router, extract::State, http::StatusCode, response::IntoResponse, routing::get};
use serde::Serialize;

use zeroclaw_config::schema::Config;
use zeroclaw_runtime::skills::SkillsService;

use crate::AppState;

/// A2A protocol version advertised on per-alias interfaces.
const A2A_PROTOCOL_VERSION: &str = "1.0";
/// JSON-RPC is the spec-mandated baseline transport binding.
const A2A_PROTOCOL_BINDING: &str = "JSONRPC";
/// IANA-registered well-known discovery path (A2A spec §14.3, RFC 8615).
const WELL_KNOWN_AGENT_CARD_PATH: &str = "/.well-known/agent-card.json";

/// A single declared transport interface (A2A `AgentInterface`). The first
/// entry of `supportedInterfaces` is the preferred one.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AgentInterface {
    pub url: String,
    pub protocol_binding: String,
    pub protocol_version: String,
}

/// A2A capability flags. All optional; only `Some` values serialize.
#[derive(Debug, Clone, Default, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AgentCapabilities {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub streaming: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub push_notifications: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extended_agent_card: Option<bool>,
}

/// A2A `AgentSkill`. `id`/`name`/`description`/`tags` are spec-required.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AgentSkill {
    pub id: String,
    pub name: String,
    pub description: String,
    pub tags: Vec<String>,
}

/// A2A `AgentCard`. Serializes to the protobuf-JSON wire shape. Used for both
/// the per-alias spec-conforming cards and the ZeroClaw discovery catalog
/// card (the catalog uses `skills: []` and a synthetic catalog interface).
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AgentCard {
    pub name: String,
    pub description: String,
    pub supported_interfaces: Vec<AgentInterface>,
    pub version: String,
    pub capabilities: AgentCapabilities,
    pub default_input_modes: Vec<String>,
    pub default_output_modes: Vec<String>,
    pub skills: Vec<AgentSkill>,
}

/// Resolve the externally advertised base URL for endpoint fields. Prefers the
/// operator-set `public_base_url`; otherwise derives `http://<bind>:<port>`.
fn advertised_base(config: &Config) -> String {
    let server = &config.a2a.server;
    let configured = server.public_base_url.trim();
    if !configured.is_empty() {
        return configured.trim_end_matches('/').to_string();
    }
    format!("http://{}:{}", server.bind, server.port)
}

/// Per-alias A2A base path under the advertised origin.
fn alias_base_path(alias: &str) -> String {
    format!("/a2a/{alias}")
}

/// Build the ZeroClaw discovery catalog card served at the origin root. Lists
/// every published alias as a skill-less entry pointing at its per-alias card
/// and endpoint. This is a catalog, not a runnable agent: it advertises the
/// `catalog` interface and carries no skills of its own.
#[must_use]
pub fn build_catalog_card(config: &Config) -> AgentCard {
    let base = advertised_base(config);
    let published = published_aliases(config);

    let mut supported_interfaces = Vec::with_capacity(published.len() + 1);
    supported_interfaces.push(AgentInterface {
        url: format!("{base}{WELL_KNOWN_AGENT_CARD_PATH}"),
        protocol_binding: "catalog".to_string(),
        protocol_version: A2A_PROTOCOL_VERSION.to_string(),
    });
    for alias in &published {
        supported_interfaces.push(AgentInterface {
            url: format!("{base}{}", alias_base_path(alias)),
            protocol_binding: A2A_PROTOCOL_BINDING.to_string(),
            protocol_version: A2A_PROTOCOL_VERSION.to_string(),
        });
    }

    AgentCard {
        name: "ZeroClaw agent catalog".to_string(),
        description: "Discovery catalog enumerating published A2A agents on \
                      this ZeroClaw install. Not a runnable agent; each entry \
                      below serves its own A2A card and endpoint."
            .to_string(),
        supported_interfaces,
        version: env!("CARGO_PKG_VERSION").to_string(),
        capabilities: AgentCapabilities::default(),
        default_input_modes: vec!["text".to_string()],
        default_output_modes: vec!["text".to_string()],
        skills: Vec::new(),
    }
}

/// Build a spec-conforming per-alias agent card, or `None` when the alias is
/// unknown or not published. Skills resolve from the alias's bundles and
/// narrow through `exposed_skills`.
#[must_use]
pub fn build_agent_card(config: &Config, alias: &str) -> Option<AgentCard> {
    let agent = config.agents.get(alias)?;
    if !agent.a2a.published {
        return None;
    }

    let base = advertised_base(config);
    let endpoint = format!("{base}{}", alias_base_path(alias));

    AgentCard {
        name: alias.to_string(),
        description: agent_description(alias),
        supported_interfaces: vec![AgentInterface {
            url: endpoint,
            protocol_binding: A2A_PROTOCOL_BINDING.to_string(),
            protocol_version: A2A_PROTOCOL_VERSION.to_string(),
        }],
        version: env!("CARGO_PKG_VERSION").to_string(),
        capabilities: AgentCapabilities::default(),
        default_input_modes: vec!["text".to_string()],
        default_output_modes: vec!["text".to_string()],
        skills: exposed_skills(config, alias),
    }
    .into()
}

/// Aliases that are both enabled and A2A-published, in stable sorted order.
fn published_aliases(config: &Config) -> Vec<String> {
    let mut out: Vec<String> = config
        .agents
        .iter()
        .filter(|(_, agent)| agent.enabled && agent.a2a.published)
        .map(|(alias, _)| alias.clone())
        .collect();
    out.sort();
    out
}

/// One-line agent description for the card. Neutral per-alias default; a
/// richer source (identity document first line) can supersede this later.
fn agent_description(alias: &str) -> String {
    format!("ZeroClaw agent '{alias}'.")
}

/// Resolve the alias's exposed skills: the resolved bundle skill set narrowed
/// by `exposed_skills`. An empty filter advertises no skills. Skill ids that
/// do not resolve to a real skill are dropped (bundles are canonical).
fn exposed_skills(config: &Config, alias: &str) -> Vec<AgentSkill> {
    let agent = match config.agents.get(alias) {
        Some(a) => a,
        None => return Vec::new(),
    };
    if agent.a2a.exposed_skills.is_empty() {
        return Vec::new();
    }

    let install_root = config.install_root_dir();
    let service = SkillsService::new(config, install_root);
    let resolved = match service.list_skills(Some(alias)) {
        Ok(skills) => skills,
        Err(_) => return Vec::new(),
    };

    let mut out = Vec::new();
    for wanted in &agent.a2a.exposed_skills {
        if let Some(summary) = resolved.iter().find(|s| s.r#ref.name() == wanted) {
            out.push(AgentSkill {
                id: summary.r#ref.name().to_string(),
                name: summary.frontmatter.name.clone(),
                description: summary.frontmatter.description.clone(),
                tags: Vec::new(),
            });
        }
    }
    out
}

/// `GET /.well-known/agent-card.json` — the discovery catalog card.
async fn handle_catalog_card(State(state): State<AppState>) -> impl IntoResponse {
    let config = state.config.read().clone();
    if !config.a2a.server.enabled {
        return StatusCode::NOT_FOUND.into_response();
    }
    Json(build_catalog_card(&config)).into_response()
}

/// `GET /a2a/{alias}/.well-known/agent-card.json` — a per-alias agent card.
async fn handle_alias_card(
    State(state): State<AppState>,
    axum::extract::Path(alias): axum::extract::Path<String>,
) -> impl IntoResponse {
    let config = state.config.read().clone();
    if !config.a2a.server.enabled {
        return StatusCode::NOT_FOUND.into_response();
    }
    match build_agent_card(&config, &alias) {
        Some(card) => Json(card).into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

/// A2A discovery routes. The server-enabled gate is enforced per request (so a
/// runtime config reload toggles it without a router rebuild); the routes are
/// always mounted but answer 404 while disabled.
pub fn a2a_routes() -> Router<AppState> {
    Router::new()
        .route(WELL_KNOWN_AGENT_CARD_PATH, get(handle_catalog_card))
        .route(
            "/a2a/{alias}/.well-known/agent-card.json",
            get(handle_alias_card),
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    use zeroclaw_config::multi_agent::AgentA2aConfig;
    use zeroclaw_config::schema::{AliasedAgentConfig, Config};

    fn config_with_published_alias(alias: &str, published: bool) -> Config {
        let mut config = Config::default();
        config.a2a.server.enabled = true;
        config.a2a.server.bind = "127.0.0.1".into();
        config.a2a.server.port = 18800;
        let agent = AliasedAgentConfig {
            a2a: AgentA2aConfig {
                published,
                exposed_skills: Vec::new(),
            },
            ..Default::default()
        };
        config.agents.insert(alias.to_string(), agent);
        config
    }

    #[test]
    fn catalog_card_has_no_skills_and_lists_published_aliases() {
        let config = config_with_published_alias("researcher", true);
        let card = build_catalog_card(&config);
        assert!(card.skills.is_empty());
        // catalog interface + one per-alias interface
        assert_eq!(card.supported_interfaces.len(), 2);
        assert_eq!(card.supported_interfaces[0].protocol_binding, "catalog");
        assert!(
            card.supported_interfaces[1]
                .url
                .ends_with("/a2a/researcher")
        );
    }

    #[test]
    fn catalog_card_excludes_unpublished_aliases() {
        let config = config_with_published_alias("hidden", false);
        let card = build_catalog_card(&config);
        // only the catalog interface, no alias entry
        assert_eq!(card.supported_interfaces.len(), 1);
        assert_eq!(card.supported_interfaces[0].protocol_binding, "catalog");
    }

    #[test]
    fn agent_card_none_for_unpublished_alias() {
        let config = config_with_published_alias("hidden", false);
        assert!(build_agent_card(&config, "hidden").is_none());
    }

    #[test]
    fn agent_card_none_for_unknown_alias() {
        let config = config_with_published_alias("known", true);
        assert!(build_agent_card(&config, "ghost").is_none());
    }

    #[test]
    fn published_agent_card_is_spec_shaped() {
        let config = config_with_published_alias("researcher", true);
        let card = build_agent_card(&config, "researcher").expect("card");
        assert_eq!(card.name, "researcher");
        assert_eq!(card.supported_interfaces.len(), 1);
        assert_eq!(
            card.supported_interfaces[0].protocol_binding,
            A2A_PROTOCOL_BINDING
        );
        // empty exposed_skills filter advertises no skills
        assert!(card.skills.is_empty());
    }

    #[test]
    fn public_base_url_overrides_derived_endpoint() {
        let mut config = config_with_published_alias("researcher", true);
        config.a2a.server.public_base_url = "https://agents.example.com/".into();
        let card = build_catalog_card(&config);
        assert_eq!(
            card.supported_interfaces[0].url,
            "https://agents.example.com/.well-known/agent-card.json"
        );
    }

    #[test]
    fn card_serializes_to_camelcase_wire_shape() {
        let config = config_with_published_alias("researcher", true);
        let card = build_agent_card(&config, "researcher").expect("card");
        let json = serde_json::to_value(&card).expect("serialize");
        assert!(json.get("supportedInterfaces").is_some());
        assert!(json.get("defaultInputModes").is_some());
        assert!(json.get("defaultOutputModes").is_some());
        // snake_case must not leak into the wire shape
        assert!(json.get("supported_interfaces").is_none());
    }
}
