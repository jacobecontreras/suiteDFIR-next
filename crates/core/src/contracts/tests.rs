//! Contract tests: round trips, schema versions, redaction, and fidelity to docs/CONTRACTS.md.
//!
//! The doc checks read docs/CONTRACTS.md itself (never the generated fixtures, so a stale fixture
//! is reported by the CI contracts-drift step, not here).

use std::collections::BTreeSet;
use std::fmt::Debug;

use serde_json::{Value, json};

use super::examples::{self, EXAMPLE_PASSWORD, Fixture};
use super::*;

const DOC: &str = include_str!("../../../../docs/CONTRACTS.md");

fn all_fixtures() -> Vec<Fixture> {
    examples::fixtures().expect("every example round-trips")
}

fn fixture(name: &str) -> Fixture {
    all_fixtures()
        .into_iter()
        .find(|f| f.name == name)
        .unwrap_or_else(|| panic!("no fixture named {name}"))
}

fn fixture_json(name: &str) -> Value {
    serde_json::from_str(&fixture(name).text).unwrap()
}

// ---- round trips ----

#[test]
fn every_fixture_round_trips_and_is_deterministic() {
    let first = all_fixtures();
    let second = all_fixtures();
    let names: BTreeSet<_> = first.iter().map(|f| f.name).collect();
    assert_eq!(names.len(), first.len(), "fixture names must be unique");
    for (a, b) in first.iter().zip(&second) {
        assert_eq!(a.text, b.text, "{} is not deterministic", a.name);
        assert!(
            a.text.ends_with("}\n") || a.text.ends_with("]\n"),
            "{}",
            a.name
        );
        // JSON → type → JSON is the identity.
        let json: Value = serde_json::from_str(&a.text).unwrap();
        assert_eq!(a.reserialize(json.clone()).unwrap(), json, "{}", a.name);
    }
}

/// Keys whose values are maps (free-form or enum keys) rather than structs; in the acquisition
/// records `tools` is a struct, which then just gets no extra field.
const MAP_KEYS: &[&str] = &["always_run", "files", "platforms", "tools"];

/// Adds an unknown field to every struct-like object in `value`.
fn add_unknown_fields(value: &mut Value, is_map: bool) {
    match value {
        Value::Object(map) => {
            for (key, child) in map.iter_mut() {
                add_unknown_fields(child, MAP_KEYS.contains(&key.as_str()));
            }
            if !is_map {
                map.insert("zz_unknown_field".into(), json!({"nested": [1, 2]}));
            }
        }
        Value::Array(items) => items.iter_mut().for_each(|v| add_unknown_fields(v, false)),
        _ => {}
    }
}

#[test]
fn unknown_fields_are_ignored_on_read() {
    for f in all_fixtures() {
        let json: Value = serde_json::from_str(&f.text).unwrap();
        let mut extended = json.clone();
        add_unknown_fields(&mut extended, false);
        let back = f
            .reserialize(extended)
            .unwrap_or_else(|e| panic!("{}: {e}", f.name));
        assert_eq!(back, json, "{}", f.name);
    }
}

#[test]
fn optional_values_are_written_as_null() {
    let initial = fixture_json("RunRecordInitial");
    for key in [
        "label",
        "started_at",
        "ended_at",
        "recovered_at",
        "duration_ms",
        "process",
        "leapp_result",
    ] {
        assert!(initial.get(key).is_some(), "{key} must be present");
    }
    assert_eq!(initial["process"], Value::Null);
    assert_eq!(initial["output"]["seal"]["manifest"], Value::Null);
    let settings = fixture_json("Settings");
    assert_eq!(settings.get("tools_dir"), Some(&Value::Null));
}

// ---- schema_version ----

fn check_schema_version<T: VersionedFile + PartialEq + Debug>(example: T) {
    let to_bytes = |v: &Value| serde_json::to_vec(v).unwrap();
    let mut json = serde_json::to_value(&example).unwrap();
    assert_eq!(json["schema_version"], json!(1), "{}", T::FILE);

    json["zz_future_field"] = json!(true);
    let parsed: T = parse_versioned(&to_bytes(&json)).unwrap();
    assert_eq!(parsed, example, "{}", T::FILE);

    // An unknown version is rejected before any other field is looked at.
    for version in [0, 2, 99] {
        let newer = json!({"schema_version": version, "something": "else"});
        match parse_versioned::<T>(&to_bytes(&newer)) {
            Err(e @ ContractError::UnsupportedSchemaVersion { .. }) => {
                let text = e.to_string();
                assert!(text.contains(T::FILE), "{text}");
                assert!(
                    text.contains(&format!("schema_version {version}")),
                    "{text}"
                );
                assert!(text.contains("only version 1"), "{text}");
            }
            other => panic!("{}: version {version}: {other:?}", T::FILE),
        }
    }
    for bad in [json!(null), json!("1"), json!(1.5), json!(-1)] {
        json["schema_version"] = bad.clone();
        assert!(
            matches!(
                parse_versioned::<T>(&to_bytes(&json)),
                Err(ContractError::MissingSchemaVersion { .. })
            ),
            "{}: {bad}",
            T::FILE
        );
    }
    json.as_object_mut().unwrap().remove("schema_version");
    assert!(matches!(
        parse_versioned::<T>(&to_bytes(&json)),
        Err(ContractError::MissingSchemaVersion { .. })
    ));
    assert!(matches!(
        parse_versioned::<T>(b"{not json"),
        Err(ContractError::Invalid { .. })
    ));
}

#[test]
fn every_file_format_checks_schema_version() {
    check_schema_version(examples::leapp_manifest());
    check_schema_version(examples::install_record());
    check_schema_version(examples::modules_file());
    check_schema_version(examples::settings());
    check_schema_version(examples::case_file());
    check_schema_version(examples::run_record());
    check_schema_version(examples::idevice_tools_manifest());
    check_schema_version(examples::acquisition_record());
    check_schema_version(examples::encryption_restore_record());
}

// ---- secrets ----

#[test]
fn passwords_are_redacted_in_debug_output() {
    let mut no_password = examples::run_request();
    no_password.itunes_password = None;
    let outputs = [
        format!("{:?}", examples::run_request()),
        format!("{:#?}", examples::run_request()),
        format!("{:?}", examples::acq_request()),
        format!("{:#?}", examples::acq_request()),
        format!("{:?}", examples::acq_restore_encryption_request()),
        format!("{:#?}", examples::acq_restore_encryption_request()),
    ];
    for out in &outputs {
        assert!(!out.contains(EXAMPLE_PASSWORD), "{out}");
        assert!(out.contains("<redacted>"), "{out}");
    }
    assert!(format!("{no_password:?}").contains("itunes_password: None"));
}

#[test]
fn passwords_appear_only_in_request_payloads() {
    let requests = ["RunRequest", "AcqRequest", "AcqRestoreEncryptionRequest"];
    for f in all_fixtures() {
        // The requests must carry the password (the check below is not vacuous); nothing else may.
        assert_eq!(
            f.text.contains(EXAMPLE_PASSWORD),
            requests.contains(&f.name),
            "{}",
            f.name
        );
    }
    let record = fixture_json("RunRecord");
    assert_eq!(record["options"]["password_supplied"], json!(true));
    let argv = record["command"]["argv"].as_array().unwrap();
    let flag = argv.iter().position(|a| a == "--itunes_password").unwrap();
    assert_eq!(argv[flag + 1], json!("<redacted>"));
    let acq = fixture_json("AcquisitionRecord");
    assert_eq!(acq["encryption"]["password_channel"], json!("env"));
}

// ---- settings_update tri-state ----

#[test]
fn settings_update_tools_dir_is_tri_state() {
    let parse = |text: &str| serde_json::from_str::<SettingsUpdateRequest>(text).unwrap();
    assert_eq!(parse("{}"), SettingsUpdateRequest::default());
    assert_eq!(parse("{}").tools_dir, ToolsDirUpdate::Unchanged);
    assert_eq!(
        parse(r#"{"tools_dir": null}"#).tools_dir,
        ToolsDirUpdate::Reset
    );
    assert_eq!(
        parse(r#"{"tools_dir": "/opt/tools"}"#).tools_dir,
        ToolsDirUpdate::Set("/opt/tools".into())
    );
    assert!(serde_json::from_str::<SettingsUpdateRequest>(r#"{"tools_dir": 5}"#).is_err());

    let write = |update: SettingsUpdateRequest| serde_json::to_value(update).unwrap();
    assert_eq!(write(SettingsUpdateRequest::default()), json!({}));
    let reset = SettingsUpdateRequest {
        tools_dir: ToolsDirUpdate::Reset,
        ..Default::default()
    };
    assert_eq!(write(reset), json!({"tools_dir": null}));
    let set = SettingsUpdateRequest {
        cases_root: Some("/cases".into()),
        tools_dir: ToolsDirUpdate::Set("/opt/tools".into()),
        ..Default::default()
    };
    assert_eq!(
        write(set),
        json!({"cases_root": "/cases", "tools_dir": "/opt/tools"})
    );
    let defaults = parse(r#"{"defaults": {"examiner": "A", "agency": "B", "timezone": "UTC"}}"#);
    assert_eq!(defaults.defaults.unwrap().examiner, "A");
    assert_eq!(defaults.tools_dir, ToolsDirUpdate::Unchanged);
}

// ---- the placeholder idevice-tools.json ----

#[test]
fn idevice_tools_placeholder_parses() {
    let manifest: IdeviceToolsManifest =
        parse_versioned(include_bytes!("../../../../idevice-tools.json")).unwrap();
    assert_eq!(manifest.version, "1.4.0");
    let names: Vec<_> = manifest.sources.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(
        names,
        [
            "libplist",
            "libimobiledevice-glue",
            "libusbmuxd",
            "libtatsu",
            "mbedtls",
            "libimobiledevice"
        ]
    );
    for source in &manifest.sources {
        assert!(source.url.starts_with("https://"), "{}", source.url);
        assert!(source.url.contains(&source.version), "{}", source.url);
        assert_eq!(source.sha256.len(), 64, "{}", source.name);
        assert!(
            source
                .sha256
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
            "{}",
            source.name
        );
    }
    assert!(manifest.platforms.is_empty());
    assert_eq!(
        manifest.system_platforms,
        [PlatformKey::LinuxX86_64, PlatformKey::LinuxAarch64]
    );
}

// ---- docs/CONTRACTS.md: file examples ----

/// The text of the section whose heading starts with `heading`, up to the next heading.
fn section(heading: &str) -> &'static str {
    let start = DOC
        .find(&format!("\n{heading}"))
        .unwrap_or_else(|| panic!("no section {heading}"))
        + 1;
    let body_start = start + DOC[start..].find('\n').unwrap() + 1;
    let end = DOC[body_start..]
        .find("\n#")
        .map_or(DOC.len(), |i| body_start + i);
    &DOC[body_start..end]
}

fn code_blocks<'a>(text: &'a str, lang: &str) -> Vec<&'a str> {
    let fence = format!("```{lang}\n");
    let mut blocks = vec![];
    let mut rest = text;
    while let Some(i) = rest.find(&fence) {
        let body = &rest[i + fence.len()..];
        let end = body.find("```").expect("closed code block");
        blocks.push(&body[..end]);
        rest = &body[end + 3..];
    }
    blocks
}

fn kind(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "bool",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

/// Compares key structure recursively. In the doc, `{ "…": "…" }` means "same shape as its
/// siblings" and matches any object; a `null` leaf matches any leaf.
fn compare_shape(doc: &Value, ours: &Value, path: &str, errors: &mut Vec<String>) {
    match (doc, ours) {
        (Value::Object(d), Value::Object(o)) => {
            if d.len() == 1 && d.keys().all(|k| k == "…" || k == "...") {
                return;
            }
            let dk: BTreeSet<_> = d.keys().collect();
            let ok: BTreeSet<_> = o.keys().collect();
            if dk != ok {
                errors.push(format!(
                    "{path}: only in doc {:?}, only in ours {:?}",
                    dk.difference(&ok).collect::<Vec<_>>(),
                    ok.difference(&dk).collect::<Vec<_>>()
                ));
            }
            for (key, dv) in d {
                if let Some(ov) = o.get(key) {
                    compare_shape(dv, ov, &format!("{path}.{key}"), errors);
                }
            }
        }
        (Value::Array(d), Value::Array(o)) => {
            if d.is_empty() != o.is_empty() {
                errors.push(format!(
                    "{path}: doc has {} items, ours {}",
                    d.len(),
                    o.len()
                ));
            }
            for (i, ov) in o.iter().enumerate() {
                if let Some(dv) = d.get(i).or(d.first()) {
                    compare_shape(dv, ov, &format!("{path}[{i}]"), errors);
                }
            }
        }
        (Value::Null, o) if !o.is_object() && !o.is_array() => {}
        (d, o) if kind(d) == kind(o) && !d.is_object() && !d.is_array() => {}
        _ => errors.push(format!(
            "{path}: doc has {}, ours {}",
            kind(doc),
            kind(ours)
        )),
    }
}

fn assert_same_shape(doc: &Value, ours: &Value, what: &str) {
    let mut errors = vec![];
    compare_shape(doc, ours, what, &mut errors);
    assert!(
        errors.is_empty(),
        "{what} differs from the doc:\n{}",
        errors.join("\n")
    );
}

fn doc_json(heading: &str, index: usize) -> Value {
    let block = code_blocks(section(heading), "json")[index];
    serde_json::from_str(block).unwrap_or_else(|e| panic!("{heading} block {index}: {e}"))
}

#[test]
fn file_examples_match_the_doc() {
    // (heading, JSON block index, fixture, whether the doc example parses as the type: false where
    // it uses `{ "…": "…" }` shorthand or `…` for a timestamp).
    let cases = [
        ("## 3.", 0, "LeappManifest", false),
        ("## 4.", 0, "InstallRecord", true),
        ("## 5.", 0, "ModulesFile", true),
        ("## 6.", 0, "Settings", true),
        ("## 6.", 1, "CaseFile", true),
        ("### 7.1", 0, "RunRecord", true),
        ("## 8.", 0, "LeappProfile", true),
        ("## 8.", 1, "LeappCaseData", true),
        ("### 13.2", 0, "IdeviceToolsManifest", false),
        ("### 13.3", 0, "AcquisitionRecord", false),
    ];
    for (heading, index, name, parses) in cases {
        let doc = doc_json(heading, index);
        let f = fixture(name);
        let ours: Value = serde_json::from_str(&f.text).unwrap();
        assert_same_shape(&doc, &ours, name);
        if parses {
            // The type reads the doc example and writes back every field it contains.
            let back = f
                .reserialize(doc.clone())
                .unwrap_or_else(|e| panic!("{name}: the doc example does not parse: {e}"));
            assert_same_shape(&doc, &back, name);
        }
    }
}

#[test]
fn initial_run_record_matches_7_2() {
    let ours = fixture_json("RunRecordInitial");
    // Same fields as the final record (§7.1); the values not known yet are null.
    let mut expected = doc_json("### 7.1", 0);
    for key in [
        "started_at",
        "ended_at",
        "duration_ms",
        "process",
        "leapp_result",
    ] {
        expected[key] = Value::Null;
    }
    for key in ["manifest", "manifest_sha256", "file_count", "total_bytes"] {
        expected["output"]["seal"][key] = Value::Null;
    }
    expected["status_reasons"] = json!([]);
    expected["warnings"] = json!([]);
    assert_same_shape(&expected, &ours, "RunRecordInitial");

    assert_eq!(ours["status"], json!("running"));
    assert_eq!(ours["status_reasons"], json!([]));
    assert_eq!(ours["warnings"], json!([]));
    for key in [
        "started_at",
        "ended_at",
        "recovered_at",
        "duration_ms",
        "process",
        "leapp_result",
    ] {
        assert_eq!(ours[key], Value::Null, "{key}");
    }
    let hash_status = ours["input"]["hash"]["status"].as_str().unwrap();
    assert!(
        ["pending", "not_requested", "not_applicable"].contains(&hash_status),
        "{hash_status}"
    );
    assert_eq!(
        ours["output"]["seal"],
        json!({"status": "pending", "manifest": null, "manifest_sha256": null,
               "file_count": null, "total_bytes": null})
    );
}

// ---- docs/CONTRACTS.md: enumerations and error codes ----

/// Backticked tokens of `text`, ignoring anything in parentheses.
fn backticked(text: &str) -> Vec<String> {
    let mut depth = 0;
    let bare: String = text
        .chars()
        .filter(|&c| {
            match c {
                '(' => depth += 1,
                ')' => depth -= 1,
                _ => return depth == 0,
            }
            false
        })
        .collect();
    bare.split('`')
        .skip(1)
        .step_by(2)
        .map(str::to_owned)
        .collect()
}

fn rust_enum_values(name: &str) -> Option<Vec<&'static str>> {
    macro_rules! lookup {
        ($($t:ident),+) => {
            match name {
                $(stringify!($t) => Some($t::ALL.iter().map(|v| v.as_str()).collect()),)+
                _ => None,
            }
        };
    }
    lookup!(
        ToolId,
        PlatformKey,
        InputKind,
        InputType,
        ModuleMode,
        RunStatus,
        HashStatus,
        SealStatus,
        InstallSource,
        EntryVerifiedAgainst,
        ToolState,
        RunPhase,
        AcqStatus,
        AcqPhase,
        PairState,
        IdeviceToolSource,
        IdeviceToolsState,
        ToolVerification,
        RestoreState,
        DevicePromptKind
    )
}

#[test]
fn enumerations_match_the_doc() {
    let mut seen = 0;
    for heading in ["## 2.", "### 13.1"] {
        for row in section(heading).lines().filter(|l| l.starts_with("| `")) {
            let cells: Vec<_> = row.split(" | ").collect();
            let name = backticked(cells[0]).remove(0);
            let doc_values = backticked(cells[1]);
            let ours = rust_enum_values(&name).unwrap_or_else(|| panic!("no Rust enum {name}"));
            assert_eq!(ours, doc_values, "{name}");
            seen += 1;
        }
    }
    assert_eq!(seen, 20);
}

#[test]
fn error_codes_match_the_doc() {
    let paragraph = section("## 12.").trim_start().split("\n\n").next().unwrap();
    let doc_codes = backticked(paragraph);
    let ours: Vec<_> = ErrorCode::ALL.iter().map(|c| c.as_str()).collect();
    assert_eq!(ours, doc_codes);
    assert_eq!(ours.len(), 45);
}

// ---- docs/CONTRACTS.md: TypeScript IPC types and command tables ----

/// A TypeScript type from the doc: an object literal (members in order: name, optional, type) or
/// anything else (kept as text).
#[derive(Debug)]
enum Ts {
    Object(Vec<(String, bool, Ts)>),
    Other(String),
}

/// Splits `text` at `sep` where it is not nested in brackets.
fn split_top(text: &str, sep: char) -> Vec<&str> {
    let (mut depth, mut start, mut parts) = (0i32, 0, vec![]);
    for (i, c) in text.char_indices() {
        match c {
            '{' | '[' | '(' | '<' => depth += 1,
            '}' | ']' | ')' | '>' => depth -= 1,
            _ if c == sep && depth == 0 => {
                parts.push(&text[start..i]);
                start = i + c.len_utf8();
            }
            _ => {}
        }
    }
    parts.push(&text[start..]);
    parts
}

fn parse_ts(text: &str, sep: char) -> Ts {
    let text = text.trim();
    match text.strip_prefix('{').and_then(|t| t.strip_suffix('}')) {
        Some(body) => Ts::Object(
            split_top(body, sep)
                .into_iter()
                .map(str::trim)
                .filter(|m| !m.is_empty())
                .map(|member| {
                    let (name, ty) = member.split_once(':').unwrap_or((member, ""));
                    let optional = name.trim().ends_with('?');
                    let name = name.trim().trim_end_matches('?').to_owned();
                    (name, optional, parse_ts(ty, sep))
                })
                .collect(),
        ),
        None => Ts::Other(text.to_owned()),
    }
}

/// The variants of a union of object literals (`| { … } | { … }`).
fn parse_union(text: &str) -> Vec<Ts> {
    split_top(text, '|')
        .into_iter()
        .filter(|v| !v.trim().is_empty())
        .map(|v| parse_ts(v, ';'))
        .collect()
}

fn compare_ts(doc: &Ts, ours: &Value, path: &str, errors: &mut Vec<String>) {
    let Ts::Object(members) = doc else { return };
    let Some(obj) = ours.as_object() else {
        errors.push(format!(
            "{path}: expected an object, ours is {}",
            kind(ours)
        ));
        return;
    };
    for (name, optional, ty) in members {
        match obj.get(name) {
            Some(value) => compare_ts(ty, value, &format!("{path}.{name}"), errors),
            None if *optional => {}
            None => errors.push(format!("{path}: missing {name}")),
        }
    }
    for key in obj.keys() {
        if !members.iter().any(|(name, _, _)| name == key) {
            errors.push(format!("{path}: {key} is not in the doc"));
        }
    }
}

/// Compares a union of tagged object literals with a fixture holding one example per variant.
fn compare_union(variants: &[Ts], ours: &Value, name: &str, errors: &mut Vec<String>) {
    let tag_of = |variant: &Ts| match variant {
        Ts::Object(members) => members.first().map(|(tag, _, ty)| match ty {
            Ts::Other(literal) => (tag.clone(), literal.trim_matches('"').to_owned()),
            Ts::Object(_) => (tag.clone(), String::new()),
        }),
        Ts::Other(_) => None,
    };
    let examples = ours.as_array().expect("union fixtures are arrays");
    let mut doc_tags = BTreeSet::new();
    for variant in variants {
        let (tag, value) = tag_of(variant).unwrap_or_else(|| panic!("{name}: untagged variant"));
        doc_tags.insert(value.clone());
        match examples.iter().find(|e| e[&tag] == json!(value)) {
            Some(example) => compare_ts(variant, example, &format!("{name}[{value}]"), errors),
            None => errors.push(format!("{name}: no example for {tag} = {value}")),
        }
    }
    if doc_tags.len() != examples.len() {
        errors.push(format!(
            "{name}: {} examples for {doc_tags:?}",
            examples.len()
        ));
    }
}

#[test]
fn ipc_types_match_the_doc() {
    let strip_comments = |block: &str| -> String {
        block
            .lines()
            .map(|l| l.find("//").map_or(l, |i| &l[..i]))
            .collect::<Vec<_>>()
            .join("\n")
    };
    let mut declarations: Vec<(String, String)> = vec![];
    for heading in ["## 9.", "### 13.5"] {
        for block in code_blocks(section(heading), "ts") {
            for decl in split_top(&strip_comments(block), ';') {
                if let Some((name, expr)) = decl
                    .trim()
                    .strip_prefix("type ")
                    .and_then(|d| d.split_once('='))
                {
                    declarations.push((name.trim().to_owned(), expr.to_owned()));
                }
            }
        }
    }
    // §11 lists the event unions without a `type` line.
    let events = code_blocks(section("## 11."), "ts");
    assert_eq!(events.len(), 2);
    declarations.push(("RunEvent".into(), strip_comments(events[0])));
    declarations.push(("InstallEvent".into(), strip_comments(events[1])));

    let mut errors = vec![];
    for (name, expr) in &declarations {
        let ours = fixture_json(name);
        if expr.trim_start().starts_with('|') {
            compare_union(&parse_union(expr), &ours, name, &mut errors);
        } else {
            compare_ts(&parse_ts(expr, ';'), &ours, name, &mut errors);
        }
    }
    assert!(errors.is_empty(), "{}", errors.join("\n"));
    assert_eq!(declarations.len(), 22);
}

/// Every command in CONTRACTS.md §10 and §13.5 with the fixtures of its `req` and of its return
/// value when that is an inline object (named return types are checked by `ipc_types_match_the_doc`).
const COMMANDS: &[(&str, Option<&str>, Option<&str>)] = &[
    ("app_info", None, Some("AppInfo")),
    ("licenses_get", None, None),
    ("settings_get", None, None),
    ("settings_update", Some("SettingsUpdateRequest"), None),
    ("tools_status", None, None),
    ("tool_verify", Some("ToolRequest"), None),
    ("tool_install", Some("ToolRequest"), None),
    ("tool_import", Some("ToolImportRequest"), None),
    ("tool_modules", Some("ToolRequest"), None),
    ("cases_list", None, None),
    ("case_create", Some("CaseCreateRequest"), None),
    ("case_open", Some("PathRequest"), None),
    ("case_update", Some("CaseUpdateRequest"), None),
    ("case_forget", Some("PathRequest"), None),
    ("run_get", Some("RunRef"), None),
    ("input_inspect", Some("InputInspectRequest"), None),
    ("ios_backups_find", None, None),
    ("profiles_list", Some("ToolRequest"), None),
    ("profile_save", Some("ProfileSaveRequest"), None),
    ("profile_delete", Some("ProfileRef"), None),
    ("profile_import", Some("ProfileImportRequest"), None),
    ("profile_export", Some("ProfileExportRequest"), None),
    ("run_start", Some("RunRequest"), Some("RunStarted")),
    ("run_cancel", Some("RunCancelRequest"), None),
    ("job_active", None, None),
    ("job_attach", Some("JobAttachRequest"), Some("JobBacklog")),
    ("open_report", Some("RunRef"), None),
    ("reveal_path", Some("PathRequest"), None),
    ("open_text_file", Some("OpenTextFileRequest"), None),
    ("temp_cleanup", None, Some("TempCleanupResult")),
    ("devices_list", None, None),
    ("device_pair", Some("DevicePairRequest"), None),
    ("acq_preflight", Some("AcqPreflightRequest"), None),
    ("acq_start", Some("AcqRequest"), Some("AcqStarted")),
    ("acq_cancel", Some("AcqCancelRequest"), None),
    ("acq_get", Some("AcqRef"), None),
    (
        "acq_restore_encryption",
        Some("AcqRestoreEncryptionRequest"),
        Some("AcqRestoreEncryptionResult"),
    ),
    ("open_acq_file", Some("OpenAcqFileRequest"), None),
];

/// Checks one `req` or return cell of a command table against its fixture.
fn check_command_cell(
    command: &str,
    cell: &str,
    fixture_name: Option<&str>,
    errors: &mut Vec<String>,
) {
    let code = backticked(cell).into_iter().next();
    let Some(code) = code.filter(|c| c != "none" && c != "string") else {
        if let Some(name) = fixture_name {
            errors.push(format!("{command}: doc has no payload, ours is {name}"));
        }
        return;
    };
    if code.starts_with('{') {
        let Some(name) = fixture_name else {
            return errors.push(format!("{command}: no fixture for {code}"));
        };
        compare_ts(&parse_ts(&code, ','), &fixture_json(name), command, errors);
    } else {
        // A named type: `RunRequest`, `ToolStatus[]`, `ActiveJob | null`, …
        let named = code.split(['[', ' ']).next().unwrap_or_default();
        match fixture_name {
            Some(name) if name != named => {
                errors.push(format!("{command}: doc names {named}, ours is {name}"))
            }
            _ if all_fixtures().iter().all(|f| f.name != named) => {
                errors.push(format!("{command}: no fixture for {named}"))
            }
            _ => {}
        }
    }
}

#[test]
fn command_tables_match_the_doc() {
    let mut errors = vec![];
    let mut seen = BTreeSet::new();
    for heading in ["## 10.", "### 13.5"] {
        for row in section(heading).lines().filter(|l| l.starts_with("| `")) {
            let row = row.replace("\\|", "¦");
            let cells: Vec<String> = row
                .split('|')
                .map(|c| c.replace('¦', "|").trim().to_owned())
                .collect();
            let command = backticked(&cells[1]).remove(0);
            let Some(&(_, req, ret)) = COMMANDS.iter().find(|(name, _, _)| *name == command) else {
                errors.push(format!("{command}: not in COMMANDS"));
                continue;
            };
            check_command_cell(&command, &cells[2], req, &mut errors);
            check_command_cell(&command, &cells[3], ret, &mut errors);
            seen.insert(command);
        }
    }
    assert!(errors.is_empty(), "{}", errors.join("\n"));
    assert_eq!(seen.len(), COMMANDS.len());
}
