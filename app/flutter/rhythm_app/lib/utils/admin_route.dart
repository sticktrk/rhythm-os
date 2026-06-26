import 'admin_route_stub.dart' if (dart.library.html) 'admin_route_web.dart'
    as platform;

bool isAdminRouteRequest(Uri baseUri, String routeName) {
  return _isAdminUri(baseUri) ||
      _isAdminRouteName(routeName) ||
      platform.isAdminRouteRequestOnPlatform();
}

bool _isAdminUri(Uri uri) {
  final path = uri.path.toLowerCase();
  final fragment = uri.fragment.toLowerCase();
  return path == '/admin' ||
      path.endsWith('/admin') ||
      fragment == '/admin' ||
      fragment == 'admin' ||
      uri.queryParameters['admin'] == '1';
}

bool _isAdminRouteName(String routeName) {
  final normalized = routeName.trim().toLowerCase();
  return normalized == '/admin' ||
      normalized == 'admin' ||
      normalized.startsWith('/admin?') ||
      normalized.startsWith('admin?') ||
      normalized.contains('admin=1');
}
