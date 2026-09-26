# suiteDFIR

A small, local desktop app for iOS backup acquisition and for running the iLEAPP and aLEAPP mobile-forensics parsers, with case organization and a defensible audit trail. For macOS (Apple silicon and Intel), Windows (x64 and arm64) and Linux (x64 and arm64); iOS acquisition is not available on Windows arm64.

> **Status:** phase 1 rewrite, version 0.2.0 in release preparation. The builds are not code-signed yet. The earlier version is [jacobecontreras/suiteDFIR](https://github.com/jacobecontreras/suiteDFIR).

## What phase 1 does

- **iOS backup over USB.** Pair an iPhone/iPad and take a full backup into the case (libimobiledevice), optionally enabling backup encryption for more data. Each acquisition gets its own audit record and hash manifest, and every change the app makes on the device is recorded.
- **Cases as plain folders.** A case is a directory with a `case.json` and one sub-folder per run and acquisition, so it is easy to inspect, archive and hand over.
- **Pinned, verified parsers.** Downloads the official iLEAPP/aLEAPP command-line builds at a pinned version and verifies their SHA-256, or imports them offline for air-gapped machines.
- **Runs.** Point it at a file-system extraction, an iTunes/Finder backup (encrypted or not), a zip/tar/gz archive or a disk image. Pick all modules, a saved profile or a custom selection, watch the live log, and cancel if needed.
- **Audit record for every run.** The record covers:
  - the parser version and binary hash;
  - the exact parameters (with secrets redacted);
  - the input hash;
  - timing;
  - a result derived from what the parser actually produced;
  - a SHA-256 manifest of the report (`sha256sum -c report.sha256`).
- **Reports.** Opens the LEAPP HTML report in your browser.

Phase 1 deliberately does **not** include Android acquisition, maps, timelines or other visualizations. See [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md#2-scope).

## Install

Download the dmg (macOS), the online or offline installer (Windows x64), the online installer (Windows arm64) or the AppImage/deb (Linux x64 and arm64) from the releases page and check it against `SHA256SUMS`. The Windows arm64 build has no iOS tools, so it parses but cannot acquire iOS backups. The [user guide](docs/USER-GUIDE.md) covers installation per system, including the steps for unsigned builds, the iOS acquisition prerequisites, and the Linux parser requirement (glibc 2.43 or later).

## Privacy

Everything runs locally. The app's only network traffic is downloading the pinned parser builds, and only when you ask it to. There is no telemetry. Backup passwords are never stored or logged.

## Documentation

- [docs/USER-GUIDE.md](docs/USER-GUIDE.md): installing and using suiteDFIR
- [docs/QA-CHECKLIST.md](docs/QA-CHECKLIST.md): the human release checks
- [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md): scope, decisions, design
- [docs/CONTRACTS.md](docs/CONTRACTS.md): file formats and the UI↔core API
- [docs/LEAPP-CLI.md](docs/LEAPP-CLI.md): verified behavior of the upstream parsers
- [docs/IDEVICE-CLI.md](docs/IDEVICE-CLI.md): libimobiledevice tools used for iOS acquisition
- [docs/ROADMAP.md](docs/ROADMAP.md): milestones and acceptance criteria
- [DEVELOPMENT.md](DEVELOPMENT.md): building, testing, contribution rules

## Credits

<!-- OWNER CHECKPOINT H7 (docs/ROADMAP.md): the credits wording for prior contributors is decided by the owner before the release candidate. Replace the placeholder line below; do not invent names. -->

> **Placeholder (owner checkpoint H7):** credits for the contributors to the earlier suiteDFIR versions will be added here by the owner.

iLEAPP and aLEAPP are developed by Alexis Brignoni and contributors and are MIT-licensed. suiteDFIR runs their official releases and does not modify them. iOS acquisition uses [libimobiledevice](https://libimobiledevice.org/) (LGPL-2.1+/GPL-2.0+), built from pinned upstream sources; the source code is attached to every release.

## License

Apache License 2.0. See [LICENSE](LICENSE), [NOTICE](NOTICE) and [THIRD-PARTY-NOTICES.md](THIRD-PARTY-NOTICES.md).
