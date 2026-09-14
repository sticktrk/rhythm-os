import 'dart:io';

import 'package:rhythm_admin_api/src/local_admin.dart';
import 'package:shelf/shelf_io.dart' as shelf_io;

/// A separate composition root: never loads staff profiles or Supabase clients.
Future<void> main() async {
  final supervisorToken = Platform.environment['SUPERVISOR_TOKEN'];
  if (supervisorToken == null || supervisorToken.trim().isEmpty) {
    throw StateError('Home Assistant Supervisor credentials are required.');
  }
  final tokenFile = Platform.environment['RHYTHM_ADDON_API_TOKEN_FILE'] ??
      '/run/rhythm/api-token';
  final localToken = (await File(tokenFile).readAsString()).trim();
  if (!RegExp(r'^[a-zA-Z0-9]{32,}$').hasMatch(localToken)) {
    throw StateError('Invalid local runtime credential.');
  }
  final users = HomeAssistantUsers(supervisorToken);
  final proxy = LocalDeviceProxy(token: localToken);
  final server = LocalAdminServer(proxy: proxy, lookupUser: users.lookup);
  final httpServer =
      await shelf_io.serve(server.handler, InternetAddress.loopbackIPv4, 8787);
  print('Rhythm local admin API ready.');
  Future<void> stop(ProcessSignal _) async {
    await httpServer.close(force: true);
    proxy.client.close();
    exit(0);
  }

  ProcessSignal.sigterm.watch().listen(stop);
  ProcessSignal.sigint.watch().listen(stop);
}
