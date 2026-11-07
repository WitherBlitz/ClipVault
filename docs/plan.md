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

- [x] 1. Core/storage: retention, pagination, Unicode, file snapshots, restart, corruption, and encryption implemented and tested.
- [x] 2. Clipboard: real Windows clipboard roundtrips, busy retry, privacy markers, companion restoration, DIB and PNG/DIBV5 alpha tested.
- [x] 3. Native UI: tray/popup, native buttons, paging, keyboard, menus and background workers implemented; native control tests pass. Interactive visual QA was stopped with Escape and remains a documented prerelease limitation.
- [x] 4. Distribution: resources, installer, packaging, CI, README, license and dependency notices complete. Format/lint, 13 tests, release build and isolated install/uninstall checks passed. Material independent review findings fixed.
- [x] 5. Publish: public WitherBlitz/ClipVault source and v0.1.0 preview release uploaded. Installer, portable EXE, notices and checksums downloaded and matched against the local artifacts. Local deliverables are copied into outputs/ at handoff.

## Review Focus

Busy/delayed clipboard owners; alpha and DIB variants; failed or partial file snapshots; eviction while staged files are referenced by the clipboard; desktop taskbar restart and multi-monitor placement.

## Verification commands

- Install: cargo fetch --locked
- Narrow: cargo test --lib model
- Full: cargo test -- --test-threads=1
- Lint: cargo clippy --all-targets -- -D warnings
- Format: cargo fmt --check
- Build/package: powershell -NoProfile -File scripts/package.ps1
