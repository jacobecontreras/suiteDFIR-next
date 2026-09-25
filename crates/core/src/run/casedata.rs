//! Writing `case.lcasedata` (CONTRACTS.md §8), passed to LEAPP with `-d` so its report shows the
//! case number, agency and examiner (ARCHITECTURE.md F8).

use std::io;
use std::path::{Path, PathBuf};

use crate::contracts::{CaseDataValues, CaseSnapshot, LeappCaseData};
use crate::fsutil;

/// The case-data file inside the run folder.
pub const CASE_DATA_FILE: &str = "case.lcasedata";

/// LEAPP's case-data file for a case.
pub fn case_data(case: &CaseSnapshot) -> LeappCaseData {
    LeappCaseData {
        leapp: "case_data".to_owned(),
        case_data_values: CaseDataValues {
            case_number: case.case_number.clone(),
            agency: case.agency.clone(),
            examiner: case.examiner.clone(),
        },
    }
}

/// Writes `<run_dir>/case.lcasedata` atomically and returns its path.
pub fn write(run_dir: &Path, case: &CaseSnapshot) -> io::Result<PathBuf> {
    let path = run_dir.join(CASE_DATA_FILE);
    fsutil::write_json_atomic(&path, &case_data(case))?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::fs;

    use crate::contracts::examples;
    use crate::run::record::case_snapshot;

    #[test]
    fn writes_leapp_native_case_data() {
        let dir = tempfile::tempdir().unwrap();
        let path = write(dir.path(), &case_snapshot(&examples::case_file())).unwrap();
        assert_eq!(path, dir.path().join("case.lcasedata"));
        let value: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        // CONTRACTS.md §8, key for key.
        assert_eq!(
            value,
            serde_json::json!({"leapp": "case_data", "case_data_values": {
                "Case Number": "2026-0142", "Agency": "County Forensics Lab", "Examiner": "J. Doe"}})
        );
        let parsed: LeappCaseData = serde_json::from_value(value).unwrap();
        assert_eq!(parsed, examples::leapp_case_data());
    }

    #[test]
    fn empty_values_and_special_characters_stay_valid_json() {
        let dir = tempfile::tempdir().unwrap();
        let mut case = case_snapshot(&examples::case_file());
        case.case_number = String::new();
        case.agency = "Lab \"North\" / Unit\n2".to_owned();
        case.examiner = "Zoë Ångström".to_owned();
        let path = write(dir.path(), &case).unwrap();
        let parsed: LeappCaseData = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
        assert_eq!(parsed.case_data_values.case_number, "");
        assert_eq!(parsed.case_data_values.agency, "Lab \"North\" / Unit\n2");
        assert_eq!(parsed.case_data_values.examiner, "Zoë Ångström");
    }
}
