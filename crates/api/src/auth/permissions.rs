//! The permission matrix (`crates/api/permissions.toml`, ADR-0017): which
//! scopes and templates each role grants. Built into the service, and
//! checked against the engine's roles, the contract's scopes and the
//! released templates when it is loaded, so a typo fails at start-up
//! instead of silently denying (or allowing) something.

use std::collections::{BTreeMap, BTreeSet};

use serde::Deserialize;

use super::scope;

/// The matrix as the service ships it.
pub const BUILT_IN: &str = include_str!("../../permissions.toml");

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct File {
    roles: BTreeMap<String, RoleFile>,
    #[serde(default)]
    exclusive: Vec<ExclusiveFile>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RoleFile {
    scopes: Vec<String>,
    #[serde(default)]
    templates: Vec<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ExclusiveFile {
    roles: [String; 2],
    #[allow(dead_code)]
    reason: String,
}

/// What one role grants.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Grant {
    pub scopes: BTreeSet<String>,
    pub templates: BTreeSet<String>,
}

/// The whole matrix.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Permissions {
    roles: BTreeMap<String, Grant>,
}

impl Permissions {
    /// The built-in matrix, checked against the templates a client may
    /// post: released, and not booked only by the engine.
    pub fn built_in() -> Result<Permissions, String> {
        Permissions::parse(BUILT_IN, &mezgeb_ops::client_postable())
    }

    /// Parses and checks a matrix: every role of the engine and no other,
    /// only scopes the contract knows, only released templates, templates
    /// only where `operations:write` is granted, and the engine's exclusive
    /// role pairs.
    pub fn parse(text: &str, released: &[&str]) -> Result<Permissions, String> {
        let file: File = toml::from_str(text).map_err(|e| format!("permissions: {e}"))?;
        let names: BTreeSet<&str> = file.roles.keys().map(String::as_str).collect();
        let expected: BTreeSet<&str> = mezgeb_engine::ROLES.into_iter().collect();
        if names != expected {
            return Err(format!(
                "permissions: the roles must be exactly {expected:?}, not {names:?}"
            ));
        }
        let known: BTreeSet<&str> = scope::ALL
            .iter()
            .flat_map(|s| s.iter().copied())
            .chain(scope::MODIFIERS.iter().copied())
            .collect();
        let mut roles = BTreeMap::new();
        for (name, role) in file.roles {
            if let Some(unknown) = role.scopes.iter().find(|s| !known.contains(s.as_str())) {
                return Err(format!(
                    "permissions: role {name}: unknown scope {unknown:?}"
                ));
            }
            if let Some(unknown) = role
                .templates
                .iter()
                .find(|t| !released.contains(&t.as_str()))
            {
                return Err(format!(
                    "permissions: role {name}: {unknown:?} is not a released template"
                ));
            }
            let posts = role.scopes.iter().any(|s| s == "operations:write");
            if !role.templates.is_empty() && !posts {
                return Err(format!(
                    "permissions: role {name} lists templates without operations:write"
                ));
            }
            roles.insert(
                name,
                Grant {
                    scopes: role.scopes.into_iter().collect(),
                    templates: role.templates.into_iter().collect(),
                },
            );
        }
        let pairs: BTreeSet<(String, String)> = file
            .exclusive
            .into_iter()
            .map(|e| {
                let [a, b] = e.roles;
                if a <= b { (a, b) } else { (b, a) }
            })
            .collect();
        let engine: BTreeSet<(String, String)> = mezgeb_engine::EXCLUSIVE_ROLES
            .iter()
            .map(|(a, b)| {
                let (a, b) = ((*a).to_owned(), (*b).to_owned());
                if a <= b { (a, b) } else { (b, a) }
            })
            .collect();
        if pairs != engine {
            return Err(format!(
                "permissions: the exclusive roles must be {engine:?}, as the engine and the \
                 database enforce, not {pairs:?}"
            ));
        }
        Ok(Permissions { roles })
    }

    /// What each role grants.
    pub fn roles(&self) -> &BTreeMap<String, Grant> {
        &self.roles
    }

    /// The scopes a set of roles grants together.
    pub fn scopes(&self, roles: &[String]) -> BTreeSet<String> {
        roles
            .iter()
            .filter_map(|r| self.roles.get(r))
            .flat_map(|g| g.scopes.iter().cloned())
            .collect()
    }

    /// The templates a set of roles may post.
    pub fn templates(&self, roles: &[String]) -> BTreeSet<String> {
        roles
            .iter()
            .filter_map(|r| self.roles.get(r))
            .flat_map(|g| g.templates.iter().cloned())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roles(list: &[&str]) -> Vec<String> {
        list.iter().map(|r| (*r).to_owned()).collect()
    }

    #[test]
    fn the_built_in_matrix_is_valid() -> Result<(), String> {
        let p = Permissions::built_in()?;
        let poster = p.scopes(&roles(&["poster"]));
        assert!(poster.contains("operations:write"));
        assert!(!poster.contains("accounts:read_all"));
        // Every released template can be posted by someone, except those
        // only the engine books, which no one can.
        let postable = p.templates(&roles(&mezgeb_engine::ROLES));
        for id in mezgeb_ops::RELEASED {
            let engine_only = mezgeb_ops::ENGINE_ONLY.contains(id);
            assert_eq!(postable.contains(*id), !engine_only, "{id}");
        }
        assert!(
            Permissions::parse(
                &BUILT_IN.replace(
                    "\"p2p_transfer.v1\",",
                    "\"p2p_transfer.v1\", \"issue_voucher.v1\","
                ),
                &mezgeb_ops::client_postable()
            )
            .is_err(),
            "an engine-only template cannot be granted"
        );
        // Only the operator and the auditor read across clients.
        for (name, grant) in p.roles() {
            let all = grant.scopes.contains("accounts:read_all");
            assert_eq!(all, name == "operator" || name == "auditor", "{name}");
        }
        assert!(
            p.scopes(&roles(&["approver"]))
                .contains("adjustments:approve")
        );
        assert!(
            !p.scopes(&roles(&["operator"]))
                .contains("adjustments:approve")
        );
        Ok(())
    }

    /// A small matrix: the poster may post only P2P transfers; `extra` is
    /// added to the reader's section.
    fn matrix(extra: &str) -> String {
        format!(
            "[roles.poster]
scopes = [\"operations:write\"]
templates = [\"p2p_transfer.v1\"]
             [roles.reader]
scopes = [\"accounts:read\"]
{extra}
             [roles.operator]
scopes = []
[roles.approver]
scopes = []
             [roles.auditor]
scopes = []
[roles.admin]
scopes = []
             [[exclusive]]
roles = [\"operator\", \"approver\"]
reason = \"x\"
"
        )
    }

    #[test]
    fn an_unlisted_template_is_denied() -> Result<(), String> {
        let p = Permissions::parse(&matrix(""), mezgeb_ops::RELEASED)?;
        let poster = p.templates(&roles(&["poster"]));
        assert_eq!(poster.into_iter().collect::<Vec<_>>(), ["p2p_transfer.v1"]);
        assert!(p.templates(&roles(&["reader"])).is_empty());
        Ok(())
    }

    #[test]
    fn a_bad_matrix_is_refused() {
        let released = mezgeb_ops::RELEASED;
        for extra in [
            // An unknown scope.
            "[roles.root]
scopes = []",
            // Templates without operations:write.
            "templates = [\"p2p_transfer.v1\"]",
            // An unknown field.
            "super = true",
        ] {
            assert!(
                Permissions::parse(&matrix(extra), released).is_err(),
                "{extra}"
            );
        }
        let unknown_scope = matrix("").replace("accounts:read", "accounts:delete");
        assert!(Permissions::parse(&unknown_scope, released).is_err());
        // A template that is not released.
        assert!(Permissions::parse(&matrix(""), &["merchant_payment.v1"]).is_err());
        // The exclusive pair the engine enforces must be listed.
        let without = matrix("").replace(
            "[[exclusive]]
roles = [\"operator\", \"approver\"]
reason = \"x\"
",
            "",
        );
        assert!(Permissions::parse(&without, released).is_err());
    }
}
