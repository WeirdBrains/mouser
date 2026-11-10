import 'package:jaspr/dom.dart';
import 'package:jaspr/server.dart';

class HomePage extends StatelessComponent {
  const HomePage({super.key});

  @override
  Component build(BuildContext context) {
    return Component.fragment([
      Document.head(
        title: 'Mouser — a fast GPU terminal emulator',
        meta: {
          'description': 'Mouser is a terminal emulator written in Rust with GPU rendering, tabs, an SSH connection manager, compressed scrollback and the Fuzzball graphics and keyboard protocol.',
        },
        children: [
          link(rel: 'icon', href: 'assets/icon.png'),
          link(rel: 'stylesheet', href: 'style.css'),
        ],
      ),
      a(classes: 'skip', href: '#main', [
      .text('Skip to content'),
      ]),
      header(classes: 'nav light', [
      a(classes: 'brand', href: '#top', [
      img(classes: 'logo', src: 'assets/logo.svg', alt: '', attributes: {'width': '36', 'height': '36'}),
      .text(' mouser'),
      ]),
      button(classes: 'menu', attributes: {'aria-expanded': 'false', 'aria-controls': 'links'}, [
      .text('Menu'),
      ]),
      nav(id: 'links', attributes: {'aria-label': 'Main'}, [
      a(href: 'screenshots/', [
      .text('Screenshots'),
      ]),
      a(href: '#install', [
      .text('Install'),
      ]),
      a(href: 'docs/user/', [
      .text('User Guides'),
      ]),
      a(href: 'docs/developer/', [
      .text('Developer Guides'),
      ]),
      a(href: 'docs/mcp/', [
      .text('MCP Guides'),
      ]),
      a(href: 'docs/api/', [
      .text('API Guides'),
      ]),
      a(href: 'fuzzball/', [
      .text('Fuzzball'),
      ]),
      a(href: 'https://github.com/WeirdBrains/mouser', [
      .text('GitHub'),
      ]),
      ]),
      ]),
      main_(id: 'main', [
      section(id: 'top', classes: 'hero light', [
      div(classes: 'hero-intro', [
      p(classes: 'prompt wordmark', [
      img(classes: 'logo', src: 'assets/logo.svg', alt: '', attributes: {'width': '48', 'height': '48'}),
      span([
      .text('mouser'),
      ]),
      ]),
      h1([
      .text('A fast GPU terminal emulator, '),
      span([
      .text('written in Rust.'),
      ]),
      ]),
      ]),
      figure(classes: 'shot beautiful', attributes: {'aria-label': 'Mouser is beautiful'}, [
      div(classes: 'media-frame hero-fill', [
      video(id: 'beautiful-video', autoplay: true, muted: true, loop: true, preload: Preload.metadata, poster: 'assets/mouser-is-beautiful-poster.png?v=14', attributes: {'playsinline': '', 'width': '1350', 'height': '944'}, [
      source(src: 'assets/mouser-is-beautiful.webm?v=14', type: 'video/webm'),
      source(src: 'assets/mouser-is-beautiful.mp4?v=14', type: 'video/mp4'),
      ]),
      ]),
      ]),
      div(classes: 'hero-meta', [
      p(classes: 'release', [
      a(href: 'https://github.com/WeirdBrains/mouser/releases/tag/v1.5.0', [
      .text('Version 1.5.0'),
      ]),
      .text(' · BSD-3-Clause'),
      ]),
      p(classes: 'cta', [
      a(classes: 'btn', href: '#install', [
      .text('Install'),
      ]),
      a(classes: 'btn ghost', href: 'https://github.com/WeirdBrains/mouser', [
      .text('View on GitHub'),
      ]),
      ]),
      p(classes: 'lede hero-lede', [
      .text('Mouser runs your login shell in a PTY, parses VT/xterm escape sequences and renders the grid on the GPU. A file commander, a media player and an optional AI chat dock above your prompt. Linux first; macOS and Windows builds too.'),
      ]),
      ]),
      ]),
      section(id: 'features', attributes: {'aria-labelledby': 'f-h'}, [
      h2(id: 'f-h', [
      .text('Features'),
      ]),
      ul(classes: 'grid', [
      li([
      h3([
      .text('GPU engines'),
      ]),
      p([
      .text('Render engines are plugins: '),
      code([
      .text('mouser'),
      ]),
      .text(' (wgpu), Skia, the CPU renderer, or your own via a C ABI. Switch live with '),
      code([
      .text('mouser engine use'),
      ]),
      .text('.'),
      ]),
      ]),
      li([
      h3([
      .text('Crash-proof windows'),
      ]),
      p([
      .text('On Linux and macOS the shells live in a session process; a GPU driver or engine crash restarts only the window, with your shells, scrollback and tabs intact.'),
      ]),
      ]),
      li([
      h3([
      .text('Commander'),
      ]),
      p([
      .text('A file manager docked above the prompt: list, grid and carousel views, image and text previews, and a video and audio player, local or over SSH.'),
      ]),
      ]),
      li([
      h3([
      .text('Chat and MCP'),
      ]),
      p([
      .text('An optional agent ('),
      code([
      .text('esc :a'),
      ]),
      .text(') that sees your screen and acts through tools, with an approval card for every command. Mouser is also an MCP server and client.'),
      ]),
      ]),
      li([
      h3([
      .text('Your models'),
      ]),
      p([
      .text('OpenAI, Anthropic, Gemini, xAI, OpenRouter, Kimi or any compatible server, or the built-in local Gemma model ('),
      code([
      .text('mouser model local enable'),
      ]),
      .text('; it asks before downloading). Keys stay in the OS keychain.'),
      ]),
      ]),
      li([
      h3([
      .text('Tabs and connections'),
      ]),
      p([
      .text('Each tab has its own PTY, screen, scrollback and title. A sidebar for SSH hosts from your ssh config, plus Waypipe, tunnels and X11.'),
      ]),
      ]),
      li([
      h3([
      .text('Compressed scrollback'),
      ]),
      p([
      .text('Chunked history compressed with Zstd or LZ4 when idle, keeping colours, and searchable across compressed chunks.'),
      ]),
      ]),
      li([
      h3([
      .text('Fuzzball protocol'),
      ]),
      p([
      .text('Mouser\'s own open graphics and keyboard protocol. Also a Rust crate: see '),
      a(href: 'fuzzball/', [
      .text('Fuzzball'),
      ]),
      .text('. Fallbacks: Kitty keyboard and Kitty graphics.'),
      ]),
      ]),
      li([
      h3([
      .text('Modern xterm'),
      ]),
      p([
      .text('Truecolour, curly and coloured underlines, wide characters, mouse, focus, bracketed paste, OSC 8 links and OSC 52.'),
      ]),
      ]),
      li([
      h3([
      .text('Plugins'),
      ]),
      p([
      .text('Modes, engines, model providers and MCP tools install as plugins with explicit permissions: '),
      code([
      .text('mouser plugin list'),
      ]),
      .text('.'),
      ]),
      ]),
      ]),
      ]),
      section(id: 'install', classes: 'light', attributes: {'aria-labelledby': 'i-h'}, [
      h2(id: 'i-h', [
      .text('Install'),
      ]),
      div(classes: 'cols install', [
      div([
      h3([
      .text('Homebrew (macOS)'),
      ]),
      pre([
      code([
      .text('''brew tap weirdbrains/mouser https://github.com/WeirdBrains/mouser
      brew trust weirdbrains/mouser   # current Homebrew asks for this once
      # CLI
      brew install weirdbrains/mouser/mouser
      # App
      brew install --cask weirdbrains/mouser/mouser'''),
      ]),
      ]),
      ]),
      div([
      h3([
      .text('Arch Linux'),
      ]),
      pre([
      code([
      .text('''# Trust the package key once (id 6318D43051C4DBBC)
      curl -sL https://mouser.weirdbrains.com/mouser-release-key.asc | sudo pacman-key --add -
      # or: sudo pacman-key --keyserver hkps://keyserver.ubuntu.com --recv-keys 3561BE80D8B3E767CF0B0B176318D43051C4DBBC
      sudo pacman-key --lsign-key 6318D43051C4DBBC
      sudo pacman -U https://github.com/WeirdBrains/mouser/releases/download/v1.5.0/mouser-bin-1.5.0-1-x86_64.pkg.tar.zst
      # Or build main from source
      git clone https://github.com/WeirdBrains/mouser
      cd mouser/packaging/arch/mouser-git && makepkg -si'''),
      ]),
      ]),
      ]),
      div([
      h3([
      .text('Debian / Ubuntu'),
      ]),
      pre([
      code([
      .text('''curl -LO https://github.com/WeirdBrains/mouser/releases/download/v1.5.0/mouser_1.5.0_amd64.deb
      sudo apt install ./mouser_1.5.0_amd64.deb'''),
      ]),
      ]),
      ]),
      div([
      h3([
      .text('Fedora / RHEL'),
      ]),
      pre([
      code([
      .text('''curl -LO https://github.com/WeirdBrains/mouser/releases/download/v1.5.0/mouser-1.5.0-1.x86_64.rpm
      sudo dnf install ./mouser-1.5.0-1.x86_64.rpm'''),
      ]),
      ]),
      ]),
      div([
      h3([
      .text('Windows'),
      ]),
      p([
      .text('Download and run the '),
      a(href: 'https://github.com/WeirdBrains/mouser/releases/download/v1.5.0/MouserSetup-1.5.0.exe', [
      .text('installer (MouserSetup-1.5.0.exe)'),
      ]),
      .text(', or use the '),
      a(href: 'https://github.com/WeirdBrains/mouser/releases/download/v1.5.0/mouser-x86_64-pc-windows-msvc.zip', [
      .text('portable zip'),
      ]),
      .text('. From PowerShell:'),
      ]),
      pre([
      code([
      .text('''curl.exe -LO https://github.com/WeirdBrains/mouser/releases/download/v1.5.0/MouserSetup-1.5.0.exe
      .\\MouserSetup-1.5.0.exe'''),
      ]),
      ]),
      ]),
      div([
      h3([
      .text('From source with cargo'),
      ]),
      pre([
      code([
      .text('''git clone https://github.com/WeirdBrains/mouser
      cd mouser
      cargo run -p mouser-app --release'''),
      ]),
      ]),
      ]),
      div([
      h3([
      .text('GitHub Releases'),
      ]),
      p([
      .text('Each '),
      a(href: 'https://github.com/WeirdBrains/mouser/releases', [
      .text('release'),
      ]),
      .text(' ships:'),
      ]),
      ul(classes: 'list', [
      li([
      .text('Linux: '),
      code([
      .text('.deb'),
      ]),
      .text(', '),
      code([
      .text('.rpm'),
      ]),
      .text(', Arch '),
      code([
      .text('mouser-bin'),
      ]),
      ]),
      li([
      .text('macOS: '),
      code([
      .text('Mouser.app'),
      ]),
      code([
      .text('.dmg'),
      ]),
      ]),
      li([
      .text('Windows: portable '),
      code([
      .text('.zip'),
      ]),
      .text(' and installer '),
      code([
      .text('.exe'),
      ]),
      ]),
      ]),
      ]),
      ]),
      p([
      .text('Every package includes the local model engine (llama.cpp: Vulkan on Linux, Metal on macOS, CPU on Windows); Linux packages depend on the Vulkan loader.'),
      ]),
      p([
      .text('Shell integration (silent '),
      code([
      .text('cd'),
      ]),
      .text(' from commander): '),
      code([
      .text('eval "\$(mouser shell-integration zsh)"'),
      ]),
      .text(' in '),
      code([
      .text('~/.zshrc'),
      ]),
      .text(' (also '),
      code([
      .text('bash'),
      ]),
      .text(', or '),
      code([
      .text('mouser shell-integration fish | source'),
      ]),
      .text(').'),
      ]),
      p([
      .text('Optional terminfo entry: '),
      code([
      .text('tic -x -o ~/.terminfo extra/mouser.terminfo'),
      ]),
      ]),
      ]),
      section(id: 'start', attributes: {'aria-labelledby': 's-h'}, [
      h2(id: 's-h', [
      .text('Quick start'),
      ]),
      p([
      .text('Launch Mouser and your login shell starts in the first tab. Open the sidebar to add or pick an SSH host; clicking it opens '),
      code([
      .text('ssh <alias>'),
      ]),
      .text(' in a new tab.'),
      ]),
      table([
      caption([
      .text('Keybindings'),
      ]),
      thead([
      tr([
      th(scope: 'col', [
      .text('Keys'),
      ]),
      th(scope: 'col', [
      .text('Action'),
      ]),
      ]),
      ]),
      tbody([
      tr([
      td([
      Component.element(tag: 'kbd', children: [
      .text('Ctrl')
      ]),
      .text('+'),
      Component.element(tag: 'kbd', children: [
      .text('Shift')
      ]),
      .text('+'),
      Component.element(tag: 'kbd', children: [
      .text('T')
      ]),
      ]),
      td([
      .text('New tab'),
      ]),
      ]),
      tr([
      td([
      Component.element(tag: 'kbd', children: [
      .text('Ctrl')
      ]),
      .text('+'),
      Component.element(tag: 'kbd', children: [
      .text('Shift')
      ]),
      .text('+'),
      Component.element(tag: 'kbd', children: [
      .text('W')
      ]),
      ]),
      td([
      .text('Close tab'),
      ]),
      ]),
      tr([
      td([
      Component.element(tag: 'kbd', children: [
      .text('Ctrl')
      ]),
      .text('+'),
      Component.element(tag: 'kbd', children: [
      .text('Tab')
      ]),
      .text(' / '),
      Component.element(tag: 'kbd', children: [
      .text('Ctrl')
      ]),
      .text('+'),
      Component.element(tag: 'kbd', children: [
      .text('Shift')
      ]),
      .text('+'),
      Component.element(tag: 'kbd', children: [
      .text('Tab')
      ]),
      ]),
      td([
      .text('Next / previous tab'),
      ]),
      ]),
      tr([
      td([
      Component.element(tag: 'kbd', children: [
      .text('Alt')
      ]),
      .text('+'),
      Component.element(tag: 'kbd', children: [
      .text('1')
      ]),
      .text('…'),
      Component.element(tag: 'kbd', children: [
      .text('9')
      ]),
      ]),
      td([
      .text('Jump to tab'),
      ]),
      ]),
      tr([
      td([
      Component.element(tag: 'kbd', children: [
      .text('Ctrl')
      ]),
      .text('+'),
      Component.element(tag: 'kbd', children: [
      .text('Shift')
      ]),
      .text('+'),
      Component.element(tag: 'kbd', children: [
      .text('V')
      ]),
      ]),
      td([
      .text('Paste (bracketed when the app asks)'),
      ]),
      ]),
      tr([
      td([
      Component.element(tag: 'kbd', children: [
      .text('Ctrl')
      ]),
      .text('+click'),
      ]),
      td([
      .text('Open an OSC 8 link'),
      ]),
      ]),
      tr([
      td([
      Component.element(tag: 'kbd', children: [
      .text('Shift')
      ]),
      .text('+mouse'),
      ]),
      td([
      .text('Bypass mouse reporting when an app grabbed the mouse'),
      ]),
      ]),
      tr([
      td([
      .text('Mouse wheel'),
      ]),
      td([
      .text('Scrollback'),
      ]),
      ]),
      ]),
      ]),
      ]),
      ]),
      footer(classes: 'light', [
      p([
      a(href: 'https://github.com/WeirdBrains/mouser', [
      .text('GitHub'),
      ]),
      .text(' · '),
      a(href: 'https://github.com/WeirdBrains/mouser/blob/main/CHANGELOG.md', [
      .text('Changelog'),
      ]),
      .text(' · '),
      a(href: 'https://github.com/WeirdBrains/mouser/blob/main/docs/terminal.md', [
      .text('Terminal docs'),
      ]),
      .text(' · '),
      a(href: 'https://github.com/WeirdBrains/mouser/blob/main/LICENSE', [
      .text('BSD-3-Clause'),
      ]),
      .text(' · © 2026 Weird Brains'),
      ]),
      ]),
      script(src: 'main.js'),
    ]);
  }
}
