import 'package:jaspr/dom.dart';
import 'package:jaspr/server.dart';

class ScreenshotsPage extends StatelessComponent {
  const ScreenshotsPage({super.key});

  @override
  Component build(BuildContext context) {
    return Component.fragment([
      Document.head(
        title: 'Screenshots — Mouser',
        meta: {
          'description': 'Mouser on Omarchy: commander, chat, sidebar, theme editor and tiled windows.',
        },
        children: [
          link(rel: 'icon', href: '../assets/icon.png'),
          link(rel: 'stylesheet', href: '../style.css'),
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
      a(href: './', [
      .text('Screenshots'),
      ]),
      a(href: '../#install', [
      .text('Install'),
      ]),
      a(href: '../docs/user/', [
      .text('User Guides'),
      ]),
      a(href: '../docs/developer/', [
      .text('Developer Guides'),
      ]),
      a(href: '../docs/mcp/', [
      .text('MCP Guides'),
      ]),
      a(href: '../docs/api/', [
      .text('API Guides'),
      ]),
      a(href: '../fuzzball/', [
      .text('Fuzzball'),
      ]),
      a(href: 'https://github.com/WeirdBrains/mouser', [
      .text('GitHub'),
      ]),
      ]),
      ]),
      main_(id: 'main', [
      section(id: 'screenshots', classes: 'light', [
      h1([
      .text('Screenshots'),
      ]),
      p(classes: 'lede', [
      .text('Mouser on '),
      a(href: 'https://omarchy.org/', [
      .text('Omarchy'),
      ]),
      .text('. Click any shot for the full image.'),
      ]),
      div(classes: 'shot-grid', [
      figure(classes: 'shot-card', [
      div(classes: 'media-frame', [
      a(href: '../assets/screenshots/mouser-commander-omarchy.png', [
      img(src: '../assets/screenshots/mouser-commander-omarchy.png', alt: 'Commander docked above the prompt on Omarchy: list view with files and the shell below.', loading: MediaLoading.lazy),
      ]),
      ]),
      figcaption([
      strong([
      .text('Commander'),
      ]),
      .text(' — Commander docked above the prompt on Omarchy: list view with files and the shell below.'),
      ]),
      ]),
      figure(classes: 'shot-card', [
      div(classes: 'media-frame', [
      a(href: '../assets/screenshots/mouser-commander-omarchy-fullscreen.png', [
      img(src: '../assets/screenshots/mouser-commander-omarchy-fullscreen.png', alt: 'Commander filling the window on an ultrawide Omarchy desktop.', loading: MediaLoading.lazy),
      ]),
      ]),
      figcaption([
      strong([
      .text('Commander fullscreen'),
      ]),
      .text(' — Commander filling the window on an ultrawide Omarchy desktop.'),
      ]),
      ]),
      figure(classes: 'shot-card', [
      div(classes: 'media-frame', [
      a(href: '../assets/screenshots/mouser-chat-omarchy.png', [
      img(src: '../assets/screenshots/mouser-chat-omarchy.png', alt: 'Chat mode with the agent transcript and input line on Omarchy.', loading: MediaLoading.lazy),
      ]),
      ]),
      figcaption([
      strong([
      .text('Chat'),
      ]),
      .text(' — Chat mode with the agent transcript and input line on Omarchy.'),
      ]),
      ]),
      figure(classes: 'shot-card', [
      div(classes: 'media-frame', [
      a(href: '../assets/screenshots/mouser-sidebar-omarchy.png', [
      img(src: '../assets/screenshots/mouser-sidebar-omarchy.png', alt: 'SSH connections sidebar open beside the terminal on Omarchy.', loading: MediaLoading.lazy),
      ]),
      ]),
      figcaption([
      strong([
      .text('Sidebar'),
      ]),
      .text(' — SSH connections sidebar open beside the terminal on Omarchy.'),
      ]),
      ]),
      figure(classes: 'shot-card', [
      div(classes: 'media-frame', [
      a(href: '../assets/screenshots/mouser-theme-editor-omarchy.png', [
      img(src: '../assets/screenshots/mouser-theme-editor-omarchy.png', alt: 'Theme editor showing colour swatches and the current palette on Omarchy.', loading: MediaLoading.lazy),
      ]),
      ]),
      figcaption([
      strong([
      .text('Theme editor'),
      ]),
      .text(' — Theme editor showing colour swatches and the current palette on Omarchy.'),
      ]),
      ]),
      figure(classes: 'shot-card', [
      div(classes: 'media-frame', [
      a(href: '../assets/screenshots/mouser-tiled-omarchy.png', [
      img(src: '../assets/screenshots/mouser-tiled-omarchy.png', alt: 'Mouser tiled with other windows on an ultrawide Omarchy desktop.', loading: MediaLoading.lazy),
      ]),
      ]),
      figcaption([
      strong([
      .text('Tiled windows'),
      ]),
      .text(' — Mouser tiled with other windows on an ultrawide Omarchy desktop.'),
      ]),
      ]),
      ]),
      p([
      .text('Also see the '),
      a(href: '../#beautiful-h', [
      .text('Mouser is beautiful'),
      ]),
      .text(' clip on the front page.'),
      ]),
      ]),
      ]),
      footer(classes: 'docfoot', [
      a(href: '../', [
      .text('mouser.weirdbrains.com'),
      ]),
      ]),
      script(src: '../main.js'),
    ]);
  }
}
