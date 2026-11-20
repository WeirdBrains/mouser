# Mouser ← Weirdnet integration plan

Drafted Nov 18, 2026 after a read-only survey of the checkouts on the maintainer's build machine. Nothing was changed, committed or launched. Effort numbers are estimates.

## Status (Nov 20, 2026)

| Phase | State | Commits |
|---|---|---|
| 0. Setup | Done | `3f87c0a` helpers 0.3.2 vendored (`validate` passes); `weirdnet-tls` 0.2.0 added to `[workspace.dependencies]` from crates.io in `08675df` |
| 1. weirdnet-tls | Done | `08675df` Let's Encrypt on weirdnet-tls; `9cff03a` self-signed and PEM certificates through it (no direct `rcgen` or `rustls-pemfile` left in mouser-core) |
| 2 to 9 | Not started | |

- Phase 0 takes Weirdnet crates from crates.io: each is published at its `main` (`weirdnet-tls` main is 0.2.0 plus a docs-only commit). Later phases add their crates the same way when they start.
- Current crates.io versions on Nov 20: weirdnet-tls 0.2.0, weirdnet-agent 0.4.5, weirdnet-mcp 0.5.2, weirdnet-model 0.4.3 (older agent, mcp and model releases are yanked). mcp 0.5.2 and model 0.4.3 both need agent `^0.4`, so the version trap below is gone (G8 is done) and phases 2 to 5 use those three together.
- Phase 1 behavior: same config keys, challenges, CLI and files directory. The store layout under `$MOUSER_HOME/mcp/acme/` is now weirdnet-tls's (`account/`, `cert/`, `state/`, one certificate set per CA); the 1.7 files are copied in on first start. Renewal follows ARI, else two thirds of the lifetime (the same 30 days before expiry for a 90-day certificate). Hooks get `MOUSER_ACME_*` and `WEIRDNET_TLS_*`. A forced renewal no longer waits out the failure back-off (weirdnet-tls behavior); the one-day guard stays.
- Tests: `mcp_acme` runs on weirdnet-tls's `MockCa` (tls-alpn-01 and http-01 issue, hot swap from the `renew-now` trigger, schedule, back-off, 1.7 migration, hook variables), plus a `mouser mcp cert status` CLI test; mouser-core's whole suite passes.

## Summary

- Mouser (`3a36891`, workspace 1.7.0) uses no Weirdnet crate today, not even `weirdnet-model`, which was extracted from Mouser.
- Ready now, biggest wins:
  - `mcp_acme.rs` moves to `weirdnet-tls` 0.2.0, which AGENTS.md requires.
  - Local models and llama.cpp move to `weirdnet-model` 0.2.0.
  - The MCP JSON-RPC core and client move to `weirdnet-mcp` 0.2.0.
  - The chat agent loop and approvals move to `weirdnet-agent`.
- Version trap (Nov 18; resolved Nov 20, see Status): only `weirdnet-agent =0.1.0`, `weirdnet-mcp =0.2.0` and `weirdnet-model =0.2.0` fit together, which is what Morph pinned. Agent 0.2.0 was published, but the model and mcp releases still depended on agent 0.1.0.
- Sync is a new feature, not a swap. `weirdnet-sync` 0.4.0 needs Weirdnet identity, a device roster and device linking (`weirdnet-keys`, `-net`, `-device-link`). Mouser has none of these. `filesync.rs` copies files over ssh and isn't device sync.
- Mouser has no frameless code today, on Linux included. `mouser-render/src/lib.rs:539` creates a plain winit 0.30.13 window, and it only looks chromeless under Hyprland. GNOME Wayland draws a winit or adwaita frame, and X11 window managers draw their own. Morph already has toolkit-free code that decides which buttons to show and detects tiling window managers (`morph-ui/src/chrome.rs`); it should move to a shared crate.

## Commits and versions looked at

| Repo | Commit | Version |
|---|---|---|
| mouser | `3a368910603b` | 1.7.0 (helpers 0.1.1 vendored) |
| morph | `57b73113c6a7` (origin/main) | |
| weirdnet-agents | `d00017c582f7` | |
| weirdnet-model | `67b78808c5c6` | 0.2.0 |
| weirdnet-agent | `49c20e926562` | 0.2.0 (adds `remote::infer`, `remote::task`) |
| weirdnet-mcp | `5423dd9d55d8` | 0.2.0, plus unreleased move to agent `=0.2.0` |
| weirdnet-net | `c6660b2e2000` | 0.7.0 |
| weirdnet-sync | `cc115de703d6` | 0.4.0 |
| weirdnet-tls | `9c1790c3763c` | 0.2.0 |
| weirdnet-themes | `936dd32a58a6` | 0.1.0 |
| weirdnet-keys | `bd5538a7425f` | 0.1.1 |
| weirdnet-storage | `8152b237b33e` | 0.1.0 |
| weirdnet-ext | `dc0b15894c9f` | 0.1.0 |
| weirdnet-device-link | `51d2d07dd422` | 0.1.0 |
| weirdnet-schemes | `c6817eba3481` | 0.1.0 |
| weirdnet-project-helpers | `cd37a355650b` | 0.3.2 |

No window or chrome crate exists yet.

Morph pins `weirdnet-keys =0.1.0`, `weirdnet-net =0.7.0`, `weirdnet-sync =0.4.0`, `weirdnet-agent =0.1.0`, `weirdnet-mcp =0.2.0` and `weirdnet-model =0.2.0` (default features off), plus `weirdnet-storage` and `weirdnet-schemes` 0.1.0.

## Mapping

| Mouser module (LOC) | Weirdnet crate | Action | Notes |
|---|---|---|---|
| `mouser-core/src/mcp_acme.rs` (1115) | weirdnet-tls 0.2.0 | Replace | Same challenges, file store, hot-swapped SNI resolver and backoff. Adds ARI renewal. |
| `mcp_server.rs` (750), certificates | weirdnet-tls `Resolver` | Partial | Drops direct `rcgen` and `rustls-pemfile`. |
| `mcp_server.rs`, JSON-RPC | weirdnet-mcp `Server::handle`, `ToolSpec` | Partial | Keep Mouser's listener, bearer token, Host/Origin checks and body caps. The stdio bridge uses `weirdnet_mcp::stdio`. |
| `mouser-app/src/mcp_tools.rs` (776) | weirdnet-mcp `ToolSpec` | Keep, rewrite table | App-specific tools (tabs, screen, files, sync, themes). |
| `mcp.rs` (633) + `mcp_client.rs` (417) | weirdnet-mcp client and agent | Replace after G3 | Needs custom CA, env scrubbing and process-tree kill upstream first. Keep reconnect and broker routing. |
| `agent.rs` (633) | weirdnet-agent session, Policy, Approver | Replace core | `AgentHost::check` and approval cards map onto `Approver` and `Policy`. |
| `permissions.rs` (266) | | Keep | Per-plugin capability broker is Mouser-specific. |
| `models/*` hosted providers (~1700) | weirdnet-agent `ModelBackend` | Wrap, then upstream (G4) | |
| `models/hf.rs` (923) | weirdnet-model hf, download, store | Partial | Search, size lookup, README summary, quant labels and split models are gaps G1 and G2. |
| `models/local.rs` (550) + `mouser-llm` (175) | weirdnet-model catalog, worker, llama features | Replace | Drop `mouser-llm`. Gains an unsupported-CPU error instead of a SIGILL crash. Keep Mouser model ids as aliases. |
| `models/setup.rs`, `hf_pick.rs`, `model_cli.rs` | | Keep, rewire | |
| `downloads.rs`, `secrets.rs`, `keys.rs` | | Keep | Possible later upstream (G6). |
| `themes.rs` + `appearance.rs` | weirdnet-themes 0.1.0 | Keep for now | Terminal ANSI palettes aren't in weirdnet-themes yet (G5). |
| `filesync.rs`, `fetch.rs`, `remote.rs` | | Keep | Candidates for a future chunked-transfer crate (G7). |
| `plugins.rs`, `install.rs` | weirdnet-ext 0.1.0 | Separate plan | Would change the plugin protocol, so it's a major-version project. |
| Window creation `mouser-render/src/lib.rs:539` | new shared crate + Morph `chrome.rs` | Upstream, then adopt | Phases 6 and 7. |
| `scripts/weirdnet_release.py` | project-helpers 0.3.2 | Re-vendor | Needed for `[[requires]]` pins. |

## Phases

All testing is headless. Each phase is a small commit to `main` with a CHANGELOG entry, updated docs, and passing fmt, clippy and tests.

0. **Setup, about half a day. Done.** Re-vendor helpers 0.3.2. Add workspace weirdnet deps as git main for development; releases pin `=X.Y.Z` from crates.io. (Done with crates.io versions, which match main.)
1. **weirdnet-tls replaces `mcp_acme.rs`, 1.5 to 2 days. Done.** Keep or migrate `$MOUSER_HOME/mcp/acme/`, use `Resolver`, map `mouser mcp cert renew` onto it. Test with `MockCa` and a hot swap. Behavior doesn't change.
2. **weirdnet-mcp server core, 1.5 days.** Dispatch through `Server::handle`, turn the tools into annotated `ToolSpec`s, and add golden `tools/list` tests so clients keep working.
3. **weirdnet-model for local models, 3 to 4 days.** Map `local-llm` onto `weirdnet-model/llama` (vulkan on Linux, metal on macOS). Keep the per-repo model folder layout. Remove `mouser-llm`. Test with the mock engine and a mock Hub.
4. **weirdnet-mcp client, 2 days, after G3.**
5. **weirdnet-agent for chat, 4 to 5 days.** Use agent 0.4 with mcp 0.5 and model 0.4 (was: pin agent 0.1.0). Port `Chat` to `AgentSession`, with terminal and screen as local tools and MCP through `McpSource`, following Morph's `mcp_runtime.rs`. Hosted providers go through a `ModelBackend` adapter. The chat UI and chat file format stay the same.
6. **Chromeless on every desktop platform, 4 to 6 days.**
   - Linux: `with_decorations(false)` and a caption band with drag, double-click to maximize, right-click window menu, and invisible resize edges.
   - macOS: Morph's approach. A transparent, hidden-title, full-size content view keeps the native traffic lights and native resize, with a 78 px inset. winit can't do custom resize on macOS.
   - Windows: undecorated with the DWM shadow, our own minimize, maximize and close, and winit drag and resize. The Windows 11 snap-layouts flyout on the maximize button isn't reachable through winit (G9), so v1 ships without it.
   - The caption band is host-owned in `mouser-render`, not a decoration plugin, so plugin timeouts can't break the window controls.
   - Config: `[window] chrome = "auto" | "native"`.
   - Tests: layout unit tests, plus smoke tests in `env -i` under xvfb-run and headless weston. macOS and Windows build in CI only.
7. **Window buttons and tiling detection, 2 days, alongside 6.**
   - Upstream Morph's `chrome.rs` into a new shared crate (G10).
   - Detection order: an override first (env var plus `[window] buttons = "auto|all|close"`), then compositor sockets (`HYPRLAND_INSTANCE_SIGNATURE`, `SWAYSOCK`, `I3SOCK`, `NIRI_SOCKET`, `BSPWM_SOCKET`), then desktop names (hyprland, sway, i3, niri, river, bspwm, dwm, qtile, xmonad and others), and finally all three buttons.
   - Tiling shows close only. A later improvement could use Wayland's tiled state (G11).
   - Tests: table tests over fake environments.
8. **Morph's sync, agent and MCP stack, 2 to 3 weeks after a design pass.**
   - Prerequisites: a Mouser identity on weirdnet-keys and weirdnet-net, device linking, and a `weirdnet-mouser://` join scheme.
   - Sync candidates: themes, preferences, ssh connections, MCP server configs (started only for a trusted, unrevoked writer), plugin state, chats as sealed or chunked records, and approvals. API keys and tokens never go in plain records.
   - Remote inference and tasks on your other devices through agent 0.2.0 `remote::infer` and `remote::task`, with roster caller checks.
   - Upstream a generic op-log store (G12) instead of copying Morph's SQLite version.
   - Morph's own sync wiring is still settling: it had persistence fixes on Nov 18.
9. **Later:** UI colors from weirdnet-themes (after G5), and plugins on weirdnet-ext (major version).

## Upstream gaps (in weirdnet first)

- G1: weirdnet-model gets HF search, sizes, README summary and quant labels.
- G2: weirdnet-model gets split multi-file GGUF and a per-repo layout.
- G3: weirdnet-mcp client gets custom CA, env scrubbing and process-group kill.
- G4: hosted ModelBackend adapters (OpenAI-compatible, Anthropic, Gemini).
- G5: weirdnet-themes gets a terminal palette.
- G6: OS keychain store and download manager (optional).
- G7: a chunked transfer crate.
- G8: weirdnet-mcp and weirdnet-model releases on the current agent. Done: mcp 0.5.2 and model 0.4.3 on agent `^0.4`.
- G13: weirdnet-tls `cert::valid_name` accepts `localhost`, `*.localhost`, IP addresses and all-digit top labels, which no public CA certifies. Mouser checks those on top.
- G14: weirdnet-tls `HookDns` sets only `WEIRDNET_TLS_*`. An app-chosen variable prefix (or extra names) would let Mouser drop the `MOUSER_ACME_*` shell wrapper in `mcp_acme::dns_hook`, which on Windows relies on `call` re-expansion.
- G15: weirdnet-tls has no offline "next renewal" query (`Acme::load` leaves `Status::next_attempt` empty and `Acme::due` may go to the network and register an account). Mouser's `Certs::schedule` repeats the small due computation from the store for `mouser mcp cert status`.
- G16: weirdnet-tls `Acme` can't make its certificate the resolver's default (handshakes without SNI or for another name), and `Acme::trigger` can't force a renewal. Mouser sets the default from a status watcher and calls `renew_once(true)` for its cross-process `renew-now` file.
- G17: weirdnet-tls has no handshake-only TLS-ALPN-01 responder for when the app's own HTTPS listener isn't running; Mouser keeps a small `serve_challenge_only`.
- G18: weirdnet-tls `FileStore` doesn't read the per-app layouts it replaces; Mouser's `migrate_legacy` copies its 1.7 files in.
- G9: a Windows snap-layouts hit-test hook.
- G10: a new window-chrome policy crate from Morph's `chrome.rs`.
- G11: Wayland tiled state in winit (optional).
- G12: a persistent op-log store for apps.

## Risks

- Mixing agent versions loads two copies of the traits; with G8 done, keep agent, mcp and model on one agent line (0.4 today).
- Existing data has to keep working: ACME state, model ids and paths, and MCP tool names.
- llama.cpp packaging: either cpu-dispatch shared libraries or Morph's x86-64-v3 baseline.
- Chromeless on Windows loses the native resize border and the snap flyout, and the caption band can clash with decoration plugin bands.
- Sync touches identity, revocation and secrets, so it's a security-sensitive feature, not a refactor.

## Decisions (m3talsmith, Nov 18, 2026)

1. The shared window crate is named **`weirdnet-chrome`** (G10 approved).
2. **No Windows snap layouts in v1**, so G9 is deferred.
3. **Everything syncs:** themes, preferences, ssh connections, MCP configs, plugin state, chats and approvals. **Secrets sync only as sealed records.**
4. **Identity, device linking and sync (Phase 8) wait until Morph's sync settles.**

## Still open (smaller calls)

- Override variable: `MOUSER_WINDOW_BUTTONS` or a shared `WEIRDNET_WINDOW_BUTTONS`. My default is the shared name, with a Mouser config key on top.
- llama.cpp for releases: cpu-dispatch or an x86-64-v3 baseline. My default is Morph's x86-64-v3 baseline.
- Hosted model adapters: upstream them or keep them in Mouser. My default is to keep them in Mouser for now and upstream later.
- Order: Phases 0 to 5, with 6 and 7 in parallel. Phase 8 waits on Morph.
