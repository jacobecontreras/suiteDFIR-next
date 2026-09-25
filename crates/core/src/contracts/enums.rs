//! String enumerations: CONTRACTS.md §2 and §13.1, the §12 error codes, and the closed value sets
//! that the spec writes inline (e.g. `which: "stdout" | "stderr" | …`).

// ---- CONTRACTS.md §2 ----

string_enum! {
    /// A supported LEAPP tool.
    pub enum ToolId {
        Ileapp = "ileapp",
        Aleapp = "aleapp",
    }
}

string_enum! {
    /// An OS and CPU pair that tools are pinned for.
    pub enum PlatformKey {
        MacosAarch64 = "macos-aarch64",
        MacosX86_64 = "macos-x86_64",
        WindowsX86_64 = "windows-x86_64",
        WindowsAarch64 = "windows-aarch64",
        LinuxX86_64 = "linux-x86_64",
        LinuxAarch64 = "linux-aarch64",
    }
}

string_enum! {
    /// Whether an input path is a file or a directory.
    pub enum InputKind {
        File = "file",
        Directory = "directory",
    }
}

string_enum! {
    /// A LEAPP `-t` input type.
    pub enum InputType {
        Fs = "fs",
        Tar = "tar",
        Zip = "zip",
        Gz = "gz",
        Itunes = "itunes",
        File = "file",
        Raw = "raw",
    }
}

string_enum! {
    /// How the modules of a run were chosen.
    pub enum ModuleMode {
        All = "all",
        Profile = "profile",
        Custom = "custom",
    }
}

string_enum! {
    /// The status of a run (CONTRACTS.md §7.3).
    pub enum RunStatus {
        Running = "running",
        Succeeded = "succeeded",
        CompletedWithErrors = "completed_with_errors",
        Failed = "failed",
        Cancelled = "cancelled",
        Interrupted = "interrupted",
    }
}

string_enum! {
    /// The state of input hashing.
    pub enum HashStatus {
        NotRequested = "not_requested",
        NotApplicable = "not_applicable",
        Pending = "pending",
        Completed = "completed",
        Cancelled = "cancelled",
        Failed = "failed",
        Interrupted = "interrupted",
    }
}

string_enum! {
    /// The state of an output manifest (`report.sha256`, `backup.sha256`).
    pub enum SealStatus {
        Pending = "pending",
        Sealed = "sealed",
        SkippedNoOutput = "skipped_no_output",
        Failed = "failed",
        Cancelled = "cancelled",
        Interrupted = "interrupted",
    }
}

string_enum! {
    /// Where an installed tool came from.
    pub enum InstallSource {
        Download = "download",
        OfflineImport = "offline_import",
        /// Debug builds only.
        DevOverride = "dev_override",
    }
}

string_enum! {
    /// What a tool's entry hash was checked against before a run.
    pub enum EntryVerifiedAgainst {
        Manifest = "manifest",
        InstallRecord = "install_record",
        /// Dev override only.
        None = "none",
    }
}

string_enum! {
    /// The install state of a LEAPP tool.
    pub enum ToolState {
        UnsupportedPlatform = "unsupported_platform",
        NotInstalled = "not_installed",
        InstalledUnverified = "installed_unverified",
        Verified = "verified",
        VerificationFailed = "verification_failed",
        DevOverride = "dev_override",
    }
}

string_enum! {
    /// The phases of a run, in order (ARCHITECTURE.md §6).
    pub enum RunPhase {
        Preparing = "preparing",
        Running = "running",
        HashingInput = "hashing_input",
        Analyzing = "analyzing",
        SealingReport = "sealing_report",
        Finalizing = "finalizing",
    }
}

// ---- CONTRACTS.md §13.1 ----

string_enum! {
    /// The status of an acquisition (CONTRACTS.md §13.3).
    pub enum AcqStatus {
        Running = "running",
        Succeeded = "succeeded",
        Failed = "failed",
        Cancelled = "cancelled",
        Interrupted = "interrupted",
    }
}

string_enum! {
    /// The phases of an acquisition, in order (ARCHITECTURE.md §6b).
    pub enum AcqPhase {
        Preparing = "preparing",
        EnablingEncryption = "enabling_encryption",
        BackingUp = "backing_up",
        RestoringEncryption = "restoring_encryption",
        Validating = "validating",
        Sealing = "sealing",
        Finalizing = "finalizing",
    }
}

string_enum! {
    /// The pairing state of a device with this host.
    pub enum PairState {
        Paired = "paired",
        NotPaired = "not_paired",
        AwaitingTrust = "awaiting_trust",
        Locked = "locked",
        TrustDenied = "trust_denied",
        PairingFailed = "pairing_failed",
        Unknown = "unknown",
    }
}

string_enum! {
    /// Where the libimobiledevice tools come from.
    pub enum IdeviceToolSource {
        Bundled = "bundled",
        System = "system",
        DevOverride = "dev_override",
    }
}

string_enum! {
    /// Whether the libimobiledevice tools and usbmuxd are usable.
    pub enum IdeviceToolsState {
        Ok = "ok",
        Missing = "missing",
        VerificationFailed = "verification_failed",
        UsbmuxdUnavailable = "usbmuxd_unavailable",
        UnsupportedPlatform = "unsupported_platform",
    }
}

string_enum! {
    /// How a libimobiledevice tool binary was verified at runtime (CONTRACTS.md §13.2).
    pub enum ToolVerification {
        /// The hash equals the pinned unsigned hash.
        Manifest = "manifest",
        /// macOS `codesign --verify --strict` passed.
        CodeSignature = "code_signature",
        /// The hash is recorded but not verifiable (signed Windows builds, Linux system tools).
        RecordedOnly = "recorded_only",
        /// Dev override.
        None = "none",
    }
}

string_enum! {
    /// The outcome of turning backup encryption off again after an acquisition.
    pub enum RestoreState {
        NotRequested = "not_requested",
        Restored = "restored",
        Failed = "failed",
        NotAttempted = "not_attempted",
        Unknown = "unknown",
    }
}

string_enum! {
    /// Why the device is waiting for the examiner.
    pub enum DevicePromptKind {
        PasscodeForBackup = "passcode_for_backup",
        PasscodeForEncryption = "passcode_for_encryption",
    }
}

// ---- CONTRACTS.md §12 ----

string_enum! {
    /// `AppError.code` (CONTRACTS.md §12).
    pub enum ErrorCode {
        AnotherInstanceRunning = "another_instance_running",
        /// Any job (run or acquisition) is active.
        RunAlreadyActive = "run_already_active",
        AcqNotFound = "acq_not_found",
        DeviceBusy = "device_busy",
        AlreadyPaired = "already_paired",
        RestoreNotApplicable = "restore_not_applicable",
        PathNotSupportedByTool = "path_not_supported_by_tool",
        DeviceNotFound = "device_not_found",
        DeviceNotPaired = "device_not_paired",
        DeviceLocked = "device_locked",
        TrustPending = "trust_pending",
        TrustDenied = "trust_denied",
        PairingFailed = "pairing_failed",
        UsbmuxdUnavailable = "usbmuxd_unavailable",
        IdeviceToolsMissing = "idevice_tools_missing",
        IdeviceToolsVerificationFailed = "idevice_tools_verification_failed",
        EncryptionAlreadyOn = "encryption_already_on",
        EncryptionPasswordRequired = "encryption_password_required",
        InsufficientSpace = "insufficient_space",
        RunNotFound = "run_not_found",
        ToolNotInstalled = "tool_not_installed",
        ToolVerificationFailed = "tool_verification_failed",
        UnsupportedPlatform = "unsupported_platform",
        DownloadFailed = "download_failed",
        HashMismatch = "hash_mismatch",
        ExtractFailed = "extract_failed",
        IntrospectionFailed = "introspection_failed",
        CaseNotFound = "case_not_found",
        CaseExists = "case_exists",
        InvalidCase = "invalid_case",
        InvalidInput = "invalid_input",
        InputTypeNotAllowed = "input_type_not_allowed",
        InputOverlapsCase = "input_overlaps_case",
        PasswordRequired = "password_required",
        InvalidTimezone = "invalid_timezone",
        ProfileNotFound = "profile_not_found",
        ProfileInvalid = "profile_invalid",
        ProfileExists = "profile_exists",
        UnknownModules = "unknown_modules",
        ReportMissing = "report_missing",
        PathNotAllowed = "path_not_allowed",
        PathTooLong = "path_too_long",
        PermissionDenied = "permission_denied",
        Io = "io",
        Internal = "internal",
    }
}

// ---- Closed value sets written inline in CONTRACTS.md ----

string_enum! {
    /// `leapp-manifest.json` `archive_kind` (§3).
    pub enum ArchiveKind {
        /// `entry` is a path inside the zip.
        Zip = "zip",
        /// `entry` is a path inside `squashfs-root/`.
        Appimage = "appimage",
    }
}

string_enum! {
    /// `run.json` `input.hash.algorithm` (§7.1). SHA-256 only (ARCHITECTURE.md D18).
    pub enum HashAlgorithm {
        Sha256 = "sha256",
    }
}

string_enum! {
    /// `InstallEvent` `stage` (§11).
    pub enum InstallStage {
        Downloading = "downloading",
        Verifying = "verifying",
        Extracting = "extracting",
        Hashing = "hashing",
        Introspecting = "introspecting",
        Done = "done",
    }
}

string_enum! {
    /// `RunEvent` `stdio_tail.stream` (§11).
    pub enum StdStream {
        Stdout = "stdout",
        Stderr = "stderr",
    }
}

string_enum! {
    /// The kind of job: `ActiveJob.kind` and `job_attach` `kind` (§9, §10).
    pub enum JobKind {
        Run = "run",
        Acquisition = "acquisition",
    }
}

string_enum! {
    /// `open_text_file` `which` (§10).
    pub enum RunFile {
        Stdout = "stdout",
        Stderr = "stderr",
        RunJson = "run_json",
        ReportManifest = "report_manifest",
    }
}

string_enum! {
    /// `open_acq_file` `which` (§13.5).
    pub enum AcqFile {
        Stdout = "stdout",
        Stderr = "stderr",
        AcquisitionJson = "acquisition_json",
        BackupManifest = "backup_manifest",
        DeviceInfo = "device_info",
    }
}

string_enum! {
    /// `AcqPreflight.level` (§13.5, ARCHITECTURE.md §6b step 3).
    pub enum PreflightLevel {
        Ok = "ok",
        Warn = "warn",
        Block = "block",
    }
}

string_enum! {
    /// `acquisition.json` `commands[].purpose` (§13.3): the device-changing commands.
    pub enum AcqCommandPurpose {
        EnableEncryption = "enable_encryption",
        Backup = "backup",
        RestoreEncryption = "restore_encryption",
    }
}

string_enum! {
    /// `acquisition.json` `device_changes[].change` (§13.3).
    pub enum DeviceChangeKind {
        PairRecordCreated = "pair_record_created",
        BackupEncryptionEnabled = "backup_encryption_enabled",
        SyncLockTaken = "sync_lock_taken",
        BackupEncryptionDisabled = "backup_encryption_disabled",
    }
}

string_enum! {
    /// `acquisition.json` `encryption.password_channel` (§13.3): passwords travel only via env.
    pub enum PasswordChannel {
        Env = "env",
    }
}
