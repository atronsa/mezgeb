//! Test harness for the API: a server in this process against a throwaway
//! database, an HTTP client, and the contract, `openapi/mezgeb.v1.yaml`,
//! which checks every response the tests receive.

#![allow(dead_code)]

#[path = "../../../engine/tests/support/mod.rs"]
pub mod support;

use std::collections::{BTreeSet, HashMap};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use mezgeb_api::auth::{Tokens, token_hash};
use mezgeb_api::{AppState, serve};
use mezgeb_core::Chart;
use mezgeb_engine::PostOptions;
use mezgeb_ops::TemplateRegistry;
use serde_json::{Map, Value};
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use tokio_util::task::TaskTracker;
use yaml_rust2::{Yaml, YamlLoader};

pub type TestResult = Result<(), Box<dyn std::error::Error>>;
pub type BoxError = Box<dyn std::error::Error + Send + Sync>;

pub const CLIENT: &str = "client-token";
pub const OPERATOR: &str = "operator-token";
pub const CHECKER: &str = "checker-token";
pub const CLIENT_ID: &str = "0192f3c4-0000-7000-8000-00000000c11e";
pub const OPERATOR_ID: &str = "0192f3c4-0000-7000-8000-0000000000ff";

const ALL_SCOPES: &str = r#""accounts:read", "accounts:read_all", "accounts:write", "accounts:admin",
    "operations:write", "entries:read", "corrections:write", "adjustments:request",
    "adjustments:approve", "holds:read", "holds:write", "periods:read", "periods:close",
    "reports:read", "offline:write""#;

fn token_file() -> String {
    format!(
        r#"
[[tokens]]
sha256 = "{}"
client_id = "{CLIENT_ID}"
actor = "app:wallet"
scopes = ["accounts:read", "accounts:write", "operations:write", "entries:read", "holds:read",
          "holds:write", "periods:read", "offline:write"]

[[tokens]]
sha256 = "{}"
client_id = "{OPERATOR_ID}"
actor = "ops:maker"
scopes = [{ALL_SCOPES}]

[[tokens]]
sha256 = "{}"
client_id = "{OPERATOR_ID}"
actor = "ops:checker"
scopes = ["adjustments:request", "adjustments:approve", "reports:read"]
"#,
        token_hash(CLIENT),
        token_hash(OPERATOR),
        token_hash(CHECKER)
    )
}

/// A running server.
pub struct TestServer {
    pub base: String,
    pub db: support::TestDb,
    shutdown: Option<oneshot::Sender<()>>,
    handle: Option<JoinHandle<std::io::Result<()>>>,
}

impl TestServer {
    /// A server on a fresh, migrated database, or `None` without a test
    /// database (see `engine/tests/support`).
    pub async fn start(
        timeout: Duration,
    ) -> Result<Option<TestServer>, Box<dyn std::error::Error>> {
        let Some(server) = support::Server::from_env()? else {
            return Ok(None);
        };
        let db = server.migrated().await?;
        Ok(Some(TestServer::on(db, timeout).await?))
    }

    /// A server on an existing database.
    pub async fn on(
        db: support::TestDb,
        timeout: Duration,
    ) -> Result<TestServer, Box<dyn std::error::Error>> {
        let chart = Chart::from_toml(include_str!("../../../../charts/wallet-provider.toml"))?;
        let state = AppState {
            pool: db.app_pool_of(16).await?,
            chart: Arc::new(chart),
            tokens: Arc::new(Tokens::parse(&token_file())?),
            registry: Arc::new(TemplateRegistry::standard()),
            options: PostOptions {
                lock_timeout: Duration::from_secs(20),
                ..PostOptions::default()
            },
            tasks: TaskTracker::new(),
            request_timeout: timeout,
            body_limit: 65_536,
        };
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let base = format!("http://{}", listener.local_addr()?);
        let (send, receive) = oneshot::channel::<()>();
        let handle = tokio::spawn(serve(listener, state, async move {
            let _ = receive.await;
        }));
        Ok(TestServer {
            base,
            db,
            shutdown: Some(send),
            handle: Some(handle),
        })
    }

    /// Stops it gracefully and waits until it has.
    pub async fn stop(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        if let Some(send) = self.shutdown.take() {
            let _ = send.send(());
        }
        if let Some(handle) = self.handle.take() {
            handle.await??;
        }
        Ok(())
    }

    /// Stops it and hands back its database, for another server.
    pub async fn into_db(mut self) -> Result<support::TestDb, Box<dyn std::error::Error>> {
        self.stop().await?;
        // `TestServer` has no Drop, so its database can be moved out.
        let TestServer { db, .. } = self;
        Ok(db)
    }

    /// Stops it and drops its database.
    pub async fn finish(mut self) -> TestResult {
        self.stop().await?;
        self.db.drop().await?;
        Ok(())
    }
}

// ---------------------------------------------------------------- the contract

fn to_json(yaml: &Yaml) -> Value {
    match yaml {
        Yaml::Hash(hash) => {
            let mut map = Map::new();
            for (k, v) in hash {
                let key = match k {
                    Yaml::String(s) => s.clone(),
                    Yaml::Integer(i) => i.to_string(),
                    other => format!("{other:?}"),
                };
                map.insert(key, to_json(v));
            }
            Value::Object(map)
        }
        Yaml::Array(items) => Value::Array(items.iter().map(to_json).collect()),
        Yaml::String(s) => Value::String(s.clone()),
        Yaml::Integer(i) => Value::from(*i),
        Yaml::Real(r) => r.parse::<f64>().map(Value::from).unwrap_or(Value::Null),
        Yaml::Boolean(b) => Value::Bool(*b),
        _ => Value::Null,
    }
}

/// The contract, and which operations the tests have exercised.
pub struct Contract {
    pub spec: Value,
    operations: Vec<(String, String)>,
    validators: Mutex<HashMap<String, Arc<jsonschema::Validator>>>,
    hits: Mutex<BTreeSet<(String, String, u16)>>,
}

impl Contract {
    pub fn load() -> Result<Contract, Box<dyn std::error::Error>> {
        let text = include_str!("../../../../openapi/mezgeb.v1.yaml");
        let docs = YamlLoader::load_from_str(text)?;
        let spec = to_json(docs.first().ok_or("empty contract")?);
        let mut operations = Vec::new();
        for (path, item) in spec["paths"].as_object().ok_or("no paths")? {
            for method in item.as_object().ok_or("bad path item")?.keys() {
                operations.push((method.to_uppercase(), path.clone()));
            }
        }
        Ok(Contract {
            spec,
            operations,
            validators: Mutex::new(HashMap::new()),
            hits: Mutex::new(BTreeSet::new()),
        })
    }

    /// The path template a request path matches, preferring the template
    /// with the most fixed segments.
    fn template(&self, method: &str, path: &str) -> Option<String> {
        let path = path.split('?').next().unwrap_or(path);
        let segments: Vec<&str> = path.split('/').collect();
        self.operations
            .iter()
            .filter(|(m, _)| m == method)
            .filter_map(|(_, template)| {
                let parts: Vec<&str> = template.split('/').collect();
                if parts.len() != segments.len() {
                    return None;
                }
                let mut fixed = 0;
                for (part, segment) in parts.iter().zip(&segments) {
                    if part.starts_with('{') {
                        continue;
                    }
                    if part != segment {
                        return None;
                    }
                    fixed += 1;
                }
                Some((fixed, template.clone()))
            })
            .max_by_key(|(fixed, _)| *fixed)
            .map(|(_, template)| template)
    }

    fn resolve<'a>(&'a self, value: &'a Value) -> &'a Value {
        match value.get("$ref").and_then(Value::as_str) {
            Some(reference) => {
                let pointer = reference.trim_start_matches('#');
                self.spec.pointer(pointer).unwrap_or(&Value::Null)
            }
            None => value,
        }
    }

    fn validator(&self, key: &str, schema: &Value) -> Result<Arc<jsonschema::Validator>, String> {
        let mut cache = self.validators.lock().map_err(|e| e.to_string())?;
        if let Some(v) = cache.get(key) {
            return Ok(v.clone());
        }
        let mut root = self.spec.clone();
        if let Value::Object(map) = &mut root {
            map.insert("x-test-schema".into(), schema.clone());
            map.insert("$ref".into(), Value::String("#/x-test-schema".into()));
        }
        let validator =
            jsonschema::draft202012::new(&root).map_err(|e| format!("schema {key}: {e}"))?;
        let validator = Arc::new(validator);
        cache.insert(key.to_owned(), validator.clone());
        Ok(validator)
    }

    /// Checks one response against the contract: the operation exists, the
    /// status is documented for it, and the body matches its schema.
    pub fn check(
        &self,
        method: &str,
        path: &str,
        status: u16,
        content_type: &str,
        body: &Value,
    ) -> Result<(), String> {
        let template = self
            .template(method, path)
            .ok_or_else(|| format!("{method} {path} is not in the contract"))?;
        let operation = &self.spec["paths"][&template][method.to_lowercase()];
        let documented = &operation["responses"][status.to_string()];
        if documented.is_null() {
            return Err(format!(
                "{method} {template}: status {status} is not documented (body {body})"
            ));
        }
        let response = self.resolve(documented);
        let media = content_type.split(';').next().unwrap_or("").trim();
        let Some(content) = response["content"].get(media) else {
            return Err(format!(
                "{method} {template} {status}: content type {media:?} is not documented"
            ));
        };
        let schema = &content["schema"];
        let key = format!("{method} {template} {status} {media}");
        let validator = self.validator(&key, schema)?;
        let errors: Vec<String> = validator
            .iter_errors(body)
            .map(|e| format!("{} at {}", e, e.instance_path()))
            .collect();
        if !errors.is_empty() {
            return Err(format!(
                "{method} {template} {status}: the body does not match the contract: {}\nbody: {body}",
                errors.join("; ")
            ));
        }
        if let Ok(mut hits) = self.hits.lock() {
            hits.insert((method.to_owned(), template, status));
        }
        Ok(())
    }

    /// The operations no test has called with a successful (2xx) response.
    pub fn uncovered(&self) -> Vec<String> {
        let hits = self.hits.lock().map(|h| h.clone()).unwrap_or_default();
        self.operations
            .iter()
            .filter(|(m, p)| {
                !hits
                    .iter()
                    .any(|(hm, hp, s)| hm == m && hp == p && ((200..300).contains(s) || *s == 501))
            })
            .map(|(m, p)| format!("{m} {p}"))
            .collect()
    }

    pub fn operation_count(&self) -> usize {
        self.operations.len()
    }
}

// ---------------------------------------------------------------- the client

/// A response, checked against the contract.
#[derive(Debug)]
pub struct Reply {
    pub status: u16,
    pub body: Value,
    pub headers: reqwest::header::HeaderMap,
}

impl Reply {
    pub fn code(&self) -> &str {
        self.body["code"].as_str().unwrap_or("")
    }

    pub fn str(&self, pointer: &str) -> String {
        self.body
            .pointer(pointer)
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned()
    }

    pub fn replayed(&self) -> bool {
        self.headers.get("idempotent-replayed").is_some()
    }
}

/// An HTTP client that checks every response against the contract.
#[derive(Clone)]
pub struct Api {
    pub http: reqwest::Client,
    pub base: String,
    pub contract: Arc<Contract>,
}

impl Api {
    pub fn new(base: &str, contract: Arc<Contract>) -> Api {
        Api {
            http: reqwest::Client::new(),
            base: base.to_owned(),
            contract,
        }
    }

    /// Sends a request and checks the response against the contract.
    pub async fn call(
        &self,
        method: &str,
        path: &str,
        token: Option<&str>,
        key: Option<&str>,
        body: Option<Value>,
    ) -> Result<Reply, BoxError> {
        let method_value = reqwest::Method::from_bytes(method.as_bytes())?;
        let mut request = self
            .http
            .request(method_value, format!("{}{path}", self.base));
        if let Some(token) = token {
            request = request.bearer_auth(token);
        }
        if let Some(key) = key {
            request = request.header("Idempotency-Key", key);
        }
        if let Some(body) = body {
            request = request.json(&body);
        }
        let response = request.send().await?;
        let status = response.status().as_u16();
        let headers = response.headers().clone();
        let content_type = headers
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_owned();
        let text = response.text().await?;
        let body: Value = if text.is_empty() {
            Value::Null
        } else {
            serde_json::from_str(&text)?
        };
        self.contract
            .check(method, path, status, &content_type, &body)?;
        Ok(Reply {
            status,
            body,
            headers,
        })
    }

    pub async fn get(&self, path: &str, token: &str) -> Result<Reply, BoxError> {
        self.call("GET", path, Some(token), None, None).await
    }

    pub async fn post(
        &self,
        path: &str,
        token: &str,
        key: &str,
        body: Value,
    ) -> Result<Reply, BoxError> {
        self.call("POST", path, Some(token), Some(key), Some(body))
            .await
    }
}

/// `{"amount": "...", "currency": "ETB"}`.
pub fn etb(amount: &str) -> Value {
    serde_json::json!({"amount": amount, "currency": "ETB"})
}
