import 'dart:convert';
import 'dart:io';

import 'package:markdown/markdown.dart' as md;


/// Section labels and blurbs, matching [scripts/build_docs.py].
const sectionInfo = <String, (String name, String blurb)>{
  'user': (
    'User Guides',
    'Guides to using Mouser: its commands, modes, chat, themes and preferences.',
  ),
  'developer': (
    'Developer Guides',
    'Building plugins (such as a mode), rendering engines (such as a custom GPU engine) and Fuzzball integrations.',
  ),
  'mcp': (
    'MCP Guides',
    "Mouser's MCP server and client, and how to integrate with the server.",
  ),
  'api': (
    'API Guides',
    'The published mouser crates, for developers who want to use them.',
  ),
};

const archiveInfo = ('Archive', 'Finished plans, kept for history.');

const badges = {
  'draft': 'Draft',
  'placeholder': 'Placeholder',
  'coming-soon': 'Coming soon',
};

class DocEntry {
  const DocEntry({
    required this.section,
    required this.slug,
    required this.title,
    required this.bodyHtml,
    this.status,
    this.sourceRel,
  });

  final String section;
  final String slug;
  final String title;
  final String bodyHtml;
  final String? status;
  final String? sourceRel;

  String get route => '/docs/$section/$slug.html';
}

class DocsCatalog {
  DocsCatalog({
    required this.entries,
    required this.bySection,
  });

  final List<DocEntry> entries;
  final Map<String, List<DocEntry>> bySection;

  static DocsCatalog? _cached;

  static DocsCatalog load() {
    return _cached ??= _load();
  }

  static Directory _docsDir() {
    final env = Platform.environment['MOUSER_DOCS_DIR'];
    if (env != null && env.isNotEmpty) return Directory(env);
    return Directory('../docs');
  }

  static Directory _extraDir() {
    final env = Platform.environment['MOUSER_SITE_SRC'];
    if (env != null && env.isNotEmpty) return Directory(env);
    return Directory('../site-src');
  }

  static DocsCatalog _load() {
    final docs = _docsDir();
    final extra = _extraDir();
    if (!docs.existsSync()) {
      throw StateError('docs directory not found at ${docs.path} (set MOUSER_DOCS_DIR)');
    }
    final sectionsFile = File('${docs.path}/sections.toml');
    if (!sectionsFile.existsSync()) {
      throw StateError('missing ${sectionsFile.path}');
    }
    final mapping = parseSectionsToml(sectionsFile.readAsStringSync());

    final mdFiles = docs
        .listSync(recursive: true)
        .whereType<File>()
        .where((f) => f.path.endsWith('.md'))
        .map((f) => f.path.substring(docs.path.length + 1).replaceAll(r'\', '/'))
        .where((rel) => !rel.split('/').contains('historical'))
        .toList()
      ..sort();

    final unmapped = [for (final rel in mdFiles) if (!mapping.containsKey(rel)) rel];
    if (unmapped.isNotEmpty) {
      throw StateError('unmapped docs: $unmapped');
    }
    final unknown = mapping.values.toSet().difference({'index', 'internal', ...sectionInfo.keys});
    if (unknown.isNotEmpty) {
      throw StateError('unknown sections in sections.toml: ${unknown.toList()..sort()}');
    }

    final where = <String, (String section, String slug)>{};
    for (final e in mapping.entries) {
      if (e.value == 'index' || e.value == 'internal') continue;
      final slug = e.key.replaceFirst(RegExp(r'\.md$'), '').replaceAll('/README', '').replaceAll('/', '-').toLowerCase();
      where[e.key] = (e.value, slug);
    }
    final historical = Directory('${docs.path}/historical');
    if (historical.existsSync()) {
      for (final f in historical.listSync().whereType<File>().where((f) => f.path.endsWith('.md'))) {
        final name = f.uri.pathSegments.last;
        where['historical/$name'] = ('archive', name.replaceFirst(RegExp(r'\.md$'), '').toLowerCase());
      }
    }

    final lists = <String, List<DocEntry>>{
      for (final k in [...sectionInfo.keys, 'archive']) k: <DocEntry>[],
    };

    for (final e in where.entries.toList()..sort((a, b) => a.key.compareTo(b.key))) {
      final rel = e.key;
      final section = e.value.$1;
      final slug = e.value.$2;
      final file = File('${docs.path}/$rel');
      if (!file.existsSync()) {
        throw StateError('mapped doc missing: $rel');
      }
      var text = file.readAsStringSync();
      final title = titleOf(text, slug);
      text = fixLinks(text, rel, where);
      final html = renderMarkdown(text);
      final src =
          '<p class="docsrc">Source: <a href="https://github.com/WeirdBrains/mouser/blob/main/docs/$rel">docs/$rel</a></p>';
      final entry = DocEntry(
        section: section,
        slug: slug,
        title: title,
        bodyHtml: html + src,
        sourceRel: rel,
      );
      lists[section]!.add(entry);
    }

    if (extra.existsSync()) {
      final extras = extra
          .listSync(recursive: true)
          .whereType<File>()
          .where((f) => f.path.endsWith('.md'))
          .toList()
        ..sort((a, b) => a.path.compareTo(b.path));
      for (final f in extras) {
        final section = Directory(f.parent.path).uri.pathSegments.where((s) => s.isNotEmpty).last;
        final parsed = splitFrontMatter(f.readAsStringSync());
        final title = parsed.meta['title'] ?? titleOf(parsed.body, f.uri.pathSegments.last.replaceFirst(RegExp(r'\.md$'), ''));
        final status = parsed.meta['status'] ?? '';
        var note = '';
        if (badges.containsKey(status)) {
          final msg = htmlEscape.convert(parsed.meta['note'] ?? '');
          note = '<p class="status $status">${badges[status]}: $msg</p>';
        }
        final slug = f.uri.pathSegments.last.replaceFirst(RegExp(r'\.md$'), '');
        final entry = DocEntry(
          section: section,
          slug: slug,
          title: title,
          bodyHtml: note + renderMarkdown(parsed.body),
          status: status.isEmpty ? null : status,
        );
        (lists[section] ??= <DocEntry>[]).add(entry);
      }
    }

    final all = [for (final list in lists.values) ...list];
    return DocsCatalog(entries: all, bySection: lists);
  }

}

(String name, String blurb) infoFor(String section) {
  if (section == 'archive') return archiveInfo;
  return sectionInfo[section]!;
}

String titleOf(String text, String fallback) {
  final m = RegExp(r'^# (.+)$', multiLine: true).firstMatch(text);
  return m != null ? m.group(1)!.trim() : fallback;
}

({Map<String, String> meta, String body}) splitFrontMatter(String text) {
  final meta = <String, String>{};
  if (text.startsWith('---\n')) {
    final end = text.indexOf('\n---\n', 4);
    if (end != -1) {
      final head = text.substring(4, end);
      text = text.substring(end + 5);
      for (final line in head.split('\n')) {
        final i = line.indexOf(':');
        if (i <= 0) continue;
        meta[line.substring(0, i).trim()] = line.substring(i + 1).trim();
      }
    }
  }
  return (meta: meta, body: text);
}

Map<String, String> parseSectionsToml(String text) {
  final out = <String, String>{};
  final re = RegExp(r'^"([^"]+)"\s*=\s*"([^"]+)"', multiLine: true);
  for (final m in re.allMatches(text)) {
    out[m.group(1)!] = m.group(2)!;
  }
  return out;
}

String renderMarkdown(String text) {
  return md.markdownToHtml(
    text,
    extensionSet: md.ExtensionSet.gitHubWeb,
  );
}

String posixDirname(String rel) {
  final i = rel.lastIndexOf('/');
  return i < 0 ? '' : rel.substring(0, i);
}

String posixJoin(String base, String path) {
  if (path.startsWith('/')) return path;
  if (base.isEmpty) return path;
  return '$base/$path';
}

String posixNormalize(String path) {
  final parts = <String>[];
  for (final p in path.split('/')) {
    if (p.isEmpty || p == '.') continue;
    if (p == '..') {
      if (parts.isNotEmpty && parts.last != '..') {
        parts.removeLast();
      } else {
        parts.add('..');
      }
    } else {
      parts.add(p);
    }
  }
  return parts.join('/');
}

/// Rewrite in-repo markdown links the same way [scripts/build_docs.py] does.
String fixLinks(String text, String rel, Map<String, (String section, String slug)> where) {
  final base = posixDirname(rel);
  return text.replaceAllMapped(RegExp(r'(\]\s*)\(([^)\s]+)\)'), (m) {
    final label = m.group(1)!;
    final target = m.group(2)!;
    if (RegExp(r'^(https?:|mailto:|#)').hasMatch(target)) {
      return m.group(0)!;
    }
    final hash = target.indexOf('#');
    final path = hash >= 0 ? target.substring(0, hash) : target;
    final anchor = hash >= 0 ? target.substring(hash) : '';
    var norm = path.isNotEmpty ? posixNormalize(posixJoin(base, path)) : rel;
    if (path.endsWith('/') || norm == 'docs' || norm == '.') {
      if (norm == '.') {
        norm = 'README.md';
      } else {
        norm = '$norm/README.md'.replaceFirst(RegExp(r'^\./'), '');
      }
    }
    final mapped = where[norm];
    if (mapped != null) {
      return '$label(../${mapped.$1}/${mapped.$2}.html$anchor)';
    }
    if (norm == 'README.md') {
      return '$label(../$anchor)';
    }
    final gh = norm.startsWith('../') ? norm.substring(3) : 'docs/$norm';
    return '$label(https://github.com/WeirdBrains/mouser/blob/main/$gh$anchor)';
  });
}

final htmlEscape = HtmlEscape();
