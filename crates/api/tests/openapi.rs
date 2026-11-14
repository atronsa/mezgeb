//! The API contract, `openapi/mezgeb.v1.yaml`, checked against the code
//! (step 2.06). The contract is the promise to clients; these tests fail if
//! a template, an error rule or an engine operation drifts from it.
//!
//! The YAML is read as text: every template operation carries its shape on
//! one line, `x-mezgeb-template: {...}`, in JSON form (valid YAML).

use std::collections::BTreeSet;

use mezgeb_ops::{RELEASED, TemplateRegistry};

const SPEC: &str = include_str!("../../../openapi/mezgeb.v1.yaml");

type TestResult = Result<(), Box<dyn std::error::Error>>;

/// The lines of the `paths:` section.
fn path_section() -> &'static str {
    let start = SPEC.find("\npaths:\n").unwrap_or(0);
    let end = SPEC.find("\ncomponents:\n").unwrap_or(SPEC.len());
    &SPEC[start..end]
}

/// Each operation: its path, method and the text of its block.
fn operations() -> Vec<(String, String, String)> {
    let mut ops = Vec::new();
    let mut path = String::new();
    let mut current: Option<(String, String, Vec<&str>)> = None;
    for line in path_section().lines() {
        if let Some(p) = line.strip_prefix("  /").and_then(|p| p.strip_suffix(':')) {
            if let Some((m, p0, body)) = current.take() {
                ops.push((p0, m, body.join("\n")));
            }
            path = format!("/{p}");
        } else if let Some(method) = ["get", "post", "put", "patch", "delete"]
            .iter()
            .find(|m| line == format!("    {m}:"))
        {
            if let Some((m, p0, body)) = current.take() {
                ops.push((p0, m, body.join("\n")));
            }
            current = Some(((*method).to_owned(), path.clone(), Vec::new()));
        } else if let Some((_, _, body)) = current.as_mut() {
            body.push(line);
        }
    }
    if let Some((m, p0, body)) = current.take() {
        ops.push((p0, m, body.join("\n")));
    }
    ops
}

/// The block of a schema in `components/schemas`: its heading line and every
/// line indented deeper than it.
fn schema_block(name: &str) -> Option<String> {
    let heading = format!("    {name}:");
    let mut lines = SPEC.lines().skip_while(|l| *l != heading);
    let first = lines.next()?;
    let mut block = vec![first];
    for line in lines {
        let indent = line.len() - line.trim_start().len();
        if !line.trim().is_empty() && indent <= 4 {
            break;
        }
        block.push(line);
    }
    Some(block.join("\n"))
}

fn strings(value: &serde_json::Value, field: &str) -> Vec<String> {
    value[field]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

/// The acceptance criterion, part 1: every template is reachable, with the
/// exact shape the registry gives it.
#[test]
fn every_template_has_an_operation_with_its_exact_shape() -> TestResult {
    let registry = TemplateRegistry::standard();
    let documented: Vec<serde_json::Value> = SPEC
        .lines()
        .filter_map(|l| l.trim().strip_prefix("x-mezgeb-template: "))
        .map(serde_json::from_str)
        .collect::<Result<_, _>>()?;
    let ids: BTreeSet<String> = documented
        .iter()
        .filter_map(|d| d["id"].as_str().map(str::to_owned))
        .collect();
    let released: BTreeSet<String> = RELEASED.iter().map(|s| (*s).to_owned()).collect();
    assert_eq!(ids, released, "templates in the contract");
    assert_eq!(
        documented.len(),
        RELEASED.len(),
        "one operation per template"
    );

    for doc in &documented {
        let id = doc["id"].as_str().ok_or("no id")?;
        assert!(
            path_section().contains(&format!("\n  /v1/operations/{id}:\n")),
            "{id} has no operation path"
        );
        let roles = registry.roles(id)?;
        let required: Vec<String> = roles
            .iter()
            .filter(|r| !r.optional)
            .map(|r| r.name.to_owned())
            .collect();
        let optional: Vec<String> = roles
            .iter()
            .filter(|r| r.optional)
            .map(|r| r.name.to_owned())
            .collect();
        assert_eq!(strings(doc, "roles"), required, "{id} roles");
        assert_eq!(
            strings(doc, "optional_roles"),
            optional,
            "{id} optional roles"
        );
        assert_eq!(
            strings(doc, "amounts"),
            registry.amounts(id)?,
            "{id} amounts"
        );
        assert_eq!(
            strings(doc, "optional_amounts"),
            registry.optional_amounts(id)?,
            "{id} optional amounts"
        );
        let metadata: Vec<&str> = registry
            .metadata(id)?
            .iter()
            .filter(|m| m.required)
            .map(|m| m.name)
            .collect();
        assert_eq!(strings(doc, "metadata"), metadata, "{id} metadata");
        assert_eq!(
            doc["refunds"].as_bool(),
            Some(registry.refunds(id)?),
            "{id} refunds"
        );
        // The request schema names every role and amount.
        let schema = format!("{}V1Request", camel(id.trim_end_matches(".v1")));
        let block = schema_block(&schema).ok_or(format!("no {schema}"))?;
        for name in required
            .iter()
            .chain(&optional)
            .chain(&strings(doc, "amounts"))
        {
            assert!(
                block.contains(&format!(" {name}:")),
                "{schema} lacks {name}"
            );
        }
    }
    Ok(())
}

fn camel(name: &str) -> String {
    name.split('_')
        .map(|p| {
            let mut c = p.chars();
            c.next()
                .map(|f| f.to_uppercase().collect::<String>() + c.as_str())
                .unwrap_or_default()
        })
        .collect()
}

/// Every posting rule (1001–1012) and template rule (2001–2022) has an
/// error code and an example in the contract.
#[test]
fn every_rule_has_a_documented_error() {
    for rule in (1001..=1012).chain(2001..=2022) {
        assert!(
            SPEC.contains(&format!("\n        rule: {rule}\n")),
            "rule {rule} has no example"
        );
    }
}

/// Every error code in the catalogue has an example, and every example is
/// reachable from some response.
#[test]
fn every_error_code_has_an_example_used_by_a_response() -> TestResult {
    let start = SPEC.find("\n    ErrorCode:\n").ok_or("no ErrorCode")?;
    let block = &SPEC[start..];
    let block = &block[..block[1..].find("\n    Problem:").ok_or("no end")?];
    let codes: Vec<&str> = block
        .lines()
        .filter_map(|l| {
            l.trim()
                .strip_prefix("- \"")
                .and_then(|c| c.strip_suffix('"'))
        })
        .collect();
    assert!(codes.len() >= 60, "found {} codes", codes.len());
    for code in &codes {
        assert!(
            SPEC.contains(&format!("\n    error-{code}:\n")),
            "{code} has no example"
        );
        assert!(
            SPEC.contains(&format!("#/components/examples/error-{code}\"")),
            "no response uses the example of {code}"
        );
    }
    let docs = include_str!("../../../docs/api/errors.md");
    for code in &codes {
        assert!(
            docs.contains(&format!("| `{code}` |")),
            "{code} is not in errors.md"
        );
    }
    Ok(())
}

/// The acceptance criterion, part 2: no endpoint takes raw arbitrary lines,
/// except the operator-only, maker-checker adjustment request.
#[test]
fn only_the_adjustment_request_takes_raw_lines() {
    let mut request_schemas = BTreeSet::new();
    for (path, method, body) in operations() {
        if let Some(rb) = body.find("requestBody:") {
            let after = &body[rb..];
            if let Some(r) = after.find("#/components/schemas/") {
                let name: String = after[r + 21..]
                    .chars()
                    .take_while(|c| c.is_ascii_alphanumeric())
                    .collect();
                request_schemas.insert((name, format!("{method} {path}")));
            }
        }
    }
    let mut with_lines = Vec::new();
    for (schema, op) in &request_schemas {
        let Some(block) = schema_block(schema) else {
            continue;
        };
        if block.contains("\n        lines:") {
            with_lines.push(op.clone());
        }
    }
    assert_eq!(with_lines, ["post /v1/adjustments"]);
    let ops = operations();
    let adjustment = ops
        .iter()
        .find(|(p, m, _)| p == "/v1/adjustments" && m == "post")
        .map(|(_, _, b)| b.as_str())
        .unwrap_or_default();
    assert!(adjustment.contains("adjustments:request"));
    let approval = ops
        .iter()
        .find(|(p, _, _)| p == "/v1/adjustments/{adjustment_id}/approval")
        .map(|(_, _, b)| b.as_str())
        .unwrap_or_default();
    assert!(approval.contains("adjustments:approve"));
    assert!(approval.contains("maker_is_checker"));
}

/// Every operation needs a scope, and every write that posts an entry or
/// places a hold takes an Idempotency-Key.
#[test]
fn every_operation_is_scoped_and_every_posting_is_idempotent() {
    let ops = operations();
    assert!(ops.len() >= 40, "found {} operations", ops.len());
    for (path, method, body) in &ops {
        assert!(
            body.contains("security:\n        - oauth2:\n            - \""),
            "{method} {path} has no scope"
        );
        let exempt = path == "/v1/periods/close" || path.starts_with("/v1/offline/");
        if method == "post" && !exempt {
            assert!(
                body.contains("#/components/parameters/IdempotencyKey"),
                "{method} {path} takes no Idempotency-Key"
            );
        }
    }
}

/// The acceptance criterion, part 3: every engine operation is reachable.
#[test]
fn every_engine_operation_is_reachable() {
    for operation_id in [
        "openAccount",
        "getAccount",
        "listAccounts",
        "getAccountBalance",
        "getAccountLedger",
        "changeAccountStatus",
        "getEntry",
        "getEntryByIdempotencyKey",
        "reverseEntry",
        "requestAdjustment",
        "approveAdjustment",
        "rejectAdjustment",
        "placeHold",
        "getHold",
        "captureHold",
        "releaseHold",
        "listPeriods",
        "getCurrentPeriod",
        "closePeriod",
        "getTrialBalance",
        "getAgeing",
        "listTemplates",
        "registerDevice",
        "issueVoucher",
        "submitReceipts",
    ] {
        assert!(
            SPEC.contains(&format!("operationId: \"{operation_id}\"")),
            "{operation_id} is missing"
        );
    }
}
