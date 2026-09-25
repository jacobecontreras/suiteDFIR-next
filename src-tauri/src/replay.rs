//! E2 replay (ROADMAP E2): every invoke that `ui/api/ipc.js` made in the recorded UI flow
//! (`tests/ui/recorded-invokes.json`, written by `scripts/record-invokes.mjs`) goes through the real
//! command handlers on Tauri's mock runtime (`tauri::test`), with both dev overrides: fake-leapp
//! for iLEAPP and fake-idevice for the iOS tools. aLEAPP is installed by the real install pipeline
//! (download → verify → extract → verify → introspect) from a test archive of a probe-answering
//! fake-leapp copy, pinned by a test manifest; the network download is replaced by that local file.
//! The opener records instead of opening.
//!
//! Every response must succeed and match its contract type exactly (deserialized into the Rust
//! type and serialized back to the same JSON), and so must every event sent on a channel.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;
use suitedfir_core::contracts::{
    AcqEvent, AcqPreflight, AcqRestoreEncryptionResult, AcqStarted, AcqStatus, AcquisitionRecord,
    ActiveJob, AppInfo, ArchiveKind, CaseDetail, CaseSummary, DeviceSummary, DevicesResult,
    InputInspection, InstallEvent, InstallSource, IosBackup, JobBacklog, PairState, PlatformAsset,
    ProfileInfo, RunEvent, RunRecord, RunStarted, RunStatus, Settings, TempCleanupResult, ToolId,
    ToolModules, ToolState, ToolStatus,
};
use suitedfir_core::{hashing, manifest};
use tauri::ipc::{CallbackFn, InvokeBody, InvokeResponseBody};
use tauri::test::{INVOKE_KEY, get_ipc_response, mock_builder, mock_context, noop_assets};
use tauri::webview::InvokeRequest;

use crate::opener::testing::Opened;
use crate::testing::{EXE, LabOptions, UDID, core_binary, lab};

const RECORDED: &str = include_str!("../../tests/ui/recorded-invokes.json");
const CONTRACTS: &str = include_str!("../../docs/CONTRACTS.md");
const WAIT: Duration = Duration::from_secs(120);

// ---- a stored zip of the aLEAPP stand-in ----

fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &byte in data {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
        }
    }
    !crc
}

/// A zip with one stored (uncompressed) entry.
fn stored_zip(name: &str, data: &[u8]) -> Vec<u8> {
    let crc = crc32(data);
    let size = u32::try_from(data.len()).unwrap();
    let name_len = u16::try_from(name.len()).unwrap();
    let mut zip = Vec::new();
    let header = |zip: &mut Vec<u8>| {
        zip.extend(20u16.to_le_bytes()); // version needed
        zip.extend(0u16.to_le_bytes()); // flags
        zip.extend(0u16.to_le_bytes()); // stored
        zip.extend(0u16.to_le_bytes()); // time
        zip.extend(0x21u16.to_le_bytes()); // date: 1980-01-01
        zip.extend(crc.to_le_bytes());
        zip.extend(size.to_le_bytes());
        zip.extend(size.to_le_bytes());
        zip.extend(name_len.to_le_bytes());
        zip.extend(0u16.to_le_bytes()); // extra
    };
    zip.extend(0x0403_4b50u32.to_le_bytes());
    header(&mut zip);
    zip.extend(name.as_bytes());
    zip.extend(data);
    let directory = u32::try_from(zip.len()).unwrap();
    zip.extend(0x0201_4b50u32.to_le_bytes());
    zip.extend(20u16.to_le_bytes()); // version made by
    header(&mut zip);
    zip.extend(0u16.to_le_bytes()); // comment
    zip.extend(0u16.to_le_bytes()); // disk
    zip.extend(0u16.to_le_bytes()); // internal attributes
    zip.extend(0u32.to_le_bytes()); // external attributes
    zip.extend(0u32.to_le_bytes()); // local header offset
    zip.extend(name.as_bytes());
    let directory_size = u32::try_from(zip.len()).unwrap() - directory;
    zip.extend(0x0605_4b50u32.to_le_bytes());
    zip.extend(0u16.to_le_bytes());
    zip.extend(0u16.to_le_bytes());
    zip.extend(1u16.to_le_bytes());
    zip.extend(1u16.to_le_bytes());
    zip.extend(directory_size.to_le_bytes());
    zip.extend(directory.to_le_bytes());
    zip.extend(0u16.to_le_bytes());
    zip
}

// ---- placeholders ----

/// What the placeholders of the recording stand for.
#[derive(Default)]
struct Values {
    map: BTreeMap<String, String>,
    runs: Vec<String>,
    acqs: Vec<String>,
}

impl Values {
    /// Fills in the placeholders of a string; paths get the OS separator.
    fn fill(&self, text: &str) -> String {
        let mut out = text.to_owned();
        for (key, value) in &self.map {
            out = out.replace(key, value);
        }
        for (i, id) in self.runs.iter().enumerate() {
            out = out.replace(&format!("<RUN_{}>", i + 1), id);
        }
        for (i, id) in self.acqs.iter().enumerate() {
            out = out.replace(&format!("<ACQ_{}>", i + 1), id);
        }
        assert!(!out.contains('<') || !out.contains('>'), "unfilled: {text}");
        if cfg!(windows) && (text.starts_with("<ROOT>") || text.starts_with("<CASE>")) {
            out = out.replace('/', "\\");
        }
        out
    }

    fn fill_value(&self, value: &Value) -> Value {
        match value {
            Value::String(text) => Value::String(self.fill(text)),
            Value::Array(items) => Value::Array(items.iter().map(|v| self.fill_value(v)).collect()),
            Value::Object(map) => Value::Object(
                map.iter()
                    .map(|(k, v)| (k.clone(), self.fill_value(v)))
                    .collect(),
            ),
            other => other.clone(),
        }
    }
}

/// Deserializes into the contract type and back: the JSON must be exactly the type's.
fn contract<T: DeserializeOwned + Serialize>(what: &str, value: &Value) -> T {
    let typed: T = serde_json::from_value(value.clone())
        .unwrap_or_else(|e| panic!("{what} does not match its contract type: {e}\n{value:#}"));
    assert_eq!(
        serde_json::to_value(&typed).unwrap(),
        *value,
        "{what} has fields its contract type does not"
    );
    typed
}

/// The command names of CONTRACTS.md §10 and §13.5 (the first column of their tables).
fn contract_commands() -> Vec<String> {
    let section = |heading: &str| -> Vec<String> {
        let start = CONTRACTS.find(heading).unwrap();
        let rest = &CONTRACTS[start + heading.len()..];
        let end = rest
            .find("\n## ")
            .into_iter()
            .chain(rest.find("\n### "))
            .min()
            .unwrap_or(rest.len());
        rest[..end]
            .lines()
            .filter_map(|line| line.strip_prefix("| `"))
            .filter_map(|line| line.split_once("` |").map(|(name, _)| name.to_owned()))
            .filter(|name| name.bytes().all(|b| b.is_ascii_lowercase() || b == b'_'))
            .collect()
    };
    let mut commands = section("\n## 10. IPC commands");
    commands.extend(section("\n### 13.5 "));
    commands
}

/// Events sent on the channels, by channel id.
type Sent = Arc<Mutex<BTreeMap<u32, Vec<Value>>>>;

/// Waits until one of `channels` got an event of type `kind`.
fn wait_for_event(sent: &Sent, channels: &[u32], kind: &str) {
    let deadline = Instant::now() + WAIT;
    loop {
        let done = {
            let sent = sent.lock().unwrap();
            channels.iter().any(|id| {
                sent.get(id)
                    .is_some_and(|events| events.iter().any(|e| e["type"] == kind))
            })
        };
        if done {
            return;
        }
        assert!(Instant::now() < deadline, "no {kind} event on {channels:?}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn wait_for_finished(sent: &Sent, channels: &[u32]) {
    wait_for_event(sent, channels, "finished");
}

#[test]
fn every_recorded_invoke_succeeds_through_the_real_handlers() {
    // The aLEAPP stand-in: a probe-answering fake-leapp copy, pinned by a test manifest.
    let platform = manifest::host_platform().expect("this host has a platform key");
    let fake_path = core_binary("fake-leapp");
    let fake = fs::read(&fake_path).unwrap();
    let entry = format!("fake-leapp-probe{EXE}");
    let archive = stored_zip(&entry, &fake);
    let placeholder = tempfile::Builder::new().prefix("sdr").tempdir().unwrap();
    let zip_path = placeholder.path().join("aleapp-replay.zip");
    fs::write(&zip_path, &archive).unwrap();
    let mut pinned = manifest::embedded().unwrap().clone();
    pinned
        .tools
        .get_mut(&ToolId::Aleapp)
        .unwrap()
        .platforms
        .insert(
            platform,
            PlatformAsset {
                asset_name: "aleapp-replay.zip".to_owned(),
                asset_size: archive.len() as u64,
                asset_sha256: hashing::sha256_file(&zip_path).unwrap(),
                archive_kind: ArchiveKind::Zip,
                entry: entry.clone(),
                entry_sha256: Some(hashing::sha256_file(&fake_path).unwrap()),
                urls: vec!["https://example.invalid/aleapp-replay.zip".to_owned()],
            },
        );
    let lab = lab(LabOptions {
        leapp_override: vec![ToolId::Ileapp],
        manifest: pinned,
        download_from: Some(zip_path.clone()),
        idevice_scenario: "not_paired".to_owned(),
    });
    let root = lab.root.path().to_path_buf();
    fs::copy(&zip_path, root.join("aleapp-replay.zip")).unwrap();
    for input in ["fs", "slow"] {
        let dir = root.join("evidence").join(input);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("evidence.txt"), "evidence").unwrap();
    }
    fs::write(
        root.join("Imported.alprofile"),
        r#"{"leapp": "aleapp", "format_version": 1, "plugins": ["smsMms", "noLongerThere"]}"#,
    )
    .unwrap();

    // The mock app with the real handlers; channel messages are captured by id.
    let sent: Sent = Arc::default();
    let sink = Arc::clone(&sent);
    let app = mock_builder()
        .invoke_handler(crate::commands::handler())
        .channel_interceptor(move |_webview, callback: CallbackFn, _index, body| {
            let value = match body {
                InvokeResponseBody::Json(json) => serde_json::from_str(json).unwrap(),
                InvokeResponseBody::Raw(bytes) => serde_json::from_slice(bytes).unwrap(),
            };
            sink.lock()
                .unwrap()
                .entry(callback.0)
                .or_default()
                .push(value);
            true
        })
        .manage(Arc::clone(&lab.state))
        .build(mock_context(noop_assets()))
        .unwrap();
    let webview = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
        .build()
        .unwrap();

    let recorded: Vec<Value> = serde_json::from_str(RECORDED).unwrap();
    let called: Vec<String> = recorded
        .iter()
        .map(|r| r["cmd"].as_str().unwrap().to_owned())
        .collect();
    for command in contract_commands() {
        assert!(
            called.contains(&command),
            "{command} is not in the recording"
        );
    }
    assert_eq!(contract_commands().len(), 38);

    let mut values = Values::default();
    values
        .map
        .insert("<ROOT>".to_owned(), root.to_string_lossy().into_owned());
    values.map.insert("<UDID>".to_owned(), UDID.to_owned());
    // The channels of each job id, and each channel's event type.
    let mut job_channels: BTreeMap<String, Vec<u32>> = BTreeMap::new();
    let mut channel_kinds: BTreeMap<u32, &str> = BTreeMap::new();
    let url = if cfg!(windows) {
        "http://tauri.localhost"
    } else {
        "tauri://localhost"
    };

    for (step, invoke) in recorded.iter().enumerate() {
        let cmd = invoke["cmd"].as_str().unwrap();
        let args = values.fill_value(&invoke["args"]);
        let req = &args["req"];
        let channel = args["onEvent"].as_str().map(|c| {
            c.strip_prefix("__CHANNEL__:")
                .unwrap()
                .parse::<u32>()
                .unwrap()
        });
        let what = format!("step {step} {cmd}");

        // Pick the fake scenarios as the mock does: by input folder name and label suffix.
        match cmd {
            "run_start" => {
                let slow = Path::new(req["input_path"].as_str().unwrap()).ends_with("slow");
                let mut env = lab.state.leapp_env.lock().unwrap();
                env.retain(|(name, _)| name != "FAKE_LEAPP_SCENARIO");
                env.push((
                    "FAKE_LEAPP_SCENARIO".into(),
                    (if slow { "slow" } else { "success" }).into(),
                ));
            }
            "acq_start" => {
                if let Some((_, scenario)) = req["label"].as_str().unwrap().rsplit_once('/') {
                    lab.state.replace_idevice(lab.idevice_config(scenario));
                }
            }
            "acq_restore_encryption" => lab.state.replace_idevice(lab.idevice_config("success")),
            // Attach once the job has logged something, so there is a backlog to return.
            "job_attach" => {
                wait_for_event(&sent, &job_channels[req["id"].as_str().unwrap()], "log");
            }
            _ => {}
        }
        if let Some(id) = channel {
            let kind = match cmd {
                "tool_install" | "tool_import" => "install",
                "run_start" => "run",
                "acq_start" => "acquisition",
                _ => {
                    if req["kind"] == "run" {
                        "run"
                    } else {
                        "acquisition"
                    }
                }
            };
            channel_kinds.insert(id, kind);
        }

        let response = get_ipc_response(
            &webview,
            InvokeRequest {
                cmd: cmd.to_owned(),
                callback: CallbackFn(0),
                error: CallbackFn(1),
                url: url.parse().unwrap(),
                body: InvokeBody::Json(args.clone()),
                headers: Default::default(),
                invoke_key: INVOKE_KEY.to_owned(),
            },
        )
        .unwrap_or_else(|error| panic!("{what} failed: {error:#}\nargs: {args:#}"))
        .deserialize::<Value>()
        .unwrap();

        match cmd {
            "app_info" => {
                let info: AppInfo = contract(&what, &response);
                assert!(info.dev_override);
            }
            "licenses_get" => {
                let text: String = contract(&what, &response);
                assert_eq!(text, crate::ops::LICENSES);
            }
            "settings_get" | "settings_update" => {
                contract::<Settings>(&what, &response);
            }
            "tools_status" => {
                let tools: Vec<ToolStatus> = contract(&what, &response);
                assert_eq!(tools.len(), 2);
            }
            "tool_verify" | "tool_install" | "tool_import" => {
                let status: ToolStatus = contract(&what, &response);
                match status.tool {
                    ToolId::Ileapp => assert_eq!(status.state, ToolState::DevOverride),
                    ToolId::Aleapp => {
                        assert_eq!(status.state, ToolState::Verified, "{status:?}");
                        // Also for tool_install: the test stands in for the network with a file.
                        assert_eq!(status.install_source, Some(InstallSource::OfflineImport));
                        assert!(status.module_count.unwrap() >= 500);
                    }
                }
            }
            "tool_modules" => {
                contract::<ToolModules>(&what, &response);
            }
            "cases_list" => {
                contract::<Vec<CaseSummary>>(&what, &response);
            }
            "case_create" => {
                let detail: CaseDetail = contract(&what, &response);
                values.map.insert("<CASE>".to_owned(), detail.path);
            }
            "case_update" | "case_open" => {
                contract::<CaseDetail>(&what, &response);
            }
            "run_get" => {
                let record: RunRecord = contract(&what, &response);
                assert_eq!(record.status, RunStatus::Succeeded);
                assert_eq!(record.tool.install_source, InstallSource::OfflineImport);
                assert_eq!(record.modules.resolved, ["callLogs"]);
            }
            "input_inspect" => {
                contract::<InputInspection>(&what, &response);
            }
            "ios_backups_find" => {
                contract::<Vec<IosBackup>>(&what, &response);
            }
            "profiles_list" => {
                contract::<Vec<ProfileInfo>>(&what, &response);
            }
            "profile_save" | "profile_import" => {
                contract::<ProfileInfo>(&what, &response);
            }
            "run_start" => {
                let started: RunStarted = contract(&what, &response);
                let id = channel.unwrap();
                job_channels.insert(started.run_id.clone(), vec![id]);
                let slow = Path::new(req["input_path"].as_str().unwrap()).ends_with("slow");
                values.runs.push(started.run_id);
                if !slow {
                    wait_for_finished(&sent, &[id]);
                }
            }
            "job_active" => {
                let job: Option<ActiveJob> = contract(&what, &response);
                assert!(job.is_some(), "{what}: no active job");
            }
            "job_attach" => {
                let backlog: JobBacklog = contract(&what, &response);
                let id = req["id"].as_str().unwrap().to_owned();
                assert!(!backlog.backlog.is_empty(), "{what}: no backlog");
                job_channels.entry(id).or_default().push(channel.unwrap());
            }
            "run_cancel" | "acq_cancel" => {
                assert_eq!(response, Value::Null, "{what}");
                let id = req["run_id"].as_str().or(req["acq_id"].as_str()).unwrap();
                wait_for_finished(&sent, &job_channels[id]);
            }
            "temp_cleanup" => {
                contract::<TempCleanupResult>(&what, &response);
            }
            "devices_list" => {
                let devices: DevicesResult = contract(&what, &response);
                assert_eq!(devices.devices[0].pair_state, PairState::NotPaired);
            }
            "device_pair" => {
                contract::<DeviceSummary>(&what, &response);
            }
            "acq_preflight" => {
                contract::<AcqPreflight>(&what, &response);
            }
            "acq_start" => {
                let started: AcqStarted = contract(&what, &response);
                let id = channel.unwrap();
                job_channels.insert(started.acq_id.clone(), vec![id]);
                let slow = req["label"].as_str().unwrap().ends_with("/slow");
                values.acqs.push(started.acq_id);
                if !slow {
                    wait_for_finished(&sent, &[id]);
                }
            }
            "acq_get" => {
                let record: AcquisitionRecord = contract(&what, &response);
                assert_eq!(record.status, AcqStatus::Succeeded);
            }
            "acq_restore_encryption" => {
                let result: AcqRestoreEncryptionResult = contract(&what, &response);
                assert!(result.restored, "{result:?}");
            }
            // Commands without a result.
            _ => assert_eq!(response, Value::Null, "{what}"),
        }
    }

    // Every event sent on a channel matches its contract type; jobs ended with `finished`.
    let sent = sent.lock().unwrap();
    for (id, kind) in &channel_kinds {
        let events = sent.get(id).map(Vec::as_slice).unwrap_or_default();
        for event in events {
            let what = format!("a {kind} event on channel {id}");
            match *kind {
                "install" => {
                    contract::<InstallEvent>(&what, event);
                }
                "run" => {
                    contract::<RunEvent>(&what, event);
                }
                _ => {
                    contract::<AcqEvent>(&what, event);
                }
            }
        }
    }
    let finished = |job: &str| -> Value {
        let events: Vec<&Value> = job_channels[job]
            .iter()
            .flat_map(|id| sent.get(id).map(Vec::as_slice).unwrap_or_default())
            .filter(|e| e["type"] == "finished")
            .collect();
        assert_eq!(events.len(), 1, "{job}: {events:?}");
        events[0].clone()
    };
    assert_eq!(job_channels.len(), 5);
    for (job, status) in [
        (&values.runs[0], "cancelled"),
        (&values.runs[1], "succeeded"),
        (&values.acqs[0], "succeeded"),
        (&values.acqs[1], "cancelled"),
        (&values.acqs[2], "succeeded"),
    ] {
        assert_eq!(finished(job)["status"], status, "{job}");
    }
    // The acquisition that left encryption on, which the later restore then turned off.
    let warnings = finished(&values.acqs[2])["warnings"].clone();
    let codes: Vec<&str> = warnings
        .as_array()
        .unwrap()
        .iter()
        .map(|w| w["code"].as_str().unwrap())
        .collect();
    assert!(codes.contains(&"encryption_left_enabled"), "{codes:?}");
    // The install channels saw the stages up to `done`.
    for (id, kind) in &channel_kinds {
        if *kind == "install" {
            let last = sent[id].last().unwrap();
            assert_eq!(last, &serde_json::json!({"type": "stage", "stage": "done"}));
        }
    }

    // The opener was asked, never the OS.
    let case = PathBuf::from(&values.map["<CASE>"]);
    let run_dir = case.join("runs").join(&values.runs[1]);
    let acq_dir = case.join("acquisitions").join(&values.acqs[0]);
    assert_eq!(
        lab.opener.calls(),
        [
            Opened::Open(run_dir.join("report").join("index.html")),
            Opened::Open(run_dir.join("leapp.stdout.log")),
            Opened::Reveal(run_dir.clone()),
            Opened::Open(acq_dir.join("acquisition.json")),
        ]
    );
    drop(placeholder);
}
