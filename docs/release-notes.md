# ClipVault v0.1.0 — initial Windows preview

A native Rust clipboard manager for Windows 10/11 x64.

- Clipboard tray icon and popup, plus Ctrl+Shift+V.
- 10 pages × 10 items, retaining the newest 100 entries.
- Unicode text, HTML/RTF companions, images with thumbnails and transparency.
- File/folder snapshots with arbitrary extensions, encrypted streaming storage and SHA256 integrity checks. Recalled files paste as copies.
- DPAPI encrypted local history; pause, clear and optional start at sign-in.
- Per-user EXE installer, portable EXE, and SHA256 checksums.

Download the Setup EXE for installation, or the other EXE for portable use. Windows may put the tray icon in overflow; move it into your notification area to keep it visible.

Validation: full Rust suite (13 passing tests), format check, clippy with warnings denied, release build, and installer checks are recorded in docs/verification.md. Native popup pagination and item selection are covered by automated Windows control tests. Interactive desktop visual testing was interrupted, so this release is marked prerelease.

Limitations: unsigned binaries; app-private/OLE objects, vector metafiles and unsaved virtual attachments are outside this version. Save virtual attachments to disk before copying. File snapshots preserve names and bytes, rather than ACLs, timestamps or alternate streams. Recalled file staging is plaintext until the item is evicted/cleared. See README for storage behavior and supported clipboard formats.
