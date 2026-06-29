import 'dart:io';

import 'package:rhythm_admin_api/src/config.dart';
import 'package:rhythm_admin_api/src/device_probe_service.dart';
import 'package:rhythm_admin_api/src/env_loader.dart';
import 'package:rhythm_admin_api/src/server.dart';
import 'package:rhythm_admin_api/src/support_access_service.dart';
import 'package:rhythm_admin_api/src/supabase_rest_client.dart';
import 'package:rhythm_admin_api/src/support_service.dart';
import 'package:shelf/shelf_io.dart' as shelf_io;

Future<void> main() async {
  final environment = await loadAdminApiEnvironment();
  final config = AdminApiConfig.fromEnvironment(environment);
  final supabase = SupabaseRestClient(config: config);
  final support = SupportService(supabase: supabase);
  final supportAccess = SupportAccessService(
    config: config,
    supabase: supabase,
  );
  final probes = DeviceProbeService(
    supabase: supabase,
    supportAccess: supportAccess,
  );
  final server = AdminApiServer(
    config: config,
    supabase: supabase,
    support: support,
    probes: probes,
  );

  final httpServer = await shelf_io.serve(
    server.handler,
    InternetAddress(config.host),
    config.port,
  );

  print(
    'Rhythm admin API listening on '
    'http://${httpServer.address.host}:${httpServer.port}',
  );
}
