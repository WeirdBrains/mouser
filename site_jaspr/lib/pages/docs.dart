import 'package:jaspr/dom.dart';
import 'package:jaspr/server.dart';

import '../docs/catalog.dart';

/// Shared chrome for generated docs pages (matches scripts/build_docs.py).
class DocsChrome extends StatelessComponent {
  const DocsChrome({
    super.key,
    required this.title,
    required this.depth,
    required this.child,
    this.active,
  });

  final String title;
  final int depth;
  final String? active;
  final Component child;

  String get up => '../' * depth;

  @override
  Component build(BuildContext context) {
    return Component.fragment([
      Document.head(
        title: '$title — Mouser docs',
        children: [
          link(rel: 'icon', href: '${up}assets/icon.png'),
          link(rel: 'stylesheet', href: '${up}style.css'),
        ],
      ),
      a(classes: 'skip', href: '#main', [
        .text('Skip to content'),
      ]),
      header(classes: 'nav light', [
        a(classes: 'brand', href: up, [
          img(classes: 'logo', src: '${up}assets/logo.svg', alt: '', attributes: {'width': '36', 'height': '36'}),
          .text(' mouser'),
        ]),
        button(classes: 'menu', attributes: {'aria-expanded': 'false', 'aria-controls': 'links'}, [
          .text('Menu'),
        ]),
        nav(id: 'links', attributes: {'aria-label': 'Main'}, [
          for (final e in sectionInfo.entries)
            a(
              href: '${up}docs/${e.key}/',
              attributes: e.key == active ? {'aria-current': 'page'} : null,
              [
                .text(e.value.$1),
              ],
            ),
          a(href: 'https://github.com/WeirdBrains/mouser', [
            .text('GitHub'),
          ]),
        ]),
      ]),
      main_(id: 'main', [
        child,
      ]),
      footer(classes: 'docfoot', [
        .text('Generated from '),
        a(href: 'https://github.com/WeirdBrains/mouser/tree/main/docs', [
          .text('docs/'),
        ]),
        .text(' on main. Do not edit by hand.'),
      ]),
      script(src: '${up}main.js'),
    ]);
  }
}

class DocsIndexPage extends StatelessComponent {
  const DocsIndexPage({super.key, required this.catalog});

  final DocsCatalog catalog;

  @override
  Component build(BuildContext context) {
    return DocsChrome(
      title: 'Docs',
      depth: 1,
      child: section(classes: 'light doc', [
        h1([
          .text('Mouser docs'),
        ]),
        ul(classes: 'grid', [
          for (final e in sectionInfo.entries)
            li([
              h3([
                a(href: '${e.key}/', [
                  .text(e.value.$1),
                ]),
              ]),
              p([
                .text(e.value.$2),
              ]),
              p(classes: 'count', [
                .text('${catalog.bySection[e.key]?.length ?? 0} pages'),
              ]),
            ]),
        ]),
      ]),
    );
  }
}

class DocsSectionPage extends StatelessComponent {
  const DocsSectionPage({super.key, required this.sectionId, required this.catalog});

  final String sectionId;
  final DocsCatalog catalog;

  @override
  Component build(BuildContext context) {
    final info = infoFor(sectionId);
    final items = List<DocEntry>.from(catalog.bySection[sectionId] ?? const [])
      ..sort((left, right) {
        final leftStatus = left.status ?? '';
        final rightStatus = right.status ?? '';
        final order = leftStatus.compareTo(rightStatus);
        if (order != 0) return order;
        return left.title.toLowerCase().compareTo(right.title.toLowerCase());
      });
    return DocsChrome(
      title: info.$1,
      depth: 2,
      active: sectionInfo.containsKey(sectionId) ? sectionId : null,
      child: section(classes: 'light doc', [
        h1([
          .text(info.$1),
        ]),
        p(classes: 'lede', [
          .text(info.$2),
        ]),
        if (items.isEmpty)
          p([
            .text('Coming soon.'),
          ])
        else
          ul(classes: 'doclist', [
            for (final item in items)
              li([
                a(href: '${item.slug}.html', [
                  .text(item.title),
                ]),
                if (item.status != null && badges.containsKey(item.status))
                  Component.fragment([
                    .text(' '),
                    span(classes: 'badge', [
                      .text(badges[item.status]!),
                    ]),
                  ]),
              ]),
          ]),
        if (sectionId == 'developer')
          p([
            .text('See also the '),
            a(href: '../archive/', [
              .text('archive'),
            ]),
            .text(' of finished plans.'),
          ]),
      ]),
    );
  }
}

class DocsArticlePage extends StatelessComponent {
  const DocsArticlePage({super.key, required this.entry});

  final DocEntry entry;

  @override
  Component build(BuildContext context) {
    return DocsChrome(
      title: entry.title,
      depth: 2,
      active: sectionInfo.containsKey(entry.section) ? entry.section : null,
      child: section(classes: 'light doc', [
        RawText(entry.bodyHtml),
      ]),
    );
  }
}
