# PC history sync implementation plan

> Use superpowers:subagent-driven-development for the independent modules, then inline UI integration and a fresh whole-change reviewer. Standing user authorization removes design/plan approval pauses.

Goal: version 0.2.0 adds automatically discovered PC histories while retaining quiet startup.
Spec: docs/sync-design.md. Preserve existing Windows x64, 100 entries per source, no clipboard writes without item selection, and encrypted local storage.

- [x] Transfer module: agreed streaming APIs and store helpers; nine transfer roundtrip, hostile-input and rollback tests pass.
- [x] Network module: TLS identity, LAN/Tailscale discovery, metadata polling, lazy restore, cancellation and bounded work; seven networking/lifecycle/parser tests pass.
- [x] UI integration: PC tabs beneath status, selected-source state, remote restore routing, independent sharing control, native tab tests and quiet startup pass.
- [ ] Review and distribution: integrate modules, fresh review, fix material findings, run fmt/clippy/full tests/package/background installer smoke; update README and versions; publish 0.2.0 and verify assets. Document limits of actual multi-machine testing.

Review focus: unauthenticated auto-pairing boundary disclosed, traversal/length limits, cache corruption/eviction, no recursive sharing, service disable and timeouts, Tailscale peer inventory failures, no UI blocking, firewall access.
