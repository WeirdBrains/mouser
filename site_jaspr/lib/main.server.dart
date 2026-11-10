/// The entrypoint for the **server** environment.
///
/// The [main] method will only be executed on the server during pre-rendering.
library;

import 'package:jaspr/server.dart';

import 'app.dart';
import 'main.server.options.dart';

void main() {
  Jaspr.initializeApp(
    options: defaultServerOptions,
  );

  runApp(Document(
    title: 'Mouser — a fast GPU terminal emulator',
    lang: 'en',
    // Keep relative asset and nav links resolving like the hand-written site.
    base: null,
    viewport: 'width=device-width, initial-scale=1',
    meta: const {},
    styles: const [],
    head: const [],
    body: const App(),
  ));
}
