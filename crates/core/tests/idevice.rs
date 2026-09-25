//! `idevice` integration tests with fake-idevice (ROADMAP X2, CONTRACTS.md §13.4): polling never
//! pairs, the pair-state transitions, the tools states, single-flight polling, and passwords that
//! travel only through the environment.

mod common;

use std::fs;
use std::process::Command;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use common::{FAKE, Lab, UDID};
use suitedfir_core::contracts::{
    DevicePromptKind, ErrorCode, IdeviceToolSource, IdeviceToolsState, PairState, ToolVerification,
};
use suitedfir_core::idevice::{
    Idevice, IdeviceConfig, IdeviceError, OutputLine, Password, ToolLookup, embedded_manifest,
};

/// Calls of `tool` with `arg` among the fake's recorded invocations.
fn count(calls: &[Vec<String>], tool: &str, arg: &str) -> usize {
    calls
        .iter()
        .filter(|call| call[0] == tool && call.iter().any(|a| a == arg))
        .count()
}

#[test]
fn polling_an_unpaired_device_never_pairs_it() {
    let lab = Lab::new("not_paired");
    for _ in 0..3 {
        let result = lab.idevice.list_devices(None);
        assert_eq!(result.tools.state, IdeviceToolsState::Ok);
        assert_eq!(result.tools.source, Some(IdeviceToolSource::Bundled));
        assert_eq!(result.tools.version.as_deref(), Some("1.4.0"));
        assert_eq!(result.devices.len(), 1);
        let device = &result.devices[0];
        assert_eq!(device.udid, UDID);
        assert_eq!(device.pair_state, PairState::NotPaired);
        assert_eq!(device.product_type.as_deref(), Some("iPhone13,2"));
        assert_eq!(device.serial_number, None, "not in the pre-session subset");
        assert_eq!(device.will_encrypt, None, "only read from paired devices");
        assert_eq!(device.data_used_bytes, None);
        assert!(!device.busy);
    }
    // The fake records every pairing attempt: there was none, and no validate either.
    assert_eq!(lab.pairing_attempts(), Vec::<String>::new());
    let calls = lab.calls();
    assert_eq!(count(&calls, "idevicepair", "hostid"), 3);
    assert_eq!(count(&calls, "idevicepair", "validate"), 0);
    assert_eq!(count(&calls, "idevicepair", "pair"), 0);
    // ideviceinfo only ever ran with -s (no session, no handshake).
    assert!(
        calls
            .iter()
            .filter(|call| call[0] == "ideviceinfo")
            .all(|call| call.iter().any(|a| a == "-s")),
        "{calls:?}"
    );
    assert_eq!(lab.idevice.paired_by_app_at(UDID), None);
    lab.assert_no_temp_dirs();
}

#[test]
fn pairing_goes_from_awaiting_trust_to_paired() {
    let lab = Lab::new("not_paired");
    let first = lab.idevice.pair(UDID, None).unwrap();
    assert_eq!(first.pair_state, PairState::AwaitingTrust);
    assert!(
        first
            .message
            .as_deref()
            .unwrap()
            .contains("accept the trust dialog"),
        "{first:?}"
    );
    assert_eq!(lab.idevice.paired_by_app_at(UDID), None);
    let second = lab.idevice.pair(UDID, None).unwrap();
    assert_eq!(second.pair_state, PairState::Paired);
    assert_eq!(second.message, None);
    assert_eq!(second.will_encrypt, Some(false));
    assert!(second.data_used_bytes.is_some());
    assert!(lab.idevice.paired_by_app_at(UDID).is_some());
    assert_eq!(lab.pairing_attempts(), ["pair", "pair"]);

    // Now paired: polling validates, and reads WillEncrypt and disk usage.
    let result = lab.idevice.list_devices(None);
    let device = &result.devices[0];
    assert_eq!(device.pair_state, PairState::Paired);
    assert_eq!(device.will_encrypt, Some(false));
    assert_eq!(device.data_used_bytes, Some(2 * 1024 * 1024));
    assert_eq!(device.data_capacity_bytes, Some(64 * 1024 * 1024 * 1024));
    assert_eq!(count(&lab.calls(), "idevicepair", "validate"), 1);
    assert_eq!(
        lab.pairing_attempts(),
        ["pair", "pair"],
        "validating did not pair"
    );

    // Pairing again is refused.
    let err = lab.idevice.pair(UDID, None).unwrap_err();
    assert!(matches!(err, IdeviceError::AlreadyPaired(_)), "{err}");
    assert_eq!(err.code(), ErrorCode::AlreadyPaired);
    lab.assert_no_temp_dirs();
}

#[test]
fn locked_trust_denied_and_pairing_failed_are_states() {
    for (scenario, state, text) in [
        ("locked", PairState::Locked, "because a passcode is set"),
        (
            "trust_denied",
            PairState::TrustDenied,
            "said that the user denied the trust dialog",
        ),
        ("pairing_failed", PairState::PairingFailed, "failed."),
    ] {
        let lab = Lab::new(scenario);
        // Polling shows "not paired" and never pairs.
        let polled = lab.idevice.list_devices(None);
        assert_eq!(
            polled.devices[0].pair_state,
            PairState::NotPaired,
            "{scenario}"
        );
        assert!(lab.pairing_attempts().is_empty(), "{scenario}");
        // Pairing reports the state, not an error.
        let summary = lab.idevice.pair(UDID, None).unwrap();
        assert_eq!(summary.pair_state, state, "{scenario}");
        assert!(
            summary.message.as_deref().unwrap().contains(text),
            "{scenario}: {summary:?}"
        );
        assert_eq!(summary.will_encrypt, None, "{scenario}");
        assert_eq!(lab.idevice.paired_by_app_at(UDID), None, "{scenario}");
        assert_eq!(lab.pairing_attempts(), ["pair"], "{scenario}");
    }
}

#[test]
fn the_busy_device_is_not_queried_and_cannot_be_paired() {
    let lab = Lab::new("success");
    let first = lab.idevice.list_devices(None);
    assert_eq!(first.devices[0].pair_state, PairState::Paired);
    let calls_before = lab.calls().len();
    let busy = lab.idevice.list_devices(Some(UDID));
    let device = &busy.devices[0];
    assert!(device.busy);
    assert_eq!(
        device.pair_state,
        PairState::Paired,
        "the last known summary"
    );
    assert_eq!(device.product_type.as_deref(), Some("iPhone13,2"));
    // Only `idevice_id -l` ran: the device itself was not queried.
    let new_calls = &lab.calls()[calls_before..];
    assert_eq!(new_calls.len(), 1, "{new_calls:?}");
    assert_eq!(new_calls[0], ["idevice_id", "-l"]);

    let err = lab.idevice.pair(UDID, Some(UDID)).unwrap_err();
    assert_eq!(err.code(), ErrorCode::DeviceBusy);
    let err = lab.idevice.pair("not-a-udid", None).unwrap_err();
    assert_eq!(err.code(), ErrorCode::DeviceNotFound);
    let other = "00008101-000A1B2C3D4E00FF";
    let err = lab.idevice.pair(other, None).unwrap_err();
    assert!(matches!(err, IdeviceError::DeviceNotFound(_)), "{err}");
}

#[test]
fn usbmuxd_missing_is_reported_not_thrown() {
    let lab = Lab::new("usbmuxd_missing");
    let result = lab.idevice.list_devices(None);
    assert_eq!(result.tools.state, IdeviceToolsState::UsbmuxdUnavailable);
    let guidance = result.tools.guidance.unwrap();
    assert!(
        guidance.contains("Apple Mobile Device Service"),
        "{guidance}"
    );
    assert!(guidance.contains("usbmuxd"), "{guidance}");
    assert!(result.devices.is_empty());
    let err = lab.idevice.pair(UDID, None).unwrap_err();
    assert_eq!(err.code(), ErrorCode::UsbmuxdUnavailable);
    lab.assert_no_temp_dirs();
}

#[test]
fn empty_ideviceinfo_output_is_a_failure() {
    let lab = Lab::new("info_empty");
    let result = lab.idevice.list_devices(None);
    assert_eq!(result.tools.state, IdeviceToolsState::Ok);
    let device = &result.devices[0];
    assert_eq!(device.pair_state, PairState::Paired);
    assert_eq!(
        (
            &device.device_name,
            &device.product_type,
            &device.product_version,
            &device.serial_number
        ),
        (&None, &None, &None, &None)
    );
    assert_eq!(device.will_encrypt, None);
    assert_eq!(device.data_used_bytes, None);
    assert!(
        device.message.as_deref().unwrap().contains("identity"),
        "{device:?}"
    );
}

#[test]
fn an_absent_will_encrypt_key_reads_as_false() {
    let lab = Lab::new("will_encrypt_absent");
    let device = &lab.idevice.list_devices(None).devices[0];
    assert_eq!(device.pair_state, PairState::Paired);
    assert_eq!(device.will_encrypt, Some(false), "{device:?}");
    // The whole domain is read (-k would print nothing for the absent key).
    let queries: Vec<_> = lab
        .calls()
        .into_iter()
        .filter(|call| call.iter().any(|a| a == "com.apple.mobile.backup"))
        .collect();
    assert_eq!(queries.len(), 1);
    assert!(!queries[0].iter().any(|a| a == "-k"), "{queries:?}");
}

#[test]
fn an_unreadable_backup_domain_is_unknown() {
    // enable_unknown makes WillEncrypt unreadable once `encryption on` has run.
    let lab = Lab::new("enable_unknown");
    let tools = lab.idevice.tools().unwrap();
    let session = lab.idevice.session(tools).unwrap();
    assert_eq!(session.will_encrypt(UDID), Some(false));
    let password = Password::new("unknown-state-pw".to_owned());
    session
        .set_encryption(UDID, true, &password, &mut |_| {})
        .unwrap();
    assert_eq!(session.will_encrypt(UDID), None);
}

#[test]
fn missing_tools_are_reported() {
    let empty = tempfile::tempdir().unwrap();
    let lab = Lab::with_tools(
        "success",
        &[],
        empty.path(),
        common::manifest_for(common::shared_tools().0.as_path()),
    );
    let result = lab.idevice.list_devices(None);
    assert_eq!(result.tools.state, IdeviceToolsState::Missing);
    assert!(result.tools.guidance.is_some());
    assert!(result.devices.is_empty());
    let err = lab.idevice.pair(UDID, None).unwrap_err();
    assert_eq!(err.code(), ErrorCode::IdeviceToolsMissing);
}

#[test]
fn a_tampered_bundled_tool_fails_verification() {
    let dir = tempfile::tempdir().unwrap();
    let tools = dir.path().join("tools");
    let manifest = common::private_tools(&tools);
    let lab = Lab::with_tools("success", &[], &tools, manifest);
    assert_eq!(
        lab.idevice.list_devices(None).tools.state,
        IdeviceToolsState::Ok
    );
    // Tamper with one tool: appending a byte keeps it runnable but changes its hash.
    let target = tools.join(format!("ideviceinfo{}", common::EXE));
    let mut bytes = fs::read(&target).unwrap();
    bytes.push(0);
    fs::write(&target, bytes).unwrap();
    let tools_state = lab.idevice.verify_tools().unwrap_err();
    assert_eq!(tools_state.state, IdeviceToolsState::VerificationFailed);
    let result = lab.idevice.list_devices(None);
    assert_eq!(result.tools.state, IdeviceToolsState::VerificationFailed);
    assert!(result.devices.is_empty());
    assert_eq!(
        lab.idevice.pair(UDID, None).unwrap_err().code(),
        ErrorCode::IdeviceToolsVerificationFailed
    );
}

#[test]
fn verified_tools_record_their_hashes() {
    let lab = Lab::new("success");
    let tools = lab.idevice.verify_tools().unwrap();
    let record = tools.record();
    assert_eq!(record.source, IdeviceToolSource::Bundled);
    assert_eq!(record.version.as_deref(), Some("1.4.0"));
    for binary in [
        &record.binaries.idevice_id,
        &record.binaries.ideviceinfo,
        &record.binaries.idevicepair,
        &record.binaries.idevicebackup2,
    ] {
        assert_eq!(binary.verified_against, ToolVerification::Manifest);
        assert_eq!(binary.sha256.len(), 64);
    }
}

#[test]
fn polling_is_single_flight() {
    // The fake holds `idevice_id -l` until the release file exists, so the first poll stays in
    // flight while the other calls arrive.
    let hold_dir = tempfile::tempdir().unwrap();
    let release = hold_dir.path().join("release");
    let lab = Arc::new(Lab::with_env(
        "success",
        &[("FAKE_IDEVICE_HOLD", release.to_str().unwrap())],
    ));
    let poll = |lab: &Arc<Lab>| {
        let lab = Arc::clone(lab);
        thread::spawn(move || lab.idevice.list_devices(None))
    };
    let wait_for = |what: &str, mut done: Box<dyn FnMut() -> bool>| {
        let deadline = Instant::now() + Duration::from_secs(10);
        while !done() {
            assert!(Instant::now() < deadline, "timed out waiting for {what}");
            thread::sleep(Duration::from_millis(10));
        }
    };
    let leader = poll(&lab);
    let probe = Arc::clone(&lab);
    wait_for(
        "the first poll to reach idevice_id",
        Box::new(move || count(&probe.calls(), "idevice_id", "-l") == 1),
    );
    let followers = [poll(&lab), poll(&lab)];
    let probe = Arc::clone(&lab);
    wait_for(
        "both calls to join the poll in flight",
        Box::new(move || probe.idevice.polls_waiting() == 2),
    );
    fs::write(&release, "").unwrap();
    let first = leader.join().unwrap();
    for follower in followers {
        assert_eq!(follower.join().unwrap(), first);
    }
    assert_eq!(first.devices.len(), 1);
    // One poll ran for all three calls.
    assert_eq!(
        count(&lab.calls(), "idevice_id", "-l"),
        1,
        "{:?}",
        lab.calls()
    );
    assert_eq!(lab.idevice.polls_waiting(), 0);
    // A later call polls again.
    lab.idevice.list_devices(None);
    assert_eq!(count(&lab.calls(), "idevice_id", "-l"), 2);
}

#[test]
fn device_info_and_pair_records() {
    let lab = Lab::new("success");
    let tools = lab.idevice.tools().unwrap();
    let session = lab.idevice.session(tools).unwrap();
    let (bytes, info) = session.full_info(UDID).unwrap();
    assert!(String::from_utf8_lossy(&bytes).contains("InternationalMobileEquipmentIdentity"));
    assert_eq!(info.serial_number.as_deref(), Some("F2LFAKE00001"));
    assert_eq!(
        session.host_id(UDID).unwrap().as_deref(),
        Some("5E1B7C2A-9D4F-4E8B-A3C6-1F0D2B7E9A48")
    );
    assert_eq!(
        session.system_buid().as_deref(),
        Some("0C6F3A9E-2B7D-4C1E-8F5A-6D9B3E0A7C21")
    );
    assert_eq!(session.will_encrypt(UDID), Some(false));
    assert_eq!(
        session.disk_usage(UDID).unwrap().data_used(),
        2 * 1024 * 1024
    );
    assert_eq!(session.is_connected(UDID), Some(true));
    drop(session);
    lab.assert_no_temp_dirs();
}

#[test]
fn encryption_changes_pass_the_password_only_through_the_environment() {
    const PASSWORD: &str = "Tr0ub4dor-env-only";
    let lab = Lab::new("success_encrypt");
    let tools = lab.idevice.tools().unwrap();
    let session = lab.idevice.session(tools).unwrap();
    let password = Password::new(PASSWORD.to_owned());
    let mut lines: Vec<OutputLine> = Vec::new();
    let on = session
        .set_encryption(UDID, true, &password, &mut |line| lines.push(line))
        .unwrap();
    assert_eq!(on.exit.exit_code, Some(0));
    assert!(
        !on.argv.iter().any(|a| a.contains(PASSWORD)),
        "{:?}",
        on.argv
    );
    assert_eq!(on.argv[1..], ["-u", UDID, "encryption", "on"]);
    let prompt = lines.iter().find(|l| l.prompt.is_some()).unwrap();
    assert_eq!(prompt.prompt, Some(DevicePromptKind::PasscodeForEncryption));
    assert_eq!(
        prompt.text,
        "Please confirm enabling the backup encryption by entering the passcode on the device."
    );
    assert_eq!(session.will_encrypt(UDID), Some(true));

    let off = session
        .set_encryption(UDID, false, &password, &mut |_| {})
        .unwrap();
    assert_eq!(off.exit.exit_code, Some(0));
    assert_eq!(session.will_encrypt(UDID), Some(false));
    assert_eq!(lab.state()["password_in_argv"], false);
    for call in lab.calls() {
        assert!(!call.iter().any(|a| a.contains(PASSWORD)), "{call:?}");
    }
    assert!(!format!("{password:?} {on:?} {off:?} {lines:?}").contains(PASSWORD));
}

#[test]
fn the_fake_fails_when_a_password_appears_in_argv() {
    let state = tempfile::tempdir().unwrap();
    let status = Command::new(FAKE)
        .args([
            "idevicebackup2",
            "-u",
            UDID,
            "encryption",
            "on",
            "hunter2-in-argv",
        ])
        .env("FAKE_IDEVICE_STATE_DIR", state.path())
        .env("FAKE_IDEVICE_SCENARIO", "success_encrypt")
        .output()
        .unwrap();
    assert_eq!(status.status.code(), Some(64));
    assert!(!String::from_utf8_lossy(&status.stderr).contains("hunter2"));
    let status = Command::new(FAKE)
        .args(["idevicepair", "-u", UDID, "hostid", "--pw=secret-in-env"])
        .env("FAKE_IDEVICE_STATE_DIR", state.path())
        .env("BACKUP_PASSWORD", "secret-in-env")
        .output()
        .unwrap();
    assert_eq!(status.status.code(), Some(64));
    let saved: serde_json::Value =
        serde_json::from_slice(&fs::read(state.path().join("state.json")).unwrap()).unwrap();
    assert_eq!(saved["password_in_argv"], true);
}

#[cfg(debug_assertions)]
#[test]
fn the_dev_override_uses_fake_idevice_for_all_four_tools() {
    let root = tempfile::tempdir().unwrap();
    let state = root.path().join("state");
    fs::create_dir_all(&state).unwrap();
    let idevice = Idevice::new(IdeviceConfig {
        lookup: ToolLookup {
            platform: Some(common::host_platform()),
            manifest: embedded_manifest().unwrap(),
            bundled_dir: None,
            dev_override: Some(FAKE.into()),
            path_var: None,
            signed_build: false,
        },
        app_cache: root.path().join("cache"),
        env: vec![("FAKE_IDEVICE_STATE_DIR".into(), state.into_os_string())],
    });
    let result = idevice.list_devices(None);
    assert_eq!(result.tools.state, IdeviceToolsState::Ok);
    assert_eq!(result.tools.source, Some(IdeviceToolSource::DevOverride));
    assert_eq!(result.tools.version, None);
    assert_eq!(result.devices[0].pair_state, PairState::Paired);
    let record = idevice.tools().unwrap().record();
    assert_eq!(
        record.binaries.idevicepair.verified_against,
        ToolVerification::None
    );
}
