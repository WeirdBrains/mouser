import 'package:jaspr/dom.dart';
import 'package:jaspr/server.dart';

/// The Weird Brains home page. Every page links to it from its footer.
const weirdBrainsUrl = 'https://weirdbrains.com';

/// A link to [weirdBrainsUrl] with [label] as its text.
Component weirdBrainsLink([String label = 'Weird Brains']) => a(href: weirdBrainsUrl, [
      .text(label),
    ]);

/// The footer credit every page carries: "A Weird Brains project".
Component weirdBrainsCredit() => p(classes: 'wb-credit', [
      .text('A '),
      weirdBrainsLink(),
      .text(' project'),
    ]);
