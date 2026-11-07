# v0.1.0 verification — 2026-11-07

Target: Windows x64, Rust 1.97.1 MSVC, Inno Setup 6.7.3.

## Executed checks

- `cargo fmt --check` — passed.
- `cargo clippy --locked --all-targets -- -D warnings` — passed.
- `cargo test --locked -- --test-threads=1` — 13 passed, 0 failed (11 library and 2 native UI tests).
- `cargo build --locked --release` — passed.
- `powershell -NoProfile -File scripts/package.ps1` — passed; installer, portable EXE, dependency notices and SHA256SUMS generated.
- `powershell -NoProfile -File scripts/smoke-installer.ps1` — passed: silent per-user installation to an isolated workspace folder, installed executable matches build hash, required files and uninstall registration exist, startup defaults to off, uninstall removes executable/registration and a startup entry enabled after installation.
- MSVC `dumpbin /DEPENDENTS target/release/clipvault.exe` — imports only Windows system DLLs; no separate Visual C++ runtime DLL required.

## Behavior covered

Real clipboard Unicode/HTML byte preservation, PNG alpha, DIB orientation and DIBV5 alpha; file-list roundtrip; confidentiality/history-exclusion markers; 101st-entry eviction; ten pages of ten native buttons; selection produces the correct restore command; native notifications survive modal reentrancy; encrypted restart; arbitrary binary file and folder restoration after original deletion; truncated snapshots rejected; unreadable metadata preserves its encrypted payloads; locked recalled files cause insertion rollback rather than losing an existing snapshot; eviction clears clipboard paths before deleting staging.

The pagination, persistence, snapshot and clipboard regression tests were first observed failing against missing/incorrect behavior, then passing after implementation. Added coverage also checks native controls, DIBV5 and locked-file cleanup.

## Independent review

A fresh read-only reviewer identified five material concerns. They were addressed before packaging:

1. Clipboard retrieval now runs on a dedicated thread, independently of storage and the UI.
2. Clipboard notifications bypass the UI state borrow, including nested menu/dialog message loops; DPI changes are explicitly deferred and recreate fonts.
3. Every staged payload removal checks and releases clipboard references. Locked-file eviction rolls back the new entry, retaining 100 entries and the old encrypted snapshot.
4. Payloads belonging to unreadable metadata are retained.
5. Encrypted file restoration verifies the saved byte length and SHA256 digest. Truncated blobs cannot be marked ready.

Debug tracing was removed. PNG and DIBV5 transparency both have direct pixel assertions.

## Limits of the evidence

Interactive desktop automation was stopped with the physical Escape key. No further desktop automation was attempted. Native hidden-window controls and clipboard tests passed, but popup visual appearance, tray clicks, mixed-DPI monitor interaction, Explorer restart and rich-text pasting into a separate consumer were not visually verified. A deliberately delayed-rendering external clipboard owner was not exercised; moving reads off the UI thread is covered by the notification dispatch test and source review, rather than such an end-to-end timing test.

The CF_HTML test verifies exact companion-format bytes, rather than rendering in a rich-text editor. These remaining manual checks are why v0.1.0 is published as a prerelease. No claim is made to capture app-private OLE objects or unsaved virtual-file attachments. See README for exact supported formats and staging/privacy behavior.
