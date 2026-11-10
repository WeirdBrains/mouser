import 'package:jaspr/dom.dart';
import 'package:jaspr/server.dart';

class FuzzballPage extends StatelessComponent {
  const FuzzballPage({super.key});

  @override
  Component build(BuildContext context) {
    return Component.fragment([
      Document.head(
        title: 'Fuzzball — the terminal protocol crate from Mouser',
        meta: {
          'description': 'Fuzzball is a small family of terminal extensions for keyboard, graphics, shell actions and plugins, with a dependency-free Rust encoder and decoder crate.',
        },
        children: [
          link(rel: 'icon', href: '../assets/fuzzball-logo-512.png'),
          link(rel: 'stylesheet', href: '../style.css'),
          Component.element(tag: 'style', children: [
            .text('.fz-logo{height:auto;width:min(240px,60%);display:block;margin:auto}.spec h3 a{color:inherit}'),
          ]),
        ],
      ),
      a(classes: 'skip', href: '#main', [
      .text('Skip to content'),
      ]),
      header(classes: 'nav light', [
      a(classes: 'brand', href: '../', [
      img(classes: 'logo', src: '../assets/logo.svg', alt: '', attributes: {'width': '36', 'height': '36'}),
      .text(' mouser'),
      ]),
      button(classes: 'menu', attributes: {'aria-expanded': 'false', 'aria-controls': 'links'}, [
      .text('Menu'),
      ]),
      nav(id: 'links', attributes: {'aria-label': 'Main'}, [
      a(href: '../#features', [
      .text('Features'),
      ]),
      a(href: '../#install', [
      .text('Install'),
      ]),
      a(href: '../#start', [
      .text('Quick start'),
      ]),
      a(href: './', attributes: {'aria-current': 'page'}, [
      .text('Fuzzball'),
      ]),
      a(href: 'https://github.com/WeirdBrains/mouser', [
      .text('GitHub'),
      ]),
      ]),
      ]),
      main_(id: 'main', [
      section(id: 'top', classes: 'hero light', [
      div(classes: 'hero-text', [
      p(classes: 'prompt wordmark', [
      img(classes: 'logo', src: '../assets/fuzzball-logo.svg', alt: '', attributes: {'width': '48', 'height': '48'}),
      span([
      .text('fuzzball'),
      ]),
      ]),
      h1([
      .text('A terminal protocol, '),
      span([
      .text('and the crate that speaks it.'),
      ]),
      ]),
      p(classes: 'lede', [
      .text('Fuzzball is a small family of terminal extensions, implemented first in Mouser: structured keyboard events, cell-anchored graphics, silent shell actions and plugins. The '),
      code([
      .text('fuzzball'),
      ]),
      .text(' crate encodes and decodes it, with no dependencies, no I/O and no terminal library: bring your own reader and writer.'),
      ]),
      p(classes: 'release', [
      a(href: 'https://crates.io/crates/fuzzball', [
      .text('Version 1.0.0'),
      ]),
      .text(' · BSD-3-Clause · MSRV Rust 1.85'),
      ]),
      p(classes: 'cta', [
      a(classes: 'btn', href: '#get', [
      .text('Get the crate'),
      ]),
      a(classes: 'btn ghost', href: '#spec', [
      .text('Read the spec'),
      ]),
      ]),
      ]),
      figure(classes: 'shot', [
      img(classes: 'fz-logo', src: '../assets/fuzzball-logo-512.png', alt: 'Fuzzball logo: a matte, fuzzy pink pom-pom cat-toy ball', attributes: {'width': '512', 'height': '512'}),
      ]),
      ]),
      section(id: 'benefits', attributes: {'aria-labelledby': 'b-h'}, [
      h2(id: 'b-h', [
      .text('Why Fuzzball'),
      ]),
      ul(classes: 'grid', [
      li([
      h3([
      .text('One way to do each thing'),
      ]),
      p([
      .text('No layered flag bits and no alternative spellings. One keyboard mode, one encoding of each graphics concept.'),
      ]),
      ]),
      li([
      h3([
      .text('Survives multiplexers'),
      ]),
      p([
      .text('Every message is a DCS string that travels through tmux and GNU screen passthrough unchanged. Nothing depends on files or shared memory.'),
      ]),
      ]),
      li([
      h3([
      .text('Detectable'),
      ]),
      p([
      .text('One capability query and a bounded wait tell an app whether Fuzzball is there, which version, and which subsystems.'),
      ]),
      ]),
      li([
      h3([
      .text('Bounded'),
      ]),
      p([
      .text('Terminals publish their limits in the capability reply, so applications can rely on them instead of guessing.'),
      ]),
      ]),
      li([
      h3([
      .text('Structured key events'),
      ]),
      p([
      .text('Press, repeat and release with physical key, logical key, modifiers, locks, text and IME, in a single opt-in mode.'),
      ]),
      ]),
      li([
      h3([
      .text('Graphics on the text grid'),
      ]),
      p([
      .text('Chunked uploads, cell-anchored placements, delete and query, with defined erase and reflow rules.'),
      ]),
      ]),
      li([
      h3([
      .text('Silent shell actions'),
      ]),
      p([
      .text('A shell integration can change directory without a visible '),
      code([
      .text('cd'),
      ]),
      .text(', keeping the line being edited. Works over ssh, tmux and screen.'),
      ]),
      ]),
      li([
      h3([
      .text('User-approved plugins'),
      ]),
      p([
      .text('Programs in the terminal ask to become plugins; the terminal asks the user with its own UI. Remote hosts are untrusted by default.'),
      ]),
      ]),
      li([
      h3([
      .text('Small, stable crate'),
      ]),
      p([
      .text('No dependencies, no I/O. 1.x keeps the wire format and API compatible, and golden-byte tests lock every message it writes.'),
      ]),
      ]),
      ]),
      ]),
      section(id: 'get', classes: 'light', attributes: {'aria-labelledby': 'g-h'}, [
      h2(id: 'g-h', [
      .text('Get the crate'),
      ]),
      div(classes: 'cols', [
      div([
      h3([
      .text('Cargo'),
      ]),
      pre([
      code([
      .text('cargo add fuzzball'),
      ]),
      ]),
      p([
      .text('or in '),
      code([
      .text('Cargo.toml'),
      ]),
      .text(':'),
      ]),
      pre([
      code([
      .text('''[dependencies]
      fuzzball = "1"'''),
      ]),
      ]),
      p(classes: 'lede', [
      .text('Version 1.0.0. Within 1.x there are no breaking changes to the wire format or the API.'),
      ]),
      ]),
      div([
      h3([
      .text('Links'),
      ]),
      ul(classes: 'list', [
      li([
      a(href: 'https://crates.io/crates/fuzzball', [
      .text('crates.io/crates/fuzzball'),
      ]),
      ]),
      li([
      a(href: 'https://docs.rs/fuzzball', [
      .text('docs.rs/fuzzball'),
      ]),
      .text(' — API docs'),
      ]),
      li([
      a(href: 'https://github.com/WeirdBrains/fuzzball', [
      .text('github.com/WeirdBrains/fuzzball'),
      ]),
      .text(' — source'),
      ]),
      li([
      a(href: 'https://weirdbrains.github.io/fuzzball/examples', [
      .text('Examples'),
      ]),
      ]),
      ]),
      h3([
      .text('Decode key events'),
      ]),
      pre([
      code([
      .text('''use fuzzball::{Decoder, Item};
      use fuzzball::keyboard::KeyEvent;

      let mut d = Decoder::new();
      d.push(input);
      while let Some(item) = d.next() {
      if let Item::Message(m) = item {
      if let Some(ev) = KeyEvent::from_message(&m) {
      println!("{:?} {:?}", ev.key, ev.text);
      }
      }
      }'''),
      ]),
      ]),
      ]),
      ]),
      ]),
      section(id: 'spec', classes: 'spec', attributes: {'aria-labelledby': 's-h'}, [
      h2(id: 's-h', [
      .text('The spec'),
      ]),
      p(classes: 'lede', [
      .text('Every message shares one frame: a DCS string with the private marker '),
      code([
      .text('='),
      ]),
      .text(' and the family tag '),
      code([
      .text('Fz'),
      ]),
      .text(', then a one-letter subsystem and verb.'),
      ]),
      pre([
      code([
      .text('ESC P = F z <subsystem> <verb> ; <params> [: <payload>] ESC \\'),
      ]),
      ]),
      ul(classes: 'grid', [
      li([
      h3([
      a(href: 'https://weirdbrains.github.io/fuzzball/spec', [
      .text('Common ('),
      code([
      .text('c'),
      ]),
      .text(')'),
      ]),
      ]),
      p([
      .text('Framing, capability negotiation, versioning, error replies, security rules, conformance and migration.'),
      ]),
      ]),
      li([
      h3([
      a(href: 'https://weirdbrains.github.io/fuzzball/spec/keyboard', [
      .text('Keyboard ('),
      code([
      .text('k'),
      ]),
      .text(')'),
      ]),
      ]),
      p([
      .text('One mode of structured key events, enabled per screen. While it is on, every press, repeat and release is one message.'),
      ]),
      ]),
      li([
      h3([
      a(href: 'https://weirdbrains.github.io/fuzzball/spec/graphics', [
      .text('Graphics ('),
      code([
      .text('g'),
      ]),
      .text(')'),
      ]),
      ]),
      p([
      .text('Upload, place, delete and query images. Placements are text-grid objects; limits are advertised.'),
      ]),
      ]),
      li([
      h3([
      a(href: 'https://weirdbrains.github.io/fuzzball/spec/shell', [
      .text('Shell actions ('),
      code([
      .text('s'),
      ]),
      .text(')'),
      ]),
      ]),
      p([
      .text('Silent actions such as change directory, sent on the shell\'s input and answered with a reply. Integrations for zsh, bash and fish.'),
      ]),
      ]),
      li([
      h3([
      a(href: 'https://weirdbrains.github.io/fuzzball/spec/plugins', [
      .text('Plugins ('),
      code([
      .text('p'),
      ]),
      .text(')'),
      ]),
      ]),
      p([
      .text('Hello, query and bye. The terminal asks the user before granting; grants are per tab.'),
      ]),
      ]),
      li([
      h3([
      a(href: 'https://weirdbrains.github.io/fuzzball/spec/wire-format', [
      .text('Wire format'),
      ]),
      ]),
      p([
      .text('Exactly what implementations emit today, where it is more specific than the drafts.'),
      ]),
      ]),
      ]),
      p([
      .text('The spec source lives in the crate repo: '),
      a(href: 'https://github.com/WeirdBrains/fuzzball/tree/main/docs', [
      .text('github.com/WeirdBrains/fuzzball/docs'),
      ]),
      .text('.'),
      ]),
      ]),
      ]),
      footer(classes: 'light', [
      p([
      a(href: 'https://github.com/WeirdBrains/fuzzball', [
      .text('Fuzzball on GitHub'),
      ]),
      .text(' · '),
      a(href: 'https://weirdbrains.github.io/fuzzball/', [
      .text('Fuzzball site'),
      ]),
      .text(' · '),
      a(href: '../', [
      .text('Mouser'),
      ]),
      .text(' · '),
      a(href: 'https://github.com/WeirdBrains/fuzzball/blob/main/LICENSE', [
      .text('BSD-3-Clause'),
      ]),
      .text(' · © 2026 Weird Brains'),
      ]),
      ]),
      script(src: '../main.js'),
    ]);
  }
}
