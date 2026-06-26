// ignore_for_file: deprecated_member_use

// ignore: avoid_web_libraries_in_flutter
import 'dart:html' as html;

bool isAdminRouteRequestOnPlatform() {
  final location = html.window.location;
  final path = (location.pathname ?? '').toLowerCase();
  final hash = location.hash.toLowerCase();
  final search = location.search?.toLowerCase() ?? '';
  final href = (location.href).toLowerCase();

  return path == '/admin' ||
      path.endsWith('/admin') ||
      hash == '#/admin' ||
      hash.startsWith('#/admin?') ||
      hash == '#admin' ||
      search.contains('admin=1') ||
      href.contains('/admin') ||
      href.contains('admin=1');
}
