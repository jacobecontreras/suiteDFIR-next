# suiteDFIR

> **Not ready for use.** suiteDFIR is in development and there's no release of the app yet. Hands-on testing with real devices and real evidence has only just started, so please don't rely on it for casework.

suiteDFIR is a desktop app for mobile forensics. It runs [iLEAPP](https://github.com/abrignoni/iLEAPP) and [aLEAPP](https://github.com/abrignoni/ALEAPP), the open-source iOS and Android parsers by Alexis Brignoni, and it can take an iOS backup from a phone connected over USB. Each case is a plain folder on disk, and every parser run and every backup gets its own record in that folder.

This repository is a rewrite of [suiteDFIR](https://github.com/jacobecontreras/suiteDFIR). The earlier version had maps, a timeline and an embedded report viewer. This one leaves those out for now and concentrates on running the parsers correctly and keeping a clear record of each step.

## What it does

### Parsers

suiteDFIR doesn't ship iLEAPP or aLEAPP. When you click Install, it downloads the official release build from GitHub at a pinned version (iLEAPP v2026.4.2, aLEAPP v2026.4.1) and checks the SHA-256 of the download and of the program inside it. The program is checked again before every run. On a machine without internet access you can import the same release files by hand.

### Runs

You choose a case, a parser and an input: a file-system extraction folder, an iTunes/Finder backup, a zip/tar/gz archive, or a disk image such as an E01. You can run every module, a saved profile or a hand-picked set, and follow the parser's log while it works.

LEAPP exits with code 0 even when the input is invalid, so suiteDFIR doesn't trust the exit code. It decides whether a run succeeded from the output LEAPP actually wrote (`_lava_data.lava` and `index.html`).

### Records

Each run folder gets a `run.json` with the parser version and hashes, the exact command line (with any backup password masked), the timezone, the modules, the start and end times, and the result. `report.sha256` lists the SHA-256 of every file in the report, in the format `sha256sum -c` reads.

Single-file inputs such as disk images and archives can be hashed during the run. Folder inputs aren't hashed yet. `run.json` is made read-only when the run finishes, but nothing is signed, so the record shows what happened without proving that the files weren't changed later.

### iOS backups

suiteDFIR uses the libimobiledevice command-line tools to pair with an iPhone or iPad and take a full iTunes-style backup into the case folder. It can turn on backup encryption with a password you choose, because an encrypted backup contains more data, and turn it off again afterwards. Those changes go into `acquisition.json`, next to a `backup.sha256` manifest of the backup.

On macOS and Windows x64 the tools are built by this project from pinned upstream sources. On Linux the app uses your distribution's packages.

This is the least tested part of the app. The first test with a real iPhone turned up a TLS problem in the tools, which is now patched, and a full backup on a real device is still to be done.

### Case folders

```
My Case/
  case.json
  runs/<run id>/            run.json, report/, report.sha256, parser logs
  acquisitions/<acq id>/    acquisition.json, backup/, backup.sha256, tool logs
```

## Platforms

The app is built with [Tauri](https://tauri.app/): a Rust core and a plain HTML and JavaScript interface, with no frameworks. It targets macOS (Apple silicon and Intel), Windows (x64 and arm64) and Linux (x64 and arm64). CI builds it and runs the tests on Linux, macOS and Windows, but so far it has only been tried by hand on an Apple silicon Mac.

iOS backups aren't available on Windows arm64, because there's no build of the iOS tools for it. On Linux, the pinned parser builds need a recent glibc: 2.43 for iLEAPP and 2.42 for aLEAPP (Ubuntu 26.04 has 2.43).

## Privacy

suiteDFIR only goes online when you ask it to download a parser. It has no telemetry, no accounts and no update check. The parsers are third-party programs, and suiteDFIR doesn't restrict their network access.

suiteDFIR doesn't save backup passwords. iLEAPP only accepts a backup password on its command line, though. While an iLEAPP run is going, other programs on the computer can see that password, and security software that logs process command lines (EDR, Sysmon) will record it. The iOS backup tools get their password through an environment variable, which keeps it out of those logs, but other programs running as the same user can still read it. One question is still open: whether iLEAPP writes the password into its own report or logs, which are kept in the case folder.

## Documentation

The docs below were written by AI along with the code (see [How this was built](#how-this-was-built)) and I'm still reviewing them. Where a doc and the code disagree, the code is right.

- [docs/USER-GUIDE.md](docs/USER-GUIDE.md): how the app is meant to be installed and used
- [docs/QA-CHECKLIST.md](docs/QA-CHECKLIST.md): the hands-on checks to finish before a release
- [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md): scope, design decisions and how the parts fit together
- [docs/CONTRACTS.md](docs/CONTRACTS.md): file formats and the interface between the UI and the core
- [docs/LEAPP-CLI.md](docs/LEAPP-CLI.md): how the pinned iLEAPP and aLEAPP builds behave
- [docs/IDEVICE-CLI.md](docs/IDEVICE-CLI.md): the libimobiledevice tools used for iOS backups
- [DEVELOPMENT.md](DEVELOPMENT.md): building and testing

## Credits

iLEAPP and aLEAPP are developed by Alexis Brignoni and contributors and are MIT-licensed. suiteDFIR runs their official release builds without modifying them.

iOS backups use [libimobiledevice](https://libimobiledevice.org/) (LGPL-2.1-or-later). The tools are built from pinned upstream sources with two small patches, one to libimobiledevice and one to Mbed TLS, so that encrypted connections to the device work. The source tarballs, the patches and the build script are published with the iOS tool builds on this repository's releases page.

## License

Apache License 2.0. See [LICENSE](LICENSE), [NOTICE](NOTICE) and [THIRD-PARTY-NOTICES.md](THIRD-PARTY-NOTICES.md).

## How this was built

This version of suiteDFIR was written entirely by AI. I used Claude Code, Anthropic's coding tool, to run a group of Claude agents. One agent planned the work and split it into tasks, others wrote the code, tests, documentation, CI and build scripts, and separate agents reviewed the pull requests that changed code. I set the goals, made the decisions the agents brought to me, and did the hands-on testing. Getting from the first commit to commit b5ac473 on September 25, 2026 took about 29 hours and produced roughly 65,000 lines of code and tests.

The agents worked through my GitHub account. The commits and merges, the pull requests and their descriptions, the review comments on them (headed "Independent review", "Review of …" and similar), the idevice-tools pre-releases, and the `gate/*` and `smoke/*` commit statuses were all posted by agents, even though they show my name. The statuses record test runs the agents did on my machines. Commits don't carry AI co-author tags, so this section is the disclosure for the whole history.

CI runs more than 500 Rust tests on each platform and 204 JavaScript tests. Where they involve iLEAPP, aLEAPP or the iOS tools, they mostly use stand-ins, which keeps them fast and repeatable. The real parsers run in CI on small sample inputs. Checks against real evidence and real devices come next, and that's where the work is now:

- Reviewing by hand the code that matters most for evidence: how a run's result is decided, how the command line is built and the password masked, hashing, and the iOS backup flow.
- Comparing reports from suiteDFIR with reports from running iLEAPP and aLEAPP directly with the same options, to confirm the app doesn't change their output.
- Working through the [QA checklist](docs/QA-CHECKLIST.md) with real devices and evidence on macOS and Windows, including a full iPhone backup.
- Closing the gaps found so far, such as hashing folder inputs and keeping a record of device pairing in the case when the app is restarted between pairing and the backup.
- Reviewing the docs, which the agents wrote along with the code.
