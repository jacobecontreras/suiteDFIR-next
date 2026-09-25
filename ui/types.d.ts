// IPC types mirroring the Rust contract types in crates/core/src/contracts/ (the source of truth;
// docs/CONTRACTS.md §2, §6, §7, §9–§13). `npm run typecheck` checks the generated examples in
// ui-dev/fixtures/contracts/index.js against these declarations.
//
// Conventions: snake_case keys; optional values are `null`, never omitted (except where a request
// field is marked `?`); times are RFC 3339 UTC strings with `Z` at second precision; durations
// are integer milliseconds; hashes are lowercase hex SHA-256.

// ---- Enumerations (§2, §13.1) ----

export type ToolId = "ileapp" | "aleapp";
export type PlatformKey =
  | "macos-aarch64"
  | "macos-x86_64"
  | "windows-x86_64"
  | "windows-aarch64"
  | "linux-x86_64"
  | "linux-aarch64";
export type InputKind = "file" | "directory";
export type InputType = "fs" | "tar" | "zip" | "gz" | "itunes" | "file" | "raw";
export type ModuleMode = "all" | "profile" | "custom";
export type RunStatus =
  | "running"
  | "succeeded"
  | "completed_with_errors"
  | "failed"
  | "cancelled"
  | "interrupted";
export type HashStatus =
  | "not_requested"
  | "not_applicable"
  | "pending"
  | "completed"
  | "cancelled"
  | "failed"
  | "interrupted";
export type SealStatus = "pending" | "sealed" | "skipped_no_output" | "failed" | "cancelled" | "interrupted";
export type InstallSource = "download" | "offline_import" | "dev_override";
export type EntryVerifiedAgainst = "manifest" | "install_record" | "none";
export type ToolState =
  | "unsupported_platform"
  | "not_installed"
  | "installed_unverified"
  | "verified"
  | "verification_failed"
  | "dev_override";
export type RunPhase = "preparing" | "running" | "hashing_input" | "analyzing" | "sealing_report" | "finalizing";

export type AcqStatus = "running" | "succeeded" | "failed" | "cancelled" | "interrupted";
export type AcqPhase =
  | "preparing"
  | "enabling_encryption"
  | "backing_up"
  | "restoring_encryption"
  | "validating"
  | "sealing"
  | "finalizing";
export type PairState =
  | "paired"
  | "not_paired"
  | "awaiting_trust"
  | "locked"
  | "trust_denied"
  | "pairing_failed"
  | "unknown";
export type IdeviceToolSource = "bundled" | "system" | "dev_override";
export type IdeviceToolsState = "ok" | "missing" | "verification_failed" | "usbmuxd_unavailable" | "unsupported_platform";
export type ToolVerification = "manifest" | "code_signature" | "recorded_only" | "none";
export type RestoreState = "not_requested" | "restored" | "failed" | "not_attempted" | "unknown";
export type DevicePromptKind = "passcode_for_backup" | "passcode_for_encryption";

// Closed value sets written inline in CONTRACTS.md.
export type HashAlgorithm = "sha256";
export type InstallStage = "downloading" | "verifying" | "extracting" | "hashing" | "introspecting" | "done";
export type StdStream = "stdout" | "stderr";
export type JobKind = "run" | "acquisition";
export type RunFile = "stdout" | "stderr" | "run_json" | "report_manifest";
export type AcqFile = "stdout" | "stderr" | "acquisition_json" | "backup_manifest" | "device_info";
export type PreflightLevel = "ok" | "warn" | "block";
export type AcqCommandPurpose = "enable_encryption" | "backup" | "restore_encryption";
export type DeviceChangeKind =
  | "pair_record_created"
  | "backup_encryption_enabled"
  | "sync_lock_taken"
  | "backup_encryption_disabled";
export type PasswordChannel = "env";

// ---- Errors (§12) ----

export type ErrorCode =
  | "another_instance_running"
  | "run_already_active"
  | "acq_not_found"
  | "device_busy"
  | "already_paired"
  | "restore_not_applicable"
  | "path_not_supported_by_tool"
  | "device_not_found"
  | "device_not_paired"
  | "device_locked"
  | "trust_pending"
  | "trust_denied"
  | "pairing_failed"
  | "usbmuxd_unavailable"
  | "idevice_tools_missing"
  | "idevice_tools_verification_failed"
  | "encryption_already_on"
  | "encryption_password_required"
  | "insufficient_space"
  | "run_not_found"
  | "tool_not_installed"
  | "tool_verification_failed"
  | "unsupported_platform"
  | "download_failed"
  | "hash_mismatch"
  | "extract_failed"
  | "introspection_failed"
  | "case_not_found"
  | "case_exists"
  | "invalid_case"
  | "invalid_input"
  | "input_type_not_allowed"
  | "input_overlaps_case"
  | "password_required"
  | "invalid_timezone"
  | "profile_not_found"
  | "profile_invalid"
  | "profile_exists"
  | "unknown_modules"
  | "report_missing"
  | "path_not_allowed"
  | "path_too_long"
  | "permission_denied"
  | "io"
  | "internal";

/** The error of every command. `message` is safe to show; `detail` never holds secrets. */
export type AppError = { code: ErrorCode; message: string; detail: string | null };

// ---- Files returned over IPC (§6, §7, §13.3) ----

export type SettingsDefaults = { examiner: string; agency: string; timezone: string };
/** `settings.json`. `tools_dir: null` = the default tools directory. */
export type Settings = {
  schema_version: number;
  cases_root: string;
  recent_cases: string[];
  defaults: SettingsDefaults;
  tools_dir: string | null;
};

/** `case.json`. */
export type CaseFile = {
  schema_version: number;
  case_id: string;
  name: string;
  case_number: string;
  examiner: string;
  agency: string;
  description: string;
  default_timezone: string | null;
  created_at: string;
  updated_at: string;
  created_by_app_version: string;
};

export type Reason = { code: string; message: string };
export type RecordApp = { name: string; version: string };
export type RecordHost = { os: string; os_version: string; arch: string; hostname: string };
export type CaseSnapshot = { case_id: string; name: string; case_number: string; examiner: string; agency: string };
export type Seal = {
  status: SealStatus;
  manifest: string | null;
  manifest_sha256: string | null;
  file_count: number | null;
  total_bytes: number | null;
};

/** `run.json`, the audit record of a run (§7). */
export type RunRecord = {
  schema_version: number;
  run_id: string;
  label: string | null;
  status: RunStatus;
  status_reasons: Reason[];
  warnings: Reason[];
  created_at: string;
  started_at: string | null;
  ended_at: string | null;
  recovered_at: string | null;
  duration_ms: number | null;
  app: RecordApp;
  host: RecordHost;
  case_snapshot: CaseSnapshot;
  tool: {
    id: ToolId;
    version: string;
    platform: PlatformKey;
    asset_name: string | null;
    asset_sha256: string | null;
    entry_sha256: string;
    entry_verified_against: EntryVerifiedAgainst;
    install_source: InstallSource;
  };
  input: {
    path: string;
    kind: InputKind;
    type: InputType;
    type_detected: InputType | null;
    size_bytes: number | null;
    itunes_encrypted: boolean | null;
    acquisition_id: string | null;
    hash: {
      algorithm: HashAlgorithm;
      status: HashStatus;
      value: string | null;
      started_at: string | null;
      completed_at: string | null;
    };
  };
  options: {
    timezone: string | null;
    timezone_supported: boolean;
    password_supplied: boolean;
    keychain_path: string | null;
    keychain_sha256: string | null;
  };
  modules: {
    mode: ModuleMode;
    profile_name: string | null;
    requested: string[];
    resolved: string[];
    unknown: string[];
    always_run: string[];
    available_count: number;
  };
  command: { argv: string[]; cwd: string };
  process: {
    exit_code: number | null;
    signal: number | null;
    exited_at: string;
    cancel_requested: boolean;
    escalated_to_kill: boolean;
  } | null;
  leapp_result: {
    lava_data_found: boolean;
    processing_status: string | null;
    leapp_version_reported: string | null;
    index_html_found: boolean;
    module_counts: { complete: number; error: number; no_files_found: number; other: number } | null;
    error_modules: string[];
  } | null;
  output: { report_dir: string; seal: Seal };
  logs: { stdout: string; stderr: string; screen_output: string };
};

export type ToolBinary = { path: string; sha256: string; verified_against: ToolVerification };
export type AcqTools = {
  version: string | null;
  source: IdeviceToolSource;
  binaries: { idevice_id: ToolBinary; ideviceinfo: ToolBinary; idevicepair: ToolBinary; idevicebackup2: ToolBinary };
};

/** `acquisition.json`, the audit record of an acquisition (§13.3). */
export type AcquisitionRecord = {
  schema_version: number;
  acq_id: string;
  label: string | null;
  status: AcqStatus;
  status_reasons: Reason[];
  warnings: Reason[];
  created_at: string;
  started_at: string | null;
  ended_at: string | null;
  recovered_at: string | null;
  duration_ms: number | null;
  app: RecordApp;
  host: RecordHost;
  case_snapshot: CaseSnapshot;
  device: {
    udid: string;
    serial_number: string | null;
    device_name: string | null;
    product_type: string | null;
    product_version: string | null;
    build_version: string | null;
    captured_at: string | null;
    info_file: string | null;
    info_file_sha256: string | null;
  };
  pairing: {
    paired_before: boolean;
    paired_by_app_at: string | null;
    host_id: string | null;
    system_buid: string | null;
  };
  device_changes: { at: string; change: DeviceChangeKind; detail: string }[];
  tools: AcqTools;
  encryption: {
    will_encrypt_before: boolean | null;
    enable_requested: boolean;
    enabled_by_examiner: boolean;
    will_encrypt_after_enable: boolean | null;
    restore_requested: boolean;
    restored_after: RestoreState;
    will_encrypt_after_restore: boolean | null;
    password_supplied: boolean;
    password_channel: PasswordChannel | null;
  };
  commands: {
    purpose: AcqCommandPurpose;
    argv: string[];
    exit_code: number | null;
    started_at: string;
    exited_at: string | null;
  }[];
  process: { exit_code: number | null; signal: number | null; cancel_requested: boolean; escalated_to_kill: boolean } | null;
  backup_result: {
    final_message: string | null;
    udid_dir: string;
    manifest_found: string | null;
    info_plist_found: boolean;
    status_plist_found: boolean;
    snapshot_state: string | null;
    last_progress_percent: number | null;
    device_file_errors: number;
    free_bytes_after: number | null;
  } | null;
  output: { backup_dir: string; seal: Seal };
  logs: { stdout: string; stderr: string };
};

// ---- Shared IPC types (§9) ----

export type ToolStatus = {
  tool: ToolId;
  display_name: string;
  pinned_version: string;
  state: ToolState;
  installed_version: string | null;
  install_source: InstallSource | null;
  module_count: number | null;
  install_dir: string | null;
  problem: string | null;
};
export type ModuleInfo = {
  name: string;
  module_name: string;
  category: string;
  display_name: string;
  description: string | null;
};
export type ToolModules = {
  tool: ToolId;
  version: string;
  always_run: Record<string, string[]>;
  timezones: string[] | null;
  modules: ModuleInfo[];
};
export type CaseSummary = {
  path: string;
  exists: boolean;
  case: CaseFile | null;
  run_count: number;
  last_run_at: string | null;
};
/** `recovered` = the run and acquisition ids marked `interrupted` by this open. */
export type CaseDetail = {
  path: string;
  case: CaseFile;
  runs: RunSummary[];
  acquisitions: AcqSummary[];
  recovered: string[];
};
export type RunSummary = {
  run_id: string;
  run_dir: string;
  label: string | null;
  status: RunStatus;
  tool: ToolId;
  tool_version: string;
  input_path: string;
  input_type: InputType;
  created_at: string;
  started_at: string | null;
  ended_at: string | null;
  duration_ms: number | null;
  report_available: boolean;
};
export type InputInspection = {
  path: string;
  kind: InputKind;
  size_bytes: number | null;
  detected_type: InputType | null;
  allowed_types: InputType[];
  is_itunes_backup: boolean;
  itunes_encrypted: boolean | null;
  hashable: boolean;
  warnings: string[];
};
export type ModuleSelection =
  | { mode: "all" }
  | { mode: "profile"; profile_name: string }
  | { mode: "custom"; modules: string[] };
/** `run_start` request. The UI clears the password field after `run_start`. */
export type RunRequest = {
  case_path: string;
  tool: ToolId;
  input_path: string;
  input_type: InputType;
  modules: ModuleSelection;
  timezone: string | null;
  itunes_password: string | null;
  keychain_path: string | null;
  hash_input: boolean;
  label: string | null;
};
export type ActiveJob =
  | { kind: "run"; case_path: string; run_id: string; tool: ToolId; created_at: string; phase: RunPhase }
  | { kind: "acquisition"; case_path: string; acq_id: string; udid: string; created_at: string; phase: AcqPhase };
export type ProfileInfo = { tool: ToolId; name: string; modules: string[]; unknown_modules: string[] };
/** S1. */
export type IosBackup = {
  path: string;
  device_name: string | null;
  product_type: string | null;
  ios_version: string | null;
  last_backup: string | null;
  encrypted: boolean | null;
  size_bytes: number | null;
};

// ---- Command requests and responses (§10) ----

export type AppInfo = {
  app_version: string;
  platform: PlatformKey | null;
  os: string;
  arch: string;
  dev_override: boolean;
  paths: { app_data: string; app_config: string; app_cache: string; app_log: string; tools_dir: string };
};
/** Omitted = unchanged. `tools_dir: null` = reset to the default. `defaults` replaces all three. */
export type SettingsUpdateRequest = { cases_root?: string; defaults?: SettingsDefaults; tools_dir?: string | null };
/** `tool_verify`, `tool_install`, `tool_modules`, `profiles_list`. */
export type ToolRequest = { tool: ToolId };
export type ToolImportRequest = { tool: ToolId; archive_path: string };
/** `parent_dir: null` = `settings.cases_root`. */
export type CaseCreateRequest = {
  name: string;
  case_number: string;
  examiner: string;
  agency: string;
  description: string;
  default_timezone: string | null;
  parent_dir: string | null;
};
/** `case_open`, `case_forget`, `reveal_path`. */
export type PathRequest = { path: string };
/** The editable fields of `case.json`. */
export type CaseFields = {
  name: string;
  case_number: string;
  examiner: string;
  agency: string;
  description: string;
  default_timezone: string | null;
};
export type CaseUpdateRequest = { path: string; fields: CaseFields };
/** `run_get`, `open_report`. */
export type RunRef = { case_path: string; run_id: string };
export type InputInspectRequest = { tool: ToolId; path: string; case_path: string };
export type ProfileSaveRequest = { tool: ToolId; name: string; modules: string[] };
/** `profile_delete`. */
export type ProfileRef = { tool: ToolId; name: string };
export type ProfileImportRequest = { tool: ToolId; path: string; name: string | null; overwrite: boolean };
export type ProfileExportRequest = { tool: ToolId; name: string; dest_path: string };
/** `run_start` response. */
export type RunStarted = { run_id: string; run_dir: string };
export type RunCancelRequest = { run_id: string };
export type JobAttachRequest = { kind: JobKind; id: string };
/** `job_attach` response. */
export type JobBacklog = { backlog: string[] };
export type OpenTextFileRequest = { case_path: string; run_id: string; which: RunFile };
/** `temp_cleanup` response. */
export type TempCleanupResult = { freed_bytes: number };

// ---- Events (§11) ----

export type RunEvent =
  | { type: "phase"; phase: RunPhase }
  | { type: "log"; lines: string[] }
  | { type: "stdio_tail"; stream: StdStream; lines: string[] }
  | { type: "hash_progress"; bytes_done: number; bytes_total: number }
  | { type: "seal_progress"; files_done: number; files_total: number | null }
  | { type: "finished"; status: RunStatus; reasons: Reason[]; warnings: Reason[]; summary: RunSummary };
export type InstallEvent =
  | { type: "stage"; stage: InstallStage }
  | { type: "download_progress"; bytes_done: number; bytes_total: number }
  | { type: "message"; text: string };

// ---- Acquisition (§13.5) ----

export type DeviceSummary = {
  udid: string;
  device_name: string | null;
  product_type: string | null;
  product_version: string | null;
  serial_number: string | null;
  pair_state: PairState;
  busy: boolean;
  will_encrypt: boolean | null;
  data_used_bytes: number | null;
  data_capacity_bytes: number | null;
  message: string | null;
};
export type DevicesResult = {
  tools: { source: IdeviceToolSource | null; version: string | null; state: IdeviceToolsState; guidance: string | null };
  devices: DeviceSummary[];
};
export type DevicePairRequest = { udid: string };
export type AcqPreflightRequest = { case_path: string; udid: string };
export type AcqPreflight = { free_bytes: number; required_bytes: number | null; level: PreflightLevel };
/** `acq_start` request. The UI clears the password after use. */
export type AcqRequest = {
  case_path: string;
  udid: string;
  label: string | null;
  enable_encryption: boolean;
  encryption_password: string | null;
  restore_encryption: boolean;
};
/** `acq_start` response. */
export type AcqStarted = { acq_id: string; acq_dir: string };
export type AcqCancelRequest = { acq_id: string };
/** `acq_get`. */
export type AcqRef = { case_path: string; acq_id: string };
export type AcqRestoreEncryptionRequest = { case_path: string; acq_id: string; password: string };
export type AcqRestoreEncryptionResult = { restored: boolean; will_encrypt_after: boolean | null };
export type OpenAcqFileRequest = { case_path: string; acq_id: string; which: AcqFile };
/**
 * `backup_path` = the absolute path of `backup/<udid>` when succeeded; `warnings` = warning codes,
 * so the Case screen can offer "Turn backup encryption off".
 */
export type AcqSummary = {
  acq_id: string;
  acq_dir: string;
  label: string | null;
  status: AcqStatus;
  udid: string;
  device_name: string | null;
  product_version: string | null;
  created_at: string;
  started_at: string | null;
  ended_at: string | null;
  duration_ms: number | null;
  backup_path: string | null;
  warnings: string[];
};
export type AcqEvent =
  | { type: "phase"; phase: AcqPhase }
  | { type: "log"; lines: string[] }
  | { type: "progress"; percent: number }
  | { type: "device_prompt"; kind: DevicePromptKind; text: string }
  | { type: "seal_progress"; files_done: number; files_total: number | null }
  | { type: "finished"; status: AcqStatus; reasons: Reason[]; warnings: Reason[]; summary: AcqSummary };
