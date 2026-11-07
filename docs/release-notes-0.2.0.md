# ClipVault v0.2.0 — PC history tabs

- PC tabs below the header select your local history or a discovered computer's history.
- Automatic discovery/pairing over IPv4 LAN and Tailscale. Each PC has its own 100-item history, with ten pages of ten items.
- Text/image thumbnails refresh automatically; selected original images and file/folder snapshots transfer over TLS on demand and restore to your local clipboard.
- Visible Sharing ON/OFF state and a persistent sharing switch. Incoming entries are not re-shared as your own copies.
- Automatic start-at-sign-in and quiet background launches retained from v0.1.1.

**Every reachable ClipVault participant on the allowed LAN/tailnet can read your shared history, including previously saved history. Automatic pairing does not verify common ownership.** Use trusted networks or disable sharing in the tray menu. Windows Firewall/Tailscale ACLs must permit UDP 39471 and TCP 39472. Install ClipVault 0.2.0 on each PC; Tailscale also requires its installed, signed-in CLI.

Validation uses isolated loopback instances, native control tests, encrypted transfer/hostile-input regression tests, and automatic background-install checks. Separate physical PCs, actual Tailscale routes and interactive firewall prompts were not tested. This remains an unsigned preview. See README for formats, transfer limits, encryption and privacy behavior.
