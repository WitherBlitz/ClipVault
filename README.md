# ClipVault

A native **Rust** clipboard history for Windows 10/11 x64. Text, images, and files live in a compact popup beside a clipboard tray icon. PC tabs below the header let you browse automatically discovered LAN and Tailscale computers.

![ClipVault clipboard icon](assets/clipvault.png)

## Install

Download **ClipVault-Setup-0.2.0-x64.exe** from [Releases](https://github.com/WitherBlitz/ClipVault/releases/tag/v0.2.0). Install this version on each PC. The installer runs per user without administrator rights, enables start-at-sign-in automatically, and immediately starts ClipVault in the background. No history popup opens during installation or sign-in. A portable EXE and SHA256 checksums are also provided. Binaries are currently unsigned.

Windows may initially place the clipboard icon under the tray's overflow arrow. Drag it onto the taskbar notification area, or enable it in Windows taskbar settings. Applications cannot force Windows to keep a tray icon visible.

## Use

- Copy as usual. Click the clipboard tray icon or press **Ctrl+Shift+V**.
- Select your own PC or another PC using the tabs below the header. Small arrows next to the tabs reveal more PCs. Each PC keeps its own history; remote clipboard changes do not overwrite your clipboard.
- Each page holds **10 items**. Use the bottom left/right arrows for **10 pages**; each source retains its newest **100 items**. Empty pages are accessible.
- Click an item to restore it to your clipboard, then paste with **Ctrl+V**. Tab/Enter select items; Escape dismisses; left/right keys change pages.
- Right-click the tray icon, or click **•••**, for pause/resume, startup, network sharing, clear local history, or exit. Start-at-sign-in is enabled by the installer; you can turn it off from this menu.
- Files and folders are saved as snapshots, regardless of extension. Recalling them supplies a normal Windows file list that Explorer and compatible apps can paste. A recalled cut operation becomes a copy, protecting the original files.

## Images and files

Clipboard PNG (including alpha), Windows DIB/DIBV5 bitmaps, JPEG/JFIF, GIF, TIFF, and WebP representations are preserved. Common images get thumbnails, with a checkerboard for transparency. Native bitmap clipboard content can be converted by Windows to DIB. Text preserves Unicode, HTML and RTF companion formats. The original captured payload is restored; thumbnails do not replace full-resolution images. Pasting into another application depends on the formats that application accepts.

File snapshots support ordinary local/network-accessible files and folders, without an extension allowlist or a per-file size cap. Snapshotting requires read access and enough free disk space. Links/junctions are rejected rather than followed, and a failed snapshot is reported without adding a partial history item. Save application-generated virtual files (for example, unsaved Outlook attachments) to disk before copying them. Application-private clipboard objects, OLE embedded documents and vector metafiles are not captured in this version. Clipboard-only virtual attachments are not represented as ordinary filesystem files.

## LAN and Tailscale

**Automatic pairing is enabled by default. Every reachable ClipVault participant on your LAN or tailnet can read your shared clipboard history.** This includes existing history saved before upgrading. Use it on trusted networks; turn off **Share history with ALL LAN / Tailscale PCs** in the tray menu to stop sharing. The header shows Sharing ON/OFF. Pairing is automatic rather than proof that a peer belongs to the same person.

LAN discovery uses IPv4 broadcasts. For Tailscale, install and sign in to Tailscale on both PCs; ClipVault reads online peers from `tailscale status --json` and sends discovery directly to their Tailscale IPv4 addresses. Other PCs must run ClipVault 0.2.0 or later with sharing enabled. Windows Firewall and Tailscale ACLs must permit **UDP 39471** for discovery and **TCP 39472** for history. Allow ClipVault on your trusted Windows network if Windows prompts; the installer does not change firewall/security settings. No router port forwarding is needed. IPv6-only networks are not supported in this version.

Names, entry summaries and thumbnails refresh automatically. Original clipboard payloads and file bytes download only when you select an item, then are copied to your local clipboard. Received items are not re-published as your own history. Offline PC tabs retain their last summaries for up to ten minutes, then expire. A PC must be online to fetch its items. There is no fixed 16-PC cap; connection concurrency, discovery rate and metadata memory are bounded. Very large collections can wait for metadata refresh as capacity becomes available.

Transfers use TLS and the exact certificate advertised by the discovered peer. The private key is encrypted for the Windows account. This encrypts traffic but does not authenticate a person's ownership during automatic first contact. The service accepts loopback, RFC1918/link-local IPv4 and Tailscale's IPv4 range, rather than public Internet sources. ClipVault runs no cloud service of its own and requires no ClipVault account or telemetry.

## Storage and privacy

History lives in `%LOCALAPPDATA%\ClipVault`; remote fetched caches live under `peers`. Text, image payloads, metadata and file snapshots are encrypted with Windows DPAPI for the current account. Each fetched peer cache retains at most 100 entries. Windows history-exclusion flags and the `Clipboard Viewer Ignore` marker used by password managers are respected.

Clearing local history or disabling sharing cannot revoke copies already fetched by other PCs.

When you recall files, plaintext copies are materialized under `staging` inside the local or peer history directory, so other programs can paste them. These remain until their cached entry is evicted or its history is cleared. Any process running as your account can access recalled files and can use DPAPI; this is protection at rest, not protection against malware running as you. An active large snapshot may finish after you pause. Very rapid clipboard changes can be missed when another application holds the clipboard locked or replaces its content before capture.

File transfers stream in 1 MiB chunks with length/SHA256 validation. Metadata, including original clipboard image formats, is limited to 512 MiB per entry and is buffered; near-limit images can require substantial memory. Transfers have a total timeout, so exceptionally large/slow files may fail and can be retried. Remote Windows paths and clipboard format IDs are validated before import.

The 101st distinct captured item removes the oldest item and its file payloads. Consecutive identical text/image captures are coalesced; files are snapshotted again because their contents may have changed. File snapshots preserve names and bytes, not filesystem permissions, alternate data streams, or timestamps. Clearing history asks for confirmation. Uninstall leaves history intact; clear it from the app first if desired. Uninstall also removes the startup registration.

## Build and verify

Requires the Rust stable MSVC x64 toolchain, Visual Studio C++ build tools/Windows SDK, and Inno Setup 6. The ICO and manifest are checked in; Python/Pillow is needed only to regenerate the icon.

```powershell
cargo fetch --locked
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked -- --test-threads=1
cargo build --locked --release
./scripts/package.ps1
```

The real clipboard integration tests temporarily use and restore supported clipboard content. Avoid copying during these tests. Other tests cover retention, native PC tabs/pagination/selection, encrypted restart, corrupt entries, file snapshots, transfer integrity/hostile input, locked-file eviction, PNG/DIBV5 alpha, and loopback TLS discovery/restore/disable. CI also tests automatic startup and quiet installation, then uploads the installer and portable executable.

Version 0.2.0 is a preview release. Automated tests use two isolated instances on one Windows machine; actual separate-PC LAN/Tailscale and interactive firewall checks were not observed. Test your own network and normal copy/paste workflows before relying on history as a backup.

For an isolated test instance: `clipvault.exe --data-dir C:\some\test-directory --show`. This uses a separate history and instance lock and defaults network sharing off. `CLIPVAULT_NO_SYNC=1` also defaults sharing off for automated installer tests with fresh configuration. A persisted sharing preference takes precedence. Do not run another clipboard manager during clipboard tests.

## Native API references

The implementation follows Microsoft's [clipboard listener guidance](https://learn.microsoft.com/en-us/windows/win32/dataxchg/using-the-clipboard) and [Shell clipboard formats](https://learn.microsoft.com/en-us/windows/win32/shell/clipboard), particularly CF_HDROP and copy drop effects.

Sync uses [rustls](https://docs.rs/rustls/latest/rustls/struct.StreamOwned.html), [rcgen](https://docs.rs/rcgen/latest/rcgen/fn.generate_simple_self_signed.html), and the [Tailscale CLI JSON interface](https://tailscale.com/docs/reference/tailscale-cli?q=cli).

MIT licensed. Source: [WitherBlitz/ClipVault](https://github.com/WitherBlitz/ClipVault).
