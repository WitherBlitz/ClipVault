# ClipVault Implementation Plan

> **For agentic workers:** Use superpowers:executing-plans to implement this plan task-by-task. The user has authorized inline execution without approval pauses.

**Goal:** Ship a tested Windows Rust clipboard manager, EXE installer, and public GitHub release.

**Architecture:** Native Win32 tray/popup and clipboard listener, serialized background capture, DPAPI encrypted local history with file snapshots. UI restores selected entries without duplicating them.

**Tech Stack:** Rust, windows-sys 0.61, image 0.25, serde/bincode, SHA256, Inno Setup 6.

**Spec:** docs/design.md

## Global Constraints

- Windows 10/11 x64; newest 100 items; ten pages of ten items.
- Preserve supported clipboard companion formats and every file extension; snapshot files and folders.
- Local encrypted persistence; no network code in the application.
- Public release to WitherBlitz; no PR required.

## Tasks

- [ ] 1. Core/storage: write failing retention, pagination, Unicode, snapshot-after-source-delete, restart, corruption, and encryption tests. Implement src/model.rs and src/store.rs; run cargo test --lib.
- [ ] 2. Clipboard: write real Windows clipboard roundtrip tests with prior clipboard preservation. Implement src/clipboard.rs (read, busy retry, privacy markers, format restoration, DIB preview); run cargo test --lib -- --test-threads=1.
- [ ] 3. Native UI: implement src/ui.rs and src/main.rs, tray lifecycle, popup and native accessible buttons, paging, keyboard shortcuts, focus dismissal, pause/errors/startup. Verify UI on the real desktop and rerun the suite.
- [ ] 4. Distribution: add icon/resource manifest, installer/setup.iss, scripts/package.ps1, CI, README and license. Run cargo fmt --check, cargo clippy --all-targets -- -D warnings, cargo test -- --test-threads=1, cargo build --release, compile installer, install/uninstall smoke test in an isolated path. Fresh reviewer checks final code and support claims; fix material findings and rerun checks.
- [ ] 5. Publish: initialize git, commit only source and documentation, create public repository, push and tag v0.1.0, upload portable EXE/installer/checksums, verify release asset list and download hashes. Copy deliverables into outputs/.

## Review Focus

Busy/delayed clipboard owners; alpha and DIB variants; failed or partial file snapshots; eviction while staged files are referenced by the clipboard; desktop taskbar restart and multi-monitor placement.

## Verification commands

- Install: cargo fetch --locked
- Narrow: cargo test --lib model
- Full: cargo test -- --test-threads=1
- Lint: cargo clippy --all-targets -- -D warnings
- Format: cargo fmt --check
- Build/package: powershell -NoProfile -File scripts/package.ps1
