//! `mezgeb-cli client` and `mezgeb-cli credential`: managing who may call
//! the ledger (step 2.08, ADR-0017). Connect as `mezgeb_admin`.
//!
//! ```text
//! mezgeb-cli client create NAME [--rate N] [--burst N]
//! mezgeb-cli client list
//! mezgeb-cli client disable NAME
//! mezgeb-cli client seed FILE
//! mezgeb-cli credential issue CLIENT --actor ACTOR --role ROLE [--role ROLE]...
//! mezgeb-cli credential list CLIENT
//! mezgeb-cli credential revoke CREDENTIAL_ID
//! ```
//!
//! `credential issue` prints the new credential's `client_id` and secret.
//! The secret is shown once and stored only as its Argon2id hash: keep it
//! in the client's secret store. To rotate, issue a second credential,
//! switch the client to it, then revoke the first.
//!
//! `client seed` creates the clients and credentials of a file with the
//! secrets it names, and skips those that exist. It is for development and
//! CI (`deploy/clients.dev.toml`), never for production secrets.

use std::fmt::Write as _;

use mezgeb_core::{ActorId, ClientId};
use mezgeb_engine::{
    ClientError, ROLES, add_credential, client_by_name, create_client, disable_client,
    generate_secret, hash_secret, list_clients, list_credentials, revoke_credential,
};
use serde::Deserialize;
use sqlx::PgConnection;
use uuid::Uuid;

pub const USAGE: &str = "       mezgeb-cli client create NAME [--rate N] [--burst N] [--database-url URL]\n       \
     mezgeb-cli client list|disable NAME|seed FILE [--database-url URL]\n       \
     mezgeb-cli credential issue CLIENT --actor ACTOR --role ROLE... [--database-url URL]\n       \
     mezgeb-cli credential list CLIENT|revoke CREDENTIAL_ID [--database-url URL]";

/// A parsed `client` or `credential` command.
#[derive(Debug, PartialEq, Eq)]
pub enum ClientCommand {
    Create {
        name: String,
        rate: u32,
        burst: u32,
    },
    List,
    Disable {
        name: String,
    },
    Seed {
        file: String,
    },
    Issue {
        client: String,
        actor: String,
        roles: Vec<String>,
    },
    Credentials {
        client: String,
    },
    Revoke {
        credential: Uuid,
    },
}

fn number(args: &mut std::slice::Iter<'_, String>, flag: &str) -> Result<u32, String> {
    args.next()
        .and_then(|n| n.parse().ok())
        .filter(|n| *n >= 1)
        .ok_or_else(|| format!("{flag} needs a whole number of at least 1"))
}

/// Parses the words after `client` or `credential`. `--database-url` is
/// taken out by the caller.
pub fn parse(group: &str, args: &[String]) -> Result<ClientCommand, String> {
    let usage = || format!("usage:\n{USAGE}");
    let mut args = args.iter();
    let verb = args.next().map(String::as_str);
    let mut positional = Vec::new();
    let (mut rate, mut burst, mut actor, mut roles) = (100, 200, None, Vec::new());
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--rate" => rate = number(&mut args, "--rate")?,
            "--burst" => burst = number(&mut args, "--burst")?,
            "--actor" => actor = Some(args.next().ok_or("--actor needs a name")?.clone()),
            "--role" => roles.push(args.next().ok_or("--role needs a role")?.clone()),
            other if other.starts_with("--") => {
                return Err(format!("unknown argument {other:?}\n{}", usage()));
            }
            other => positional.push(other.to_owned()),
        }
    }
    let one = |what: &str| -> Result<String, String> {
        match positional.as_slice() {
            [only] => Ok(only.clone()),
            _ => Err(format!(
                "{group} {} needs one {what}\n{}",
                verb.unwrap_or(""),
                usage()
            )),
        }
    };
    match (group, verb) {
        ("client", Some("create")) => Ok(ClientCommand::Create {
            name: one("name")?,
            rate,
            burst,
        }),
        ("client", Some("list")) if positional.is_empty() => Ok(ClientCommand::List),
        ("client", Some("disable")) => Ok(ClientCommand::Disable { name: one("name")? }),
        ("client", Some("seed")) => Ok(ClientCommand::Seed { file: one("file")? }),
        ("credential", Some("issue")) => {
            let actor = actor.ok_or(format!("credential issue needs --actor\n{}", usage()))?;
            if roles.is_empty() {
                return Err(format!(
                    "credential issue needs at least one --role ({})\n{}",
                    ROLES.join(", "),
                    usage()
                ));
            }
            Ok(ClientCommand::Issue {
                client: one("client name")?,
                actor,
                roles,
            })
        }
        ("credential", Some("list")) => Ok(ClientCommand::Credentials {
            client: one("client name")?,
        }),
        ("credential", Some("revoke")) => {
            let id = one("credential ID")?;
            Ok(ClientCommand::Revoke {
                credential: id
                    .parse()
                    .map_err(|_| format!("{id:?} is not a credential ID"))?,
            })
        }
        _ => Err(usage()),
    }
}

/// The seed file: clients, each with its credentials and their secrets.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SeedFile {
    clients: Vec<SeedClient>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SeedClient {
    id: Uuid,
    name: String,
    #[serde(default = "default_rate")]
    rate_per_second: u32,
    #[serde(default = "default_burst")]
    burst: u32,
    credentials: Vec<SeedCredential>,
}

fn default_rate() -> u32 {
    100
}

fn default_burst() -> u32 {
    200
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SeedCredential {
    id: Uuid,
    actor: String,
    roles: Vec<String>,
    secret: String,
}

fn failed(error: ClientError) -> String {
    error.to_string()
}

async fn client_named(conn: &mut PgConnection, name: &str) -> Result<ClientId, String> {
    client_by_name(conn, name)
        .await
        .map_err(failed)?
        .map(|c| c.id)
        .ok_or_else(|| format!("no client named {name:?}"))
}

/// Runs the command; the output is printed by the caller.
pub async fn run(conn: &mut PgConnection, command: ClientCommand) -> Result<String, String> {
    let mut out = String::new();
    match command {
        ClientCommand::Create { name, rate, burst } => {
            let id = ClientId::from_uuid(Uuid::now_v7()).map_err(|e| e.to_string())?;
            let client = create_client(conn, id, &name, rate, burst)
                .await
                .map_err(failed)?;
            let _ = writeln!(
                out,
                "client {} ({}) created: {} requests per second, bursts of {}",
                client.name, client.id, client.rate_per_second, client.burst
            );
        }
        ClientCommand::List => {
            for c in list_clients(conn).await.map_err(failed)? {
                let status = if c.active { "active" } else { "disabled" };
                let _ = writeln!(
                    out,
                    "{}  {}  {status}  {}/s burst {}",
                    c.id, c.name, c.rate_per_second, c.burst
                );
            }
        }
        ClientCommand::Disable { name } => {
            let id = client_named(conn, &name).await?;
            disable_client(conn, id).await.map_err(failed)?;
            let _ = writeln!(out, "client {name} disabled: its tokens no longer work");
        }
        ClientCommand::Issue {
            client,
            actor,
            roles,
        } => {
            let id = client_named(conn, &client).await?;
            let actor = ActorId::new(&actor).map_err(|e| format!("actor {actor:?}: {e}"))?;
            let secret = generate_secret().map_err(|e| e.to_string())?;
            let hash = hash_secret(&secret).map_err(|e| e.to_string())?;
            let credential = add_credential(conn, Uuid::now_v7(), id, &actor, &roles, &hash)
                .await
                .map_err(failed)?;
            let _ = writeln!(out, "client_id:     {}", credential.id);
            let _ = writeln!(out, "client_secret: {secret}");
            let _ = writeln!(
                out,
                "roles:         {}\nThe secret is shown only now. Keep it in the client's secret store.",
                credential.roles.join(", ")
            );
        }
        ClientCommand::Credentials { client } => {
            let id = client_named(conn, &client).await?;
            for c in list_credentials(conn, id).await.map_err(failed)? {
                let status = if c.revoked_at.is_some() {
                    "revoked"
                } else {
                    "active"
                };
                let _ = writeln!(
                    out,
                    "{}  {}  {status}  {}",
                    c.id,
                    c.actor,
                    c.roles.join(",")
                );
            }
        }
        ClientCommand::Revoke { credential } => {
            let revoked = revoke_credential(conn, credential).await.map_err(failed)?;
            let _ = writeln!(
                out,
                "credential {} ({}) revoked: its tokens no longer work",
                revoked.id, revoked.actor
            );
        }
        ClientCommand::Seed { file } => {
            let text =
                std::fs::read_to_string(&file).map_err(|e| format!("cannot read {file}: {e}"))?;
            let seed: SeedFile = toml::from_str(&text).map_err(|e| format!("{file}: {e}"))?;
            for client in seed.clients {
                let id = ClientId::from_uuid(client.id).map_err(|e| e.to_string())?;
                match create_client(conn, id, &client.name, client.rate_per_second, client.burst)
                    .await
                {
                    Ok(_) => {
                        let _ = writeln!(out, "client {} created", client.name);
                    }
                    Err(ClientError::NameTaken(_)) => {}
                    Err(e) => return Err(failed(e)),
                }
                let existing: Vec<Uuid> = list_credentials(conn, id)
                    .await
                    .map_err(failed)?
                    .iter()
                    .map(|c| c.id)
                    .collect();
                for credential in client.credentials {
                    if existing.contains(&credential.id) {
                        continue;
                    }
                    if credential.secret.len() < 16 {
                        return Err(format!(
                            "{file}: the secret of {} is shorter than 16 characters",
                            credential.actor
                        ));
                    }
                    let actor = ActorId::new(&credential.actor).map_err(|e| e.to_string())?;
                    let hash = hash_secret(&credential.secret).map_err(|e| e.to_string())?;
                    add_credential(conn, credential.id, id, &actor, &credential.roles, &hash)
                        .await
                        .map_err(failed)?;
                    let _ = writeln!(out, "  credential {} ({}) added", credential.id, actor);
                }
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn parses_client_commands() {
        assert_eq!(
            parse("client", &args(&["create", "wallet-app", "--rate", "50"])),
            Ok(ClientCommand::Create {
                name: "wallet-app".into(),
                rate: 50,
                burst: 200
            })
        );
        assert_eq!(parse("client", &args(&["list"])), Ok(ClientCommand::List));
        assert_eq!(
            parse("client", &args(&["seed", "deploy/clients.dev.toml"])),
            Ok(ClientCommand::Seed {
                file: "deploy/clients.dev.toml".into()
            })
        );
        assert_eq!(
            parse(
                "credential",
                &args(&[
                    "issue",
                    "wallet-app",
                    "--actor",
                    "app:wallet",
                    "--role",
                    "poster",
                    "--role",
                    "reader"
                ])
            ),
            Ok(ClientCommand::Issue {
                client: "wallet-app".into(),
                actor: "app:wallet".into(),
                roles: vec!["poster".into(), "reader".into()]
            })
        );
        assert!(matches!(
            parse(
                "credential",
                &args(&["revoke", "0192f3c4-0000-7000-8000-00000000c11e"])
            ),
            Ok(ClientCommand::Revoke { .. })
        ));
    }

    #[test]
    fn refuses_bad_client_commands() {
        for bad in [
            &["create"][..],
            &["create", "a", "b"],
            &["create", "a", "--rate", "0"],
            &["create", "a", "--rate"],
            &["list", "extra"],
            &["drop", "a"],
        ] {
            assert!(parse("client", &args(bad)).is_err(), "{bad:?}");
        }
        assert!(parse("credential", &args(&["issue", "a", "--actor", "x"])).is_err());
        assert!(parse("credential", &args(&["issue", "a", "--role", "poster"])).is_err());
        assert!(parse("credential", &args(&["revoke", "not-an-id"])).is_err());
    }
}
