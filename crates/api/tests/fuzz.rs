//! API fuzzing (step 2.16): random requests against the running service.
//!
//! Each case takes an operation of `openapi/mezgeb.v1.yaml`, starts from its
//! request example (so that many cases get past parsing and into the
//! ledger), and mutates it: a value replaced by a hostile one, a field
//! removed or added, an ID swapped for one the service returned earlier, a
//! body that is not JSON at all, a strange path parameter, query string,
//! idempotency key or token. Cases run eight at a time, so they race too.
//!
//! A case fails if the service answers `5xx`, or answers anything the
//! contract does not document. At the end, the ledger's full verification
//! (`TestDb::drop`: balances, totals, chains) and the escrow check must find
//! nothing: random input must never break an invariant.
//!
//! `MEZGEB_FUZZ_SEED` (default 1) and `MEZGEB_FUZZ_CASES` (default 1500)
//! choose the run; a failure prints the seed and the case to replay.

mod common;

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use common::{
    BoxError, CHECKER, CLIENT, CLIENT_ID, Contract, OPERATOR, OTHER, ROLE_TOKENS, TestServer,
};
use serde_json::{Map, Value, json};

/// SplitMix64: small, fast and the same everywhere, so a seed replays.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }

    fn chance(&mut self, percent: u64) -> bool {
        self.next() % 100 < percent
    }

    fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        &items[self.below(items.len())]
    }
}

fn env_number(name: &str, default: u64) -> u64 {
    std::env::var(name)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

/// Strings that have broken parsers, ledgers and logs before.
fn hostile_strings() -> Vec<String> {
    let mut strings: Vec<String> = [
        "",
        " ",
        "0",
        "-1",
        "-0.00",
        "0.00",
        "1.001",
        "1e3",
        "NaN",
        "Infinity",
        "null",
        "true",
        "{}",
        "[]",
        "\u{0}",
        "\u{202e}",
        "２",
        "١",
        "😀",
        "' OR 1=1 --",
        "\"; DROP TABLE entries; --",
        "<script>",
        "%00",
        "../../etc/passwd",
        "\\",
        "\r\nX-Injected: 1",
        "${jndi:ldap://x}",
        "1010",
        "4010",
        "2010:cust-abebe",
        "2010:",
        ":",
        "2010:cust-",
        "9999:x",
        "0192f3c4-0000-7000-8000-00000000dead",
        "00000000-0000-0000-0000-000000000000",
        "92233720368547758.07",
        "92233720368547758.08",
        "-92233720368547758.08",
        "2099-01-01T00:00:00Z",
        "1970-01-01T00:00:00Z",
        "2026-02-30T00:00:00Z",
        "ETB",
        "etb",
        "USD",
        "p2p_transfer.v1",
        "operator",
        "frozen",
        "closed",
    ]
    .iter()
    .map(|s| (*s).to_owned())
    .collect();
    strings.push("9".repeat(40));
    strings.push("a".repeat(5_000));
    strings
}

fn random_value(rng: &mut Rng, depth: u32, strings: &[String], pool: &[String]) -> Value {
    match rng.below(if depth == 0 { 6 } else { 8 }) {
        0 => Value::Null,
        1 => Value::Bool(rng.chance(50)),
        2 => json!(*rng.pick(&[0i64, -1, 1, i64::MAX, i64::MIN, 9_007_199_254_740_993, 100])),
        3 => json!(*rng.pick(&[1.5f64, -0.0, 1e308, 0.1, -2.5])),
        4 if !pool.is_empty() => Value::String(rng.pick(pool).clone()),
        4 | 5 => Value::String(rng.pick(strings).clone()),
        6 => Value::Array(
            (0..rng.below(4))
                .map(|_| random_value(rng, depth - 1, strings, pool))
                .collect(),
        ),
        _ => {
            let mut map = Map::new();
            for _ in 0..rng.below(4) {
                let key = if rng.chance(50) {
                    rng.pick(&["amount", "currency", "accounts", "amounts", "id", "status"])
                        .to_string()
                } else {
                    rng.pick(strings).clone()
                };
                map.insert(key, random_value(rng, depth - 1, strings, pool));
            }
            Value::Object(map)
        }
    }
}

/// Every JSON pointer in `value`.
fn pointers(value: &Value, at: String, out: &mut Vec<String>) {
    out.push(at.clone());
    match value {
        Value::Object(map) => {
            for (k, v) in map {
                let key = k.replace('~', "~0").replace('/', "~1");
                pointers(v, format!("{at}/{key}"), out);
            }
        }
        Value::Array(items) => {
            for (i, v) in items.iter().enumerate() {
                pointers(v, format!("{at}/{i}"), out);
            }
        }
        _ => {}
    }
}

/// One to three mutations of `body`.
fn mutate(rng: &mut Rng, mut body: Value, strings: &[String], pool: &[String]) -> Value {
    for _ in 0..=rng.below(3) {
        let mut all = Vec::new();
        pointers(&body, String::new(), &mut all);
        let at = rng.pick(&all).clone();
        let (parent, last) = match at.rfind('/') {
            Some(i) => (at[..i].to_owned(), at[i + 1..].to_owned()),
            None => (String::new(), String::new()),
        };
        match rng.below(6) {
            // Replace a value.
            0 | 1 => {
                let new = random_value(rng, 2, strings, pool);
                if at.is_empty() {
                    body = new;
                } else if let Some(slot) = body.pointer_mut(&at) {
                    *slot = new;
                }
            }
            // Remove a field.
            2 => {
                if let Some(Value::Object(map)) = body.pointer_mut(&parent) {
                    map.remove(&last.replace("~1", "/").replace("~0", "~"));
                }
            }
            // Add a field nobody asked for.
            3 => {
                if let Some(Value::Object(map)) = body.pointer_mut(&at) {
                    let key = rng.pick(strings).clone();
                    map.insert(key, random_value(rng, 1, strings, pool));
                }
            }
            // An amount at the edge.
            4 => {
                if let Some(slot) = body.pointer_mut(&at) {
                    *slot = json!({"amount": rng.pick(strings).clone(),
                                   "currency": *rng.pick(&["ETB", "ETB", "USD", "etb"])});
                }
            }
            // An ID the service handed out.
            _ => {
                if let (Some(slot), false) = (body.pointer_mut(&at), pool.is_empty()) {
                    *slot = Value::String(rng.pick(pool).clone());
                }
            }
        }
    }
    body
}

/// An error and every cause under it: reqwest's own message alone is
/// "error sending request", which says nothing.
fn error_chain(error: &dyn std::error::Error) -> String {
    let mut text = error.to_string();
    let mut cause = error.source();
    while let Some(e) = cause {
        text.push_str(&format!(": {e}"));
        cause = e.source();
    }
    text
}

fn percent_encode(text: &str) -> String {
    text.bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || b"-._~:".contains(&b) {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}

/// The request examples of the contract's operations, and every account
/// reference they name.
struct Operation {
    method: String,
    path: String,
    example: Option<Value>,
    /// The tokens, one per role of the wallet client, allowed to call it.
    allowed: Vec<&'static str>,
}

fn operations(contract: &Contract) -> Result<Vec<Operation>, BoxError> {
    let permissions = mezgeb_api::auth::Permissions::built_in()?;
    let mut out = Vec::new();
    for (path, item) in contract.spec["paths"].as_object().ok_or("no paths")? {
        if path == "/oauth/token" {
            continue; // Form-encoded: fuzzed on its own below.
        }
        for (method, operation) in item.as_object().ok_or("bad path item")? {
            let example =
                operation["requestBody"]["content"]["application/json"]["example"].clone();
            let scopes: Vec<String> = operation["security"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|r| r["oauth2"].as_array())
                .flatten()
                .filter_map(|s| s.as_str().map(str::to_owned))
                .collect();
            let template = path.strip_prefix("/v1/operations/");
            let allowed = ROLE_TOKENS
                .iter()
                .filter(|(role, _)| {
                    permissions.roles().get(*role).is_some_and(|grant| {
                        scopes.iter().any(|s| grant.scopes.contains(s))
                            && template.is_none_or(|t| grant.templates.contains(t))
                    })
                })
                .map(|(_, token)| *token)
                .collect();
            out.push(Operation {
                method: method.to_uppercase(),
                path: path.clone(),
                example: (!example.is_null()).then_some(example),
                allowed,
            });
        }
    }
    Ok(out)
}

fn references(value: &Value, out: &mut BTreeSet<String>) {
    match value {
        Value::String(s) => {
            let code = s.split(':').next().unwrap_or("");
            if code.len() == 4 && code.bytes().all(|b| b.is_ascii_digit()) {
                out.insert(s.clone());
            }
        }
        Value::Array(items) => items.iter().for_each(|v| references(v, out)),
        Value::Object(map) => map.values().for_each(|v| references(v, out)),
        _ => {}
    }
}

/// IDs in a response, for later cases to use.
fn harvest(value: &Value, pool: &mut Vec<String>) {
    match value {
        Value::String(s) if s.len() == 36 && s.matches('-').count() == 4 => {
            if pool.len() < 500 && !pool.contains(s) {
                pool.push(s.clone());
            }
        }
        Value::Array(items) => items.iter().for_each(|v| harvest(v, pool)),
        Value::Object(map) => map.values().for_each(|v| harvest(v, pool)),
        _ => {}
    }
}

struct Case {
    method: String,
    url: String,
    token: Option<String>,
    key: Option<String>,
    body: Option<Vec<u8>>,
    describe: String,
}

fn build_case(
    rng: &mut Rng,
    n: u64,
    ops: &[Operation],
    strings: &[String],
    pool: &[String],
    keys: &[String],
) -> Case {
    let op = rng.pick(ops);
    // Path parameters.
    let mut path = String::new();
    for (i, part) in op.path.split('/').enumerate() {
        if i > 0 {
            path.push('/');
        }
        if part.starts_with('{') {
            let value = match rng.below(4) {
                0 if !pool.is_empty() => rng.pick(pool).clone(),
                0 | 1 => "0192f3c4-0000-7000-8000-00000000dead".to_owned(),
                2 => {
                    if part == "{template_id}" {
                        "p2p_transfer.v1".to_owned()
                    } else {
                        "1".to_owned()
                    }
                }
                _ => percent_encode(rng.pick(strings)),
            };
            path.push_str(&value);
        } else {
            path.push_str(part);
        }
    }
    if op.method == "GET" && rng.chance(50) {
        let query = *rng.pick(&[
            "limit=0",
            "limit=101",
            "limit=-1",
            "limit=abc",
            "limit=1&limit=2",
            "cursor=zzz",
            "cursor=",
            "from=2026-01-01T00:00:00Z&to=2025-01-01T00:00:00Z",
            "from=garbage",
            "reference=2010:cust-abebe",
            "status=weird",
            "%ff=1",
            "limit=1",
            "from=1970-01-01T00:00:00Z&to=2099-01-01T00:00:00Z",
        ]);
        path = format!("{path}?{query}");
    }
    // Mostly a token that may call the operation, so the case gets past
    // the scope check and into the handler and the ledger.
    let token = match rng.below(20) {
        0..=12 if !op.allowed.is_empty() => Some(rng.pick(&op.allowed).to_string()),
        0..=6 => Some(CLIENT.to_owned()),
        7..=11 => Some(OPERATOR.to_owned()),
        12 | 13 => Some(CHECKER.to_owned()),
        14..=16 => Some(rng.pick(&ROLE_TOKENS).1.to_owned()),
        17 => Some(OTHER.to_owned()),
        18 => None,
        _ => Some(
            rng.pick(strings)
                .chars()
                .filter(char::is_ascii_graphic)
                .collect(),
        ),
    };
    let key = (op.method == "POST").then(|| match rng.below(10) {
        0 | 1 if !keys.is_empty() => rng.pick(keys).clone(),
        2 => rng.pick(&["", " ", "a b", "../x", "k"]).to_string(),
        3 => "k".repeat(300),
        _ => format!("fz-{n}"),
    });
    let body = if op.method == "POST" {
        let raw: Option<Vec<u8>> = match rng.below(20) {
            0 => Some(b"{".to_vec()),
            1 => Some("[".repeat(10_000).into_bytes()),
            2 => Some(vec![b' '; 70_000]),
            3 => Some(vec![0xff, 0xfe, 0x00, 0x7b]),
            _ => None,
        };
        Some(raw.unwrap_or_else(|| {
            let start = op.example.clone().unwrap_or_else(|| json!({}));
            let value = if rng.chance(30) {
                start
            } else {
                mutate(rng, start, strings, pool)
            };
            value.to_string().into_bytes()
        }))
    } else {
        None
    };
    let shown = body
        .as_ref()
        .map(|b| String::from_utf8_lossy(&b[..b.len().min(300)]).into_owned())
        .unwrap_or_default();
    Case {
        describe: format!("{} {path} key={key:?} body={shown}", op.method),
        method: op.method.clone(),
        url: path,
        token,
        key,
        body,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn random_requests_never_get_a_5xx_or_break_an_invariant() -> Result<(), BoxError> {
    let Some(server) = TestServer::start(Duration::from_secs(60))
        .await
        .map_err(|e| e.to_string())?
    else {
        return Ok(());
    };
    let contract = Arc::new(Contract::load().map_err(|e| e.to_string())?);
    let api = common::Api::new(&server.base, contract.clone());
    let seed = env_number("MEZGEB_FUZZ_SEED", 1);
    let cases = env_number("MEZGEB_FUZZ_CASES", 1_500);
    let ops = operations(&contract)?;
    let strings = hostile_strings();

    // The accounts the examples name, open and funded, so that mutated
    // examples reach the ledger; offline payments on.
    let mut named = BTreeSet::new();
    for op in &ops {
        if let Some(example) = &op.example {
            references(example, &mut named);
        }
    }
    for reference in &named {
        let token = if reference.contains(':') {
            CLIENT
        } else {
            OPERATOR
        };
        api.post(
            "/v1/accounts",
            token,
            &format!("open-{reference}"),
            json!({"reference": reference}),
        )
        .await
        .ok();
    }
    for reference in named.iter().filter(|r| r.starts_with("2010:")) {
        api.post(
            "/v1/operations/deposit_confirmed.v1",
            CLIENT,
            &format!("fund-{reference}"),
            json!({"accounts": {"customer_funds": "1010", "customer_wallet": reference},
                   "amounts": {"amount": common::etb("1000.00")}}),
        )
        .await
        .ok();
    }
    let mut admin = server.db.admin().await?;
    mezgeb_engine::set_tier_limits(
        &mut admin,
        CLIENT_ID.parse()?,
        mezgeb_offline::Tier::C,
        Some(mezgeb_offline::Tier::C.maximum()),
    )
    .await?;
    drop(admin);

    // A new connection for every case. The server closes a connection
    // after refusing a body it did not read (one over the limit); a pooled
    // client can pick that connection up for its next request before it
    // sees the close, and that request then fails to send through no fault
    // of its own. Seen on CI: a small, valid request failing that way.
    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(120))
        .pool_max_idle_per_host(0)
        .build()?;
    let pool = Arc::new(Mutex::new(Vec::<String>::new()));
    let keys = Arc::new(Mutex::new(Vec::<String>::new()));
    let failures = Arc::new(Mutex::new(Vec::<String>::new()));
    let statuses = Arc::new(Mutex::new(std::collections::BTreeMap::<u16, u64>::new()));
    let mut rng = Rng(seed);
    let mut n = 0;
    while n < cases {
        let mut batch = Vec::new();
        for _ in 0..8.min(cases - n) {
            let case = {
                let pool = pool.lock().map(|p| p.clone()).unwrap_or_default();
                let keys = keys.lock().map(|k| k.clone()).unwrap_or_default();
                build_case(&mut rng, n, &ops, &strings, &pool, &keys)
            };
            let (http, base, contract) = (http.clone(), server.base.clone(), contract.clone());
            let (pool, keys, failures, statuses) = (
                pool.clone(),
                keys.clone(),
                failures.clone(),
                statuses.clone(),
            );
            let number = n;
            batch.push(tokio::spawn(async move {
                let method = reqwest::Method::from_bytes(case.method.as_bytes())?;
                let mut request = http.request(method, format!("{base}{}", case.url));
                if let Some(token) = &case.token {
                    request = request.bearer_auth(token);
                }
                if let Some(key) = &case.key {
                    request = request.header("Idempotency-Key", key);
                }
                if let Some(body) = case.body.clone() {
                    request = request
                        .header("Content-Type", "application/json")
                        .body(body);
                }
                let large = case.body.as_ref().is_some_and(|b| b.len() > 65_536);
                let response = match request.send().await {
                    Ok(response) => response,
                    // A body over the limit is refused before it is read,
                    // and the connection closed: the client, still sending,
                    // may see that instead of the answer. That is allowed;
                    // a closed connection for anything else is not.
                    Err(e) if large && e.is_request() => {
                        if let Ok(mut s) = statuses.lock() {
                            *s.entry(0).or_default() += 1;
                        }
                        return Ok(());
                    }
                    Err(e) => {
                        if let Ok(mut f) = failures.lock() {
                            f.push(format!(
                                "seed {seed} case {number}: {}\n  -> no answer: {}",
                                case.describe,
                                error_chain(&e)
                            ));
                        }
                        return Ok(());
                    }
                };
                let status = response.status().as_u16();
                let content_type = response
                    .headers()
                    .get("content-type")
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or("")
                    .to_owned();
                let text = response.text().await?;
                let body: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
                if let Ok(mut s) = statuses.lock() {
                    *s.entry(status).or_default() += 1;
                }
                let mut problems = Vec::new();
                if status >= 500 {
                    problems.push(format!("status {status}: {text}"));
                } else if let Err(e) =
                    contract.check(&case.method, &case.url, status, &content_type, &body)
                {
                    problems.push(e);
                }
                if !problems.is_empty() {
                    if let Ok(mut f) = failures.lock() {
                        f.push(format!(
                            "seed {seed} case {number}: {}\n  -> {}",
                            case.describe,
                            problems.join("; ")
                        ));
                    }
                } else if (200..300).contains(&status) {
                    if let Ok(mut p) = pool.lock() {
                        harvest(&body, &mut p);
                    }
                    if let (Some(key), Ok(mut k)) = (&case.key, keys.lock())
                        && k.len() < 200
                    {
                        k.push(key.clone());
                    }
                }
                Ok::<(), BoxError>(())
            }));
            n += 1;
        }
        for task in batch {
            task.await??;
        }
    }

    // The token endpoint, with random forms.
    for _ in 0..50 {
        let form = match rng.below(5) {
            0 => "grant_type=client_credentials".to_owned(),
            1 => format!("grant_type={}", percent_encode(rng.pick(&strings))),
            2 => "grant_type=client_credentials&scope=periods:close%20accounts:admin".to_owned(),
            3 => format!(
                "{}={}",
                percent_encode(rng.pick(&strings)),
                percent_encode(rng.pick(&strings))
            ),
            _ => String::from_utf8_lossy(&[0xff, b'=', 0x00]).into_owned(),
        };
        let basic = rng
            .chance(50)
            .then_some((common::CLIENT_CREDENTIAL, common::CLIENT_SECRET));
        match api.token(basic, &form).await {
            Ok(reply) if reply.status < 500 => {}
            Ok(reply) => failures.lock().map_err(|e| e.to_string())?.push(format!(
                "seed {seed} token form {form:?}: {} {}",
                reply.status, reply.body
            )),
            Err(e) => failures
                .lock()
                .map_err(|e| e.to_string())?
                .push(format!("seed {seed} token form {form:?}: {e}")),
        }
    }

    let statuses = statuses.lock().map(|s| s.clone()).unwrap_or_default();
    println!("seed {seed}, {cases} cases, statuses {statuses:?}");
    let failures = failures.lock().map(|f| f.clone()).unwrap_or_default();
    if !failures.is_empty() {
        let shown: Vec<&String> = failures.iter().take(25).collect();
        return Err(format!(
            "{} of {cases} cases failed; the first:\n{}",
            failures.len(),
            shown
                .iter()
                .map(|s| s.as_str())
                .collect::<Vec<_>>()
                .join("\n")
        )
        .into());
    }
    // Some cases must have reached the ledger, or the run proved little.
    let successes: u64 = statuses
        .iter()
        .filter(|(s, _)| (200..300).contains(*s))
        .map(|(_, c)| c)
        .sum();
    assert!(successes > cases / 20, "only {successes} cases succeeded");

    let mut verifier = server.db.verifier().await?;
    assert_eq!(mezgeb_engine::escrow_findings(&mut verifier).await?, vec![]);
    drop(verifier);
    // Full verification of balances, totals and chains.
    server.finish().await.map_err(|e| e.to_string())?;
    Ok(())
}
