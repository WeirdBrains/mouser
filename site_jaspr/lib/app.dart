import 'package:jaspr/jaspr.dart';
import 'package:jaspr_router/jaspr_router.dart';

import 'docs/catalog.dart';
import 'pages/docs.dart';
import 'pages/fuzzball.dart';
import 'pages/home.dart';
import 'pages/screenshots.dart';

class App extends StatelessComponent {
  const App({super.key});

  @override
  Component build(BuildContext context) {
    final docs = DocsCatalog.load();
    return Router(routes: [
      Route(
        path: '/',
        title: 'Mouser — a fast GPU terminal emulator',
        builder: (context, state) => const HomePage(),
      ),
      Route(
        path: '/fuzzball',
        title: 'Fuzzball — the terminal protocol crate from Mouser',
        builder: (context, state) => const FuzzballPage(),
      ),
      Route(
        path: '/screenshots',
        title: 'Screenshots — Mouser',
        builder: (context, state) => const ScreenshotsPage(),
      ),
      Route(
        path: '/docs',
        title: 'Docs — Mouser docs',
        builder: (context, state) => DocsIndexPage(catalog: docs),
      ),
      for (final sec in [...sectionInfo.keys, 'archive'])
        Route(
          path: '/docs/$sec',
          title: '${infoFor(sec).$1} — Mouser docs',
          builder: (context, state) => DocsSectionPage(sectionId: sec, catalog: docs),
        ),
      for (final page in docs.entries)
        Route(
          path: page.route,
          title: '${page.title} — Mouser docs',
          builder: (context, state) => DocsArticlePage(entry: page),
        ),
    ]);
  }
}
