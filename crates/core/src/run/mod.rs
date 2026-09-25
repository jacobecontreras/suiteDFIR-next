//! Runs: the `run.json` record (`record`), status rules (`status`), LEAPP argv building and
//! redaction (`argv`), profiles (`profile`) and `.lcasedata` case data (`casedata`).

pub mod argv;
pub mod casedata;
pub mod profile;
pub mod record;
pub mod status;

#[cfg(test)]
mod tests {
    //! The iTunes backup password exists only in `RunRequest` and the spawned argv
    //! (DEVELOPMENT.md §4.3). It never reaches `run.json`, `Debug` output or error messages.

    use std::fs;
    use std::path::Path;

    use super::argv::{self, ArgvSpec};
    use super::record::{self, RecordError, RunSetup};
    use crate::contracts::{AppError, RunStatus, SealStatus, Timestamp, examples};
    use crate::fsutil::test_support::make_writable;

    const PASSWORD: &str = examples::EXAMPLE_PASSWORD;

    fn assert_no_password(what: &str, text: &str) {
        assert!(
            !text.contains(PASSWORD),
            "{what} leaks the password: {text}"
        );
    }

    #[test]
    fn the_password_never_leaks() {
        let case = tempfile::tempdir().unwrap();
        let request = examples::run_request();
        assert_eq!(request.itunes_password.as_deref(), Some(PASSWORD));
        assert_no_password("RunRequest Debug", &format!("{request:?}"));

        let created = Timestamp::parse("2026-09-24T18:30:05Z").unwrap();
        let (run_id, run_dir) = record::create_run_dir(case.path(), request.tool, created).unwrap();
        let entry = case.path().join("ileapp");
        let profile = run_dir.join("profile.ilprofile");
        let spec = ArgvSpec {
            entry: &entry,
            input_type: request.input_type,
            input: Path::new(&request.input_path),
            run_dir: &run_dir,
            profile: Some(&profile),
            timezone: request.timezone.as_deref(),
            itunes_password: request.itunes_password.as_deref(),
            keychain: None,
        };
        assert_no_password("ArgvSpec Debug", &format!("{spec:?}"));
        let command = argv::build(&spec).unwrap();
        // The spawned argv carries it, right after the flag; nothing else does.
        let at = command.argv().iter().position(|a| a == PASSWORD).unwrap();
        assert_eq!(command.argv()[at - 1], "--itunes_password");
        assert_no_password("LeappCommand Debug", &format!("{command:?}"));
        let recorded = command.recorded();
        assert_eq!(recorded.argv[at], "<redacted>");
        assert_no_password("recorded command", &format!("{recorded:?}"));

        let example = examples::run_record_initial();
        let mut run = record::initial_record(RunSetup {
            run_id,
            label: request.label.clone(),
            created_at: created,
            host: example.host,
            case_snapshot: example.case_snapshot,
            tool: example.tool,
            input: example.input,
            options: example.options,
            modules: example.modules,
            command: recorded,
        })
        .unwrap();
        assert!(run.options.password_supplied);
        record::write_initial(&run_dir, &run).unwrap();
        let file = run_dir.join("run.json");
        assert_no_password("initial run.json", &fs::read_to_string(&file).unwrap());

        run.status = RunStatus::Cancelled;
        run.output.seal.status = SealStatus::SkippedNoOutput;
        record::finalize(&run_dir, &mut run, created).unwrap();
        assert_no_password("final run.json", &fs::read_to_string(&file).unwrap());
        assert_no_password("RunRecord Debug", &format!("{run:?}"));
        assert_no_password("RunRecord JSON", &serde_json::to_string(&run).unwrap());

        // Error messages from the steps that see the password or the record.
        let err = record::finalize(&run_dir, &mut run, created).unwrap_err();
        assert!(matches!(err, RecordError::AlreadyFinalized { .. }));
        assert_no_password("RecordError", &format!("{err} {err:?}"));
        let app: AppError = err.into();
        assert_no_password("AppError", &format!("{app} {app:?}"));
        let bad = ArgvSpec {
            input: Path::new(""),
            ..spec
        };
        let err = argv::build(&bad).unwrap_err();
        assert_no_password("ArgvError", &format!("{err} {err:?}"));
        make_writable(&file);
    }
}
