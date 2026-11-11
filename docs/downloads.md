# Installing and updating plugins and engines

Mouser installs plugins and engines from a name, a GitHub release, a URL, a
local folder or archive, or a crate. Every download is checked before
anything runs, and each item gets its own version folder so you can roll back.
Plugins and engines use the same verbs, flags and output:

| verb | `mouser plugin …` | `mouser engine …` | `esc :` line |
|---|---|---|---|
| install | `mouser plugin install SRC` | `mouser engine install SRC` | `:plugin install SRC`, `:pi SRC` / `:engine install SRC`, `:ei SRC` |
| update | `mouser plugin update NAME\|--all` | `mouser engine update NAME\|--all` | `:plugin update [NAME]`, `:pu` / `:engine update`, `:eu` |
| rollback | `mouser plugin rollback NAME` | `mouser engine rollback NAME` | `:prb NAME` / `:erb NAME` |
| remove | `mouser plugin remove NAME` | `mouser engine remove NAME` | `:prm NAME` / `:erm NAME` |
| list | `mouser plugin list` | `mouser engine list` | `:pl` / `:el` |
| retry | `mouser plugin retry` | `mouser engine retry` | `:plugin retry`, `:pr` / `:engine retry`, `:er` |
| cancel | `mouser plugin cancel ID\|--all` | `mouser engine cancel ID\|--all` | `:cancel ID\|all`, `:pc` / `:ec` |

Every verb takes `--yes` (don't ask), `--version VERSION` and `--wait` (the
CLI always waits; on the `esc :` line downloads run in the background).
`remove` is also spelled `uninstall`. `mouser downloads` lists the running
Mouser's jobs and `mouser downloads --cancel ID|all` stops them; on the `esc :`
line that's `:downloads` and `:cancel`. `mouser --install SRC` (repeatable)
starts Mouser and installs in the background; with `--wait` it installs
without a window and exits (add `--yes`, since nothing can ask).

## Sources

- **A name** (`commander`, `chat`, `preferences`, `mouser`, `skia`): an official item
  from the `plugins.lock` compiled into this Mouser, at the version it pins. Each
  comes from the signed release of its WeirdBrains repo (`WeirdBrains/mouser-commander`,
  `mouser-chat`, `mouser-preferences`, `mouser-engine-wgpu`, `mouser-engine-skia`).
- **`gh:Owner/Repo[@tag]`**: a GitHub release (the latest without a tag). If
  the release has `mouser-plugin.json` (`{"version", "key", "assets": [{"target",
  "url", "sha256", "size", "minisig"}]}`) it is used; otherwise the asset named
  `NAME-VERSION-TARGET.EXT` for this machine, with `ASSET.sha256`,
  `ASSET.minisig` and `minisign.pub` beside it when the author publishes them.
- **`https://…`**: an asset (its `.sha256` and `.minisig` are looked for beside
  it, and `minisign.pub` in the same folder), or a release manifest ending in
  `.json` or `.toml` in the `mouser-plugin.json` shape.
- **A path**: a folder with `plugin.toml` or `engine.toml`, the manifest
  itself, a library next to `engine.toml`, or a `.tar.gz`/`.zip`.
- **`crate:NAME`**: `cargo binstall NAME` (needs cargo-binstall; mode plugins
  only), registered from the binary's `--manifest` output.

TARGET is a Rust triple: `x86_64-unknown-linux-gnu` (then `-musl`),
`aarch64-…`, `universal-apple-darwin` (then the arch's own),
`x86_64-pc-windows-msvc`. With no asset for this machine the error lists the
targets the release has.

## What is checked

In this order; any failure stops that install and nothing else:

1. HTTPS only, on every redirect.
2. A size limit of 200 MB.
3. sha256: from the lock, the release manifest or `ASSET.sha256`. Official
   items must have one.
4. A minisign signature. Official items must verify against the WeirdBrains
   key pinned in Mouser (minisign key `61BA1720747F9146`,
   `RWRGkX90IBe6YaOypKF9r1wz4OqpCp3Ggb1Ag2omAzxTRaqoHzsNQSdP`). Third-party items verify against their
   author's `minisign.pub`. Mouser remembers that key the first time
   (`$MOUSER_HOME/plugins/keys.toml`, by source) and refuses a later file signed
   with another key, or unsigned, from the same source. An unsigned item
   installs only after a warning.
5. The archive is unpacked into a temporary folder. Absolute paths, `..`,
   links and devices are refused.
6. A probe: an engine library is loaded in a separate process
   (`mouser __engine-probe`), never in Mouser, and must report the name in its
   manifest. A plugin's `plugin.toml` must parse.
7. The plugin API or engine ABI range.
8. macOS: `codesign --verify` (a warning when unsigned), then the quarantine
   flag is cleared. Windows: Authenticode (a warning when unsigned).

Then a card shows the name, version, source, signer, the permissions it asks
for and any warnings, with Install and Cancel (in a window it is a popup; on
the CLI it is `Install? [Y/n]`: Enter or `y` installs, `n` cancels, and no
answer (end of input, no terminal) cancels; `--yes` skips it). Official items on first run
don't ask. A local folder asks only when an update adds permissions. Every
install, update and rollback goes to `$MOUSER_HOME/logs/plugins-audit.log`.

## Versions, update and rollback

An install goes to `$MOUSER_HOME/plugins/<name>/<version>/` (engines:
`$MOUSER_HOME/engines/<name>/<version>/`). The `current` file switches to it
only after the checks pass. `<name>/source.toml` records where it came from,
for `update`. The newest two versions are kept. `rollback` switches `current`
to the other one, and `remove` deletes every version. An install from before
this layout (`<name>/plugin.toml`) is kept as `<name>/previous/`.

## The download manager

Installs and updates run on their own threads, never the UI or render thread.
At most 4 run at once, and at most 2 per server. Progress shows on the status
line; `:downloads` lists the jobs with their state and why one failed. A
running Mouser writes the list to `$MOUSER_HOME/downloads/jobs.toml` and picks up
`mouser downloads --cancel` requests from there.

## First run and the offline bundle

On start, Mouser queues every `plugins.lock` item that isn't installed or
shipped beside it. They download in parallel while the terminal works on the
basic renderer, and each is used as soon as its own download verifies. If one
fails (offline, say), Mouser carries on as a plain terminal with a note, and
`esc :plugin retry` tries again.

The same start also updates official items installed from the lock at an older
version than this Mouser pins, so updating Mouser brings its plugins along (Mouser
1.5 moved chat to 0.1.3 and commander to 0.1.5; 1.6 moved chat to 0.1.4; 1.7 moves chat to 0.1.5). An update that asks for a new
permission shows its card first. Left alone: items installed from `gh:`, a path
or a crate, ones at or past the pin, and ones rolled back from the pinned version
(`mouser plugin rollback`).

The `-full` offline bundle (`mouser-<target>-full.tar.gz`, `.zip` on Windows,
on every release) carries the lock's archives for its target, every engine and
official plugin with its `.minisig`, in `plugins/` beside
Mouser (`Contents/Resources/plugins` on macOS; `MOUSER_OFFLINE_BUNDLE` names
another folder). First run installs from there with the same checks and no
network.

Mouser's own engines also ship unpacked in `engines/` beside it ([Engines](./engines.md)).
