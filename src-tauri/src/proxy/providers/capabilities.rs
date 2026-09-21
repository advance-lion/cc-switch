//! Provider Center compatibility registry.
//!
//! Single source of truth for whether a given upstream protocol can be used
//! by a given Agent, either directly or through CC Switch's local proxy
//! translation layer.  Provider Center, preview/apply, and the frontend all
//! consume this module so they never disagree.

use serde::{Deserialize, Serialize};

/// How an Agent can reach an upstream provider.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum Compatibility {
    /// The Agent natively speaks the upstream protocol — no proxy needed.
    Direct,

    /// The Agent's native protocol differs from the upstream, but CC Switch
    /// has a bidirectional request/response/streaming transform path.
    Proxy {
        /// Stable identifier for the transform route.
        route_id: String,
        /// Whether the Agent must be in proxy-takeover mode to use this route.
        requires_takeover: bool,
    },

    /// Neither direct nor proxy translation is available.
    Unsupported {
        /// Human-readable reason (stable, not localized — the frontend maps it).
        reason: String,
    },
}

impl Compatibility {
    pub fn is_compatible(&self) -> bool {
        !matches!(self, Compatibility::Unsupported { .. })
    }

    pub fn requires_proxy(&self) -> bool {
        matches!(self, Compatibility::Proxy { .. })
    }
}

/// Route identifiers for the transform paths CC Switch currently implements.
pub mod route {
    pub const CLAUDE_TO_OPENAI_CHAT: &str = "claude-to-openai-chat";
    pub const CLAUDE_TO_OPENAI_RESPONSES: &str = "claude-to-openai-responses";
    pub const CLAUDE_TO_GEMINI: &str = "claude-to-gemini";
    pub const CODEX_TO_OPENAI_CHAT: &str = "codex-to-openai-chat";
    pub const CODEX_TO_ANTHROPIC: &str = "codex-to-anthropic";
}

/// Resolve whether `target_agent` can use a provider whose upstream speaks
/// `upstream_protocol`.
///
/// `upstream_protocol` uses the Provider Center protocol strings:
/// `anthropic`, `openai-chat`, `openai-responses`, `gemini`, `ollama`.
///
/// `target_agent` uses `AppType::as_str()` values:
/// `claude`, `claude-desktop`, `codex`, `gemini`, `grokbuild`,
/// `opencode`, `openclaw`, `hermes`, `pi`.
pub fn resolve_compatibility(upstream_protocol: &str, target_agent: &str) -> Compatibility {
    let proto = normalize_protocol(upstream_protocol);
    let agent = target_agent.trim().to_lowercase();

    match (agent.as_str(), proto.as_str()) {
        // ── Claude / Claude Desktop ──────────────────────────────────
        ("claude" | "claude-desktop", "anthropic") => Compatibility::Direct,
        ("claude" | "claude-desktop", "openai-chat") => Compatibility::Proxy {
            route_id: route::CLAUDE_TO_OPENAI_CHAT.to_string(),
            requires_takeover: true,
        },
        ("claude" | "claude-desktop", "openai-responses") => Compatibility::Proxy {
            route_id: route::CLAUDE_TO_OPENAI_RESPONSES.to_string(),
            requires_takeover: true,
        },
        ("claude" | "claude-desktop", "gemini") => Compatibility::Proxy {
            route_id: route::CLAUDE_TO_GEMINI.to_string(),
            requires_takeover: true,
        },

        // ── Codex ────────────────────────────────────────────────────
        ("codex", "openai-responses") => Compatibility::Direct,
        ("codex", "openai-chat") => Compatibility::Proxy {
            route_id: route::CODEX_TO_OPENAI_CHAT.to_string(),
            requires_takeover: true,
        },
        ("codex", "anthropic") => Compatibility::Proxy {
            route_id: route::CODEX_TO_ANTHROPIC.to_string(),
            requires_takeover: true,
        },

        // ── Gemini ───────────────────────────────────────────────────
        ("gemini", "gemini") => Compatibility::Direct,

        // ── Grok Build ───────────────────────────────────────────────
        ("grokbuild", "openai-chat") => Compatibility::Direct,

        // ── OpenCode ─────────────────────────────────────────────────
        ("opencode", "openai-chat") | ("opencode", "ollama") => Compatibility::Direct,

        // ── OpenClaw (universal) ─────────────────────────────────────
        ("openclaw", _) => Compatibility::Direct,

        // ── Hermes ───────────────────────────────────────────────────
        ("hermes", "openai-chat") | ("hermes", "ollama") => Compatibility::Direct,

        // ── Pi (universal) ───────────────────────────────────────────
        ("pi", _) => Compatibility::Direct,

        // ── DeepSeek Harness ─────────────────────────────────────────────
        // Hand-declared llm-pi-ai providers support these three native APIs.
        ("dsh", "openai-chat" | "openai-responses" | "anthropic" | "ollama") => {
            Compatibility::Direct
        }

        // ── Everything else ──────────────────────────────────────────
        _ => Compatibility::Unsupported {
            reason: format!(
                "{agent} 当前不支持 {upstream_protocol} 协议，且没有可用的本地路由转换"
            ),
        },
    }
}

/// Normalize Provider Center protocol strings to canonical form.
fn normalize_protocol(raw: &str) -> String {
    match raw.trim().to_lowercase().as_str() {
        "openai-chat" | "openai_chat" | "openai-chat-completions" => "openai-chat",
        "openai-responses" | "openai_responses" | "responses" => "openai-responses",
        "anthropic" | "anthropic-messages" | "anthropic_messages" => "anthropic",
        "gemini" | "gemini-generate-content" | "gemini_native" | "gemini-native" => "gemini",
        "ollama" => "ollama",
        other => other,
    }
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claude_direct_anthropic() {
        assert_eq!(
            resolve_compatibility("anthropic", "claude"),
            Compatibility::Direct
        );
    }

    #[test]
    fn claude_proxy_openai_chat() {
        let c = resolve_compatibility("openai-chat", "claude");
        assert!(matches!(
            c,
            Compatibility::Proxy {
                requires_takeover: true,
                ..
            }
        ));
        assert!(c.requires_proxy());
    }

    #[test]
    fn claude_desktop_proxy_openai_responses() {
        let c = resolve_compatibility("openai-responses", "claude-desktop");
        assert!(matches!(
            c,
            Compatibility::Proxy {
                requires_takeover: true,
                ..
            }
        ));
    }

    #[test]
    fn claude_proxy_gemini() {
        let c = resolve_compatibility("gemini", "claude");
        assert!(matches!(
            c,
            Compatibility::Proxy {
                requires_takeover: true,
                ..
            }
        ));
    }

    #[test]
    fn codex_direct_responses() {
        assert_eq!(
            resolve_compatibility("openai-responses", "codex"),
            Compatibility::Direct
        );
    }

    #[test]
    fn codex_proxy_chat() {
        let c = resolve_compatibility("openai-chat", "codex");
        assert!(matches!(
            c,
            Compatibility::Proxy {
                requires_takeover: true,
                ..
            }
        ));
    }

    #[test]
    fn codex_proxy_anthropic() {
        let c = resolve_compatibility("anthropic", "codex");
        assert!(matches!(
            c,
            Compatibility::Proxy {
                requires_takeover: true,
                ..
            }
        ));
    }

    #[test]
    fn gemini_direct_only() {
        assert_eq!(
            resolve_compatibility("gemini", "gemini"),
            Compatibility::Direct
        );
        assert!(matches!(
            resolve_compatibility("openai-chat", "gemini"),
            Compatibility::Unsupported { .. }
        ));
    }

    #[test]
    fn grokbuild_direct_chat() {
        assert_eq!(
            resolve_compatibility("openai-chat", "grokbuild"),
            Compatibility::Direct
        );
    }

    #[test]
    fn opencode_direct_chat_and_ollama() {
        assert_eq!(
            resolve_compatibility("openai-chat", "opencode"),
            Compatibility::Direct
        );
        assert_eq!(
            resolve_compatibility("ollama", "opencode"),
            Compatibility::Direct
        );
    }

    #[test]
    fn openclaw_universal() {
        for proto in &[
            "openai-chat",
            "openai-responses",
            "anthropic",
            "gemini",
            "ollama",
        ] {
            assert_eq!(
                resolve_compatibility(proto, "openclaw"),
                Compatibility::Direct,
                "openclaw should be direct for {proto}"
            );
        }
    }

    #[test]
    fn hermes_direct_chat_and_ollama() {
        assert_eq!(
            resolve_compatibility("openai-chat", "hermes"),
            Compatibility::Direct
        );
        assert_eq!(
            resolve_compatibility("ollama", "hermes"),
            Compatibility::Direct
        );
    }

    #[test]
    fn pi_universal() {
        for proto in &[
            "openai-chat",
            "openai-responses",
            "anthropic",
            "gemini",
            "ollama",
        ] {
            assert_eq!(
                resolve_compatibility(proto, "pi"),
                Compatibility::Direct,
                "pi should be direct for {proto}"
            );
        }
    }

    #[test]
    fn unsupported_combinations() {
        assert!(matches!(
            resolve_compatibility("openai-chat", "gemini"),
            Compatibility::Unsupported { .. }
        ));
        assert!(matches!(
            resolve_compatibility("anthropic", "grokbuild"),
            Compatibility::Unsupported { .. }
        ));
    }

    #[test]
    fn protocol_normalization() {
        assert_eq!(
            resolve_compatibility("openai_chat", "claude"),
            resolve_compatibility("openai-chat", "claude"),
        );
        assert_eq!(
            resolve_compatibility("OpenAI-Responses", "codex"),
            resolve_compatibility("openai-responses", "codex"),
        );
    }
}
