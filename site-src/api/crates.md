---
title: The published mouser crates
---
# The published mouser crates

These crates are on crates.io, with API docs on docs.rs. Walkthroughs for each
are coming soon; until then, docs.rs and the linked guides are the reference.

| crate | what it is | docs | guide |
|---|---|---|---|
| [mouser-mode-api](https://crates.io/crates/mouser-mode-api) | protocol, UI model and SDK for mode plugins | [docs.rs](https://docs.rs/mouser-mode-api) | [Mode plugin protocol](../developer/mode-protocol.html) |
| [mouser-engine-abi](https://crates.io/crates/mouser-engine-abi) | the stable C ABI (v2) for rendering engines | [docs.rs](https://docs.rs/mouser-engine-abi) | [Rendering engines](../developer/engines.html) |
| [fuzzball](https://crates.io/crates/fuzzball) | encoder and decoder for the Fuzzball protocol | [docs.rs](https://docs.rs/fuzzball) | [Fuzzball protocol](../developer/protocol.html) |
| [mouser-commander](https://crates.io/crates/mouser-commander) | the commander mode as a plugin | [docs.rs](https://docs.rs/mouser-commander) | [Modes](../user/modes.html) |
| [mouser-chat](https://crates.io/crates/mouser-chat) | the chat mode as a plugin | [docs.rs](https://docs.rs/mouser-chat) | [Chat](../user/chat.html) |
| [mouser-preferences](https://crates.io/crates/mouser-preferences) | the preferences mode as a plugin | [docs.rs](https://docs.rs/mouser-preferences) | [Preferences](../user/preferences.html) |
| [mouser-engine-wgpu](https://crates.io/crates/mouser-engine-wgpu) | the wgpu renderer (engine `mouser`) as an engine library | [docs.rs](https://docs.rs/mouser-engine-wgpu) | [Rendering engines](../developer/engines.html) |
| [mouser-engine-skia](https://crates.io/crates/mouser-engine-skia) | the Skia renderer (engine `skia`) as an engine library | [docs.rs](https://docs.rs/mouser-engine-skia) | [Rendering engines](../developer/engines.html) |
| [mouser-engine-flat](https://crates.io/crates/mouser-engine-flat) | an example custom engine: CPU raster with softbuffer | [docs.rs](https://docs.rs/mouser-engine-flat) | [Rendering engines](../developer/engines.html) |

The plugin and engine crates are from the Mouser 1.4.0 split, which hasn't
shipped in a Mouser release yet.
