# suiteDFIR

A small, local desktop app for iOS backup acquisition and for running the iLEAPP and aLEAPP mobile-forensics parsers, with case organization and a defensible audit trail.

> **Status:** private phase 1 rewrite, in progress. The current public version is [jacobecontreras/suiteDFIR](https://github.com/jacobecontreras/suiteDFIR).

## What phase 1 does

- **iOS backup over USB.** Pair an iPhone/iPad and take a full backup into the case (libimobiledevice), optionally enabling backup encryption for more data. Each acquisition gets its own audit record and hash manifest.
- **Cases as plain folders.** A case is a directory with a `case.json` and one sub-folder per run, so it is easy to inspect, archive and hand over.
- **Pinned, verified parsers.** Downloads the official iLEAPP/aLEAPP command-line builds at a pinned version and verifies their SHA-256, or imports them offline for air-gapped machines.
- **Runs.** Point it at a file-system extraction, an iTunes/Finder backup (encrypted or not), a zip/tar/gz archive or a disk image. Pick all modules, a saved profile or a custom selection, watch the live log, and cancel if needed.
- **Audit record for every run.** The record covers:
  - the parser version and binary hash;
  - the exact parameters (with secrets redacted);
  - the input hash;
  - timing;
  - a result derived from what the parser actually produced;
  - a SHA-256 manifest of the report.
- **Reports.** Opens the LEAPP HTML report in your browser.

Phase 1 deliberately does **not** include Android acquisition, maps, timelines or other visualizations. See [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md#2-scope).

## Privacy

Everything runs locally. The app's only network traffic is downloading the pinned parser builds, and only when you ask it to. There is no telemetry.

## Documentation

- [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md): scope, decisions, design
- [docs/CONTRACTS.md](docs/CONTRACTS.md): file formats and the UI↔core API
- [docs/LEAPP-CLI.md](docs/LEAPP-CLI.md): verified behavior of the upstream parsers
- [docs/IDEVICE-CLI.md](docs/IDEVICE-CLI.md): libimobiledevice tools used for iOS acquisition
- [docs/ROADMAP.md](docs/ROADMAP.md): milestones and acceptance criteria
- [DEVELOPMENT.md](DEVELOPMENT.md): building, testing, contribution rules

## Credits

iLEAPP and aLEAPP are developed by Alexis Brignoni and contributors and are MIT-licensed. suiteDFIR runs their official releases and does not modify them. iOS acquisition uses [libimobiledevice](https://libimobiledevice.org/) (LGPL-2.1+/GPL-2.0+), built from pinned upstream sources.

## License

Apache License 2.0. See [LICENSE](LICENSE) and [THIRD-PARTY-NOTICES.md](THIRD-PARTY-NOTICES.md).
