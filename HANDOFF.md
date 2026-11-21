# Mouser handoff log

A running log for whoever picks up Mouser next. Each finished part of the work
adds a dated entry here, newest first, in the same push. Longer background is in
[docs/HANDOVER.md](docs/HANDOVER.md) and the
[Weirdnet integration plan](docs/weirdnet-integration-plan.md).

## 2026-11-20: Weirdnet plan Phase 2, MCP server core on weirdnet-mcp

Author: m3talsmith. Commit: [`8d74fa0`](https://github.com/WeirdBrains/mouser/commit/8d74fa0).

**What changed**

- [`crates/mouser-core/src/mcp_server.rs`](crates/mouser-core/src/mcp_server.rs) passes each JSON-RPC message to [weirdnet-mcp](https://crates.io/crates/weirdnet-mcp)'s `Server::handle`. `mouser mcp serve --stdio` runs on `weirdnet_mcp::stdio::bridge`. Mouser keeps the transport: the listener, `Mcp-Session-Id` sessions, the bearer token, Host/Origin checks, body and connection caps, and TLS.
- [`crates/mouser-app/src/mcp_tools.rs`](crates/mouser-app/src/mcp_tools.rs) holds the 27 tools as a static table of annotated `ToolSpec`s. Tool names, parameter types, required lists and annotations are unchanged. Parameters that had no description now have one, because weirdnet-mcp always writes one. The chat calls the tools through `AppTools::run`.
- A golden test, `tools_list_is_golden`, compares `tools/list` with [`crates/mouser-app/tests/golden/mcp_tools_list.json`](crates/mouser-app/tests/golden/mcp_tools_list.json). `MOUSER_TEST_BLESS=1` rewrites the file after a deliberate change.
- What clients can notice:
  - An unknown protocol version is answered with 2025-06-18; it was 2025-11-25, which is still answered when a client asks for it.
  - Protocol 2024-11-05 is now accepted.
  - A stdio bridge request that can't reach Mouser fails with JSON-RPC error -32603; it was -32000, and the message is the same.
- The plan now has Phase 2 marked done, gaps G19 to G21, and a new Phase 10: agent mode on the Ymir model pool through weirdnet-agent, next to the local model. Phase 10 is not started.
- `weirdnet-mcp = "0.5.2"` resolves to 0.5.3, which has no API changes. `Cargo.lock` isn't committed, so nothing pins the exact version.

**Upstream gaps (for weirdnet-mcp)**

- G19: weirdnet-mcp's protocol version list is missing 2025-11-25, and an app can't extend it. Mouser's `EXTRA_VERSIONS` shim answers 2025-11-25 when a client asks for it.
- G20: `Tools::call` gets only the bearer token, not the caller. Mouser's permission broker keys on the session's client name, so Mouser's `Tools` trait adds it through a per-request adapter, `ForClient`.
- G21 (optional): a blocking (no tokio) Streamable HTTP front end for weirdnet-mcp. Mouser keeps its own until then.

**Checks**

- `cargo fmt --check`: clean.
- `cargo clippy -p mouser-core -p mouser-app --all-targets -- -D warnings`: clean.
- `mouser-core` tests: the whole suite passed, including the new `rpc_through_weirdnet_mcp` test.
- `mouser-app` tests: every test passed in a virtual display (xvfb), including the golden test and the 3 MCP CLI tests.
- Flaky test: `mcp_server::tests::bind_preferred_falls_back_when_busy` failed once and then passed on the full run. It frees a port and binds it again while other tests run in parallel, so another test can take it first. This change didn't touch that code.
- GitHub CI: not checked before the push. The CI workflow runs only on tags and manual runs.

**Not tested**

- A live session with an outside MCP client (an editor or agent) against a running Mouser window.
- macOS and Windows builds.

**What's left / next**

- Send G19 to G21 to the weirdnet-mcp maintainers.
- Phase 3, local models on weirdnet-model, starts when m3talsmith says go.

## 2026-11-20: MCP certificates move to weirdnet-tls 0.3

Author: m3talsmith. Commit: [`c973e26`](https://github.com/WeirdBrains/mouser/commit/c973e26).

**What changed**

- [`crates/mouser-core/src/mcp_acme.rs`](crates/mouser-core/src/mcp_acme.rs) and [`crates/mouser-app/src/mcp_cli.rs`](crates/mouser-app/src/mcp_cli.rs) use [weirdnet-tls](https://crates.io/crates/weirdnet-tls) 0.3.0 in place of Mouser's local workarounds for gaps G13 to G18:
  - name checks use `cert::valid_name`;
  - the DNS hook is `HookDns::with_prefix("MOUSER_ACME_")`, so Windows hooks need no `call` wrapper;
  - `cert status` reads `Acme::schedule`;
  - forced renewals use `Acme::trigger_renewal`, and the one-day guard stays;
  - `mouser mcp cert renew` answers on `Acme::serve_challenge`;
  - the 1.7 files are imported through `store::Legacy` and `Acme::import`.
- Net about 80 lines fewer (+113/−193). Config keys, the CLI and the files directory are unchanged.

**Checks**

- `mouser-core` tests passed, including the `mcp_acme` tests on weirdnet-tls's `MockCa`.
- `mouser-app` MCP CLI tests: 3 of 3 passed.
- `cargo clippy -- -D warnings`: clean.
- GitHub CI: the Docs site run for this commit succeeded. The CI workflow runs only on tags and manual runs.

**Not tested**

- A live certificate from Let's Encrypt; only the mock CA was used.
- A DNS hook on Windows.

**What's left / next**

- Nothing for weirdnet-tls. Phase 2 of the plan came next (entry above).
