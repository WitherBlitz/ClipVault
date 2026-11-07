# ClipVault 0.2.0 verification — 2026-11-07

## Scope

Adds PC tabs below the header, separate 100-item histories, automatic IPv4 LAN/Tailscale discovery and first-contact pairing, TLS transfer, lazy remote recalls, and a persistent sharing switch. Quiet automatic startup remains enabled by installation. Automatic pairing follows the user's requested any-PC policy; TLS does not establish common ownership. README and the Sharing ON label disclose the network-sharing boundary.

## Test evidence

The initial integrated suite passed 25 tests. Fresh review produced three material findings; regression fixes added raw-format rejection, independent sharing cancellation, and socket-level TLS timeouts. The final `scripts/package.ps1` run passed **31 tests** (27 library, 3 native UI, 1 real-process startup), `cargo fmt --check`, `cargo clippy --locked --all-targets -- -D warnings`, optimized compilation and installer compilation. `scripts/smoke-installer.ps1` passed automatic startup, background launch and uninstall checks.

Coverage includes native PC selection and remote restore command routing; local updates while a remote PC is selected; source pagination; encrypted text/image/file snapshots; 36 MiB multi-format image transfer; Unicode/global-memory clipboard format preservation; malicious IDs, registered names, Windows paths, truncated streams and hash mismatches; cache rollback and retention; two loopback sync instances discovering/polling/restoring text, PNG and arbitrary binary files; exact certificate mismatch rejection; disabled/persisted sync; and real dribbled TLS handshakes stopped by deadline and disable.

The exit test verifies cancellation does not change the saved sharing preference. Independent control work prevents the sharing switch from waiting behind file downloads. Socket guards enforce enable state and deadlines beneath rustls, including handshakes. File imports are serialized per service and re-encrypted locally; only the local Store is advertised, preventing recursive re-sharing of remote entries.

Native offscreen painting with synthetic My PC/PlexPC fixtures was inspected to check tab placement above the history list. This is a rendered layout test, not a physical-desktop screenshot. The build machine's installed Tailscale CLI returned valid JSON with eight online peers; no clipboard data was transmitted to those peers during that read-only check.

## Installer checks

The isolated smoke test verifies EXE hash, required files, automatic startup registration without popup arguments, exactly one background process after installation, a quiet sign-in-equivalent launch, and uninstall/startup cleanup. Test instances isolate LOCALAPPDATA and default sharing off, leaving the real profile and firewall unchanged.

## Unobserved environments and limits

Actual separate-PC LAN/Tailscale restoration, interactive Windows Firewall permission, IPv6-only networks, mixed-DPI desktop operation and rich-text rendering in another consumer were not observed. Tailscale ACLs and firewall rules must allow UDP 39471/TCP 39472. IPv6-only discovery is outside this release. The initial preview's private-OLE/virtual-attachment limits remain.

Network metadata is bounded to 512 MiB and buffered; files stream in chunks with integrity checks. Connections are bounded by rate, concurrency, idle and total timeouts. Very large or slow transfers can exceed the timeout. Previously fetched copies cannot be revoked by disabling or clearing the source. Binaries remain unsigned previews.
