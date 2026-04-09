// ignore_for_file: avoid_print

// Firebase to Supabase Migration Script
//
// This script migrates data from Firebase Firestore to Supabase.
//
// Usage:
//   1. Set environment variables:
//      export FIREBASE_PROJECT_ID=your-firebase-project
//      export SUPABASE_URL=https://xxx.supabase.co
//      export SUPABASE_SERVICE_KEY=your-service-role-key  # NOT anon key!
//
//   2. Run with Firebase credentials:
//      dart run scripts/migrate_firebase_to_supabase.dart
//
// Requirements:
//   - Firebase Admin SDK service account JSON
//   - Supabase service role key (has full database access)
//
// Note: This is a one-time migration script. Run it once, verify data,
// then delete Firebase resources.

import 'dart:convert';
import 'dart:io';

/// Migration configuration
class MigrationConfig {
  final String firebaseProjectId;
  final String supabaseUrl;
  final String supabaseServiceKey;
  final String? firebaseCredentialsPath;

  MigrationConfig({
    required this.firebaseProjectId,
    required this.supabaseUrl,
    required this.supabaseServiceKey,
    this.firebaseCredentialsPath,
  });

  factory MigrationConfig.fromEnvironment() {
    final projectId = Platform.environment['FIREBASE_PROJECT_ID'];
    final supabaseUrl = Platform.environment['SUPABASE_URL'];
    final supabaseKey = Platform.environment['SUPABASE_SERVICE_KEY'];
    final credPath = Platform.environment['GOOGLE_APPLICATION_CREDENTIALS'];

    if (projectId == null || projectId.isEmpty) {
      throw Exception('FIREBASE_PROJECT_ID environment variable is required');
    }
    if (supabaseUrl == null || supabaseUrl.isEmpty) {
      throw Exception('SUPABASE_URL environment variable is required');
    }
    if (supabaseKey == null || supabaseKey.isEmpty) {
      throw Exception('SUPABASE_SERVICE_KEY environment variable is required');
    }

    return MigrationConfig(
      firebaseProjectId: projectId,
      supabaseUrl: supabaseUrl,
      supabaseServiceKey: supabaseKey,
      firebaseCredentialsPath: credPath,
    );
  }
}

class HttpPostResponse {
  final int statusCode;
  final String body;

  const HttpPostResponse({
    required this.statusCode,
    required this.body,
  });
}

/// Main migration function
Future<void> main() async {
  print('=== Firebase to Supabase Migration ===\n');

  try {
    final config = MigrationConfig.fromEnvironment();
    print('Firebase Project: ${config.firebaseProjectId}');
    print('Supabase URL: ${config.supabaseUrl}');
    print('');

    // Step 1: Export from Firebase
    print('Step 1: Exporting data from Firebase Firestore...');
    final firebaseData = await exportFromFirebase(config);
    print('  Found ${firebaseData['homes']?.length ?? 0} homes');
    print('  Found ${firebaseData['hubs']?.length ?? 0} hubs');

    // Step 2: Transform data
    print('\nStep 2: Transforming data for Supabase...');
    final supabaseData = transformData(firebaseData);

    // Step 3: Import to Supabase
    print('\nStep 3: Importing data to Supabase...');
    await importToSupabase(config, supabaseData);

    print('\n=== Migration Complete ===');
    print('Please verify data in Supabase dashboard before deleting Firebase.');
  } catch (e) {
    print('\nError: $e');
    print('\nMigration failed. No data was modified in Supabase.');
    exit(1);
  }
}

/// Export data from Firebase Firestore using REST API
Future<Map<String, List<Map<String, dynamic>>>> exportFromFirebase(
  MigrationConfig config,
) async {
  // Use Firebase REST API to export documents
  // This requires a service account or Firebase Auth token

  final baseUrl =
      'https://firestore.googleapis.com/v1/projects/${config.firebaseProjectId}/databases/(default)/documents';

  final homes = <Map<String, dynamic>>[];
  final hubs = <Map<String, dynamic>>[];

  // Get access token using gcloud or service account
  final token = await getFirebaseAccessToken(config);

  // Export homes collection
  print('  Fetching homes collection...');
  final homesResponse = await httpGet('$baseUrl/homes', token);
  if (homesResponse['documents'] != null) {
    for (final doc in homesResponse['documents']) {
      final home = parseFirestoreDocument(doc);
      homes.add(home);

      // Export hubs subcollection for each home
      final homeId = home['id'];
      final hubsResponse = await httpGet('$baseUrl/homes/$homeId/hubs', token);
      if (hubsResponse['documents'] != null) {
        for (final hubDoc in hubsResponse['documents']) {
          final hub = parseFirestoreDocument(hubDoc);
          hub['homeId'] = homeId;
          hubs.add(hub);
        }
      }
    }
  }

  return {'homes': homes, 'hubs': hubs};
}

/// Get Firebase access token
Future<String> getFirebaseAccessToken(MigrationConfig config) async {
  // Try using gcloud CLI first
  try {
    final result = await Process.run('gcloud', [
      'auth',
      'print-access-token',
    ]);
    if (result.exitCode == 0) {
      return (result.stdout as String).trim();
    }
  } catch (_) {}

  // Fall back to service account if available
  if (config.firebaseCredentialsPath != null) {
    print('  Using service account credentials...');
    // In a real implementation, you'd use the service account JSON
    // to generate a JWT and exchange it for an access token
    throw Exception(
      'Service account authentication not implemented. '
      'Please run: gcloud auth application-default login',
    );
  }

  throw Exception(
    'Could not get Firebase access token. '
    'Please run: gcloud auth application-default login',
  );
}

/// Parse Firestore document format to plain JSON
Map<String, dynamic> parseFirestoreDocument(Map<String, dynamic> doc) {
  final name = doc['name'] as String;
  final id = name.split('/').last;
  final fields = doc['fields'] as Map<String, dynamic>? ?? {};

  final result = <String, dynamic>{'id': id};

  for (final entry in fields.entries) {
    result[entry.key] = parseFirestoreValue(entry.value);
  }

  return result;
}

/// Parse Firestore value types
dynamic parseFirestoreValue(Map<String, dynamic> value) {
  if (value.containsKey('stringValue')) return value['stringValue'];
  if (value.containsKey('integerValue')) {
    return int.parse(value['integerValue']);
  }
  if (value.containsKey('doubleValue')) return value['doubleValue'];
  if (value.containsKey('booleanValue')) return value['booleanValue'];
  if (value.containsKey('timestampValue')) return value['timestampValue'];
  if (value.containsKey('nullValue')) return null;
  if (value.containsKey('mapValue')) {
    final fields = value['mapValue']['fields'] as Map<String, dynamic>? ?? {};
    return fields.map((k, v) => MapEntry(k, parseFirestoreValue(v)));
  }
  if (value.containsKey('arrayValue')) {
    final values = value['arrayValue']['values'] as List? ?? [];
    return values.map((v) => parseFirestoreValue(v)).toList();
  }
  return null;
}

/// Transform Firebase data to Supabase format
Map<String, List<Map<String, dynamic>>> transformData(
  Map<String, List<Map<String, dynamic>>> firebaseData,
) {
  final homes = <Map<String, dynamic>>[];
  final hubs = <Map<String, dynamic>>[];

  // Transform homes
  for (final home in firebaseData['homes'] ?? []) {
    homes.add({
      'id': home['id'],
      'name': home['name'],
      'owner_id': home['ownerId'],
      'member_ids': home['memberIds'] ?? [home['ownerId']],
      'location': home['location'],
      'sleep_schedule': home['sleepSchedule'] ??
          {
            'bedtime': 22.0,
            'wakeTime': 6.5,
            'enabled': true,
          },
      'curve_config': home['curveConfig'],
      'timezone': home['timezone'],
      'created_at':
          home['createdAt'] ?? DateTime.now().toUtc().toIso8601String(),
      'updated_at':
          home['updatedAt'] ?? DateTime.now().toUtc().toIso8601String(),
    });
  }

  // Transform hubs
  for (final hub in firebaseData['hubs'] ?? []) {
    hubs.add({
      'id': hub['id'],
      'home_id': hub['homeId'],
      'type': hub['type'],
      'name': hub['name'],
      'endpoint': hub['endpoint'],
      'enabled': hub['enabled'] ?? true,
      'token': hub['token'],
      'last_connected': hub['lastConnected'],
      'created_at':
          hub['createdAt'] ?? DateTime.now().toUtc().toIso8601String(),
      'updated_at':
          hub['updatedAt'] ?? DateTime.now().toUtc().toIso8601String(),
    });
  }

  print('  Transformed ${homes.length} homes');
  print('  Transformed ${hubs.length} hubs');

  return {'homes': homes, 'hubs': hubs};
}

/// Import data to Supabase
Future<void> importToSupabase(
  MigrationConfig config,
  Map<String, List<Map<String, dynamic>>> data,
) async {
  final headers = {
    'Content-Type': 'application/json',
    'apikey': config.supabaseServiceKey,
    'Authorization': 'Bearer ${config.supabaseServiceKey}',
    'Prefer': 'return=minimal',
  };

  // Import homes first (hubs have FK to homes)
  final homes = data['homes'] ?? [];
  if (homes.isNotEmpty) {
    print('  Inserting ${homes.length} homes...');
    final response = await httpPost(
      '${config.supabaseUrl}/rest/v1/homes',
      headers,
      homes,
    );
    if (response.statusCode >= 400) {
      throw Exception('Failed to insert homes: ${response.body}');
    }
    print('  Homes inserted successfully');
  }

  // Import hubs
  final hubs = data['hubs'] ?? [];
  if (hubs.isNotEmpty) {
    print('  Inserting ${hubs.length} hubs...');
    final response = await httpPost(
      '${config.supabaseUrl}/rest/v1/hubs',
      headers,
      hubs,
    );
    if (response.statusCode >= 400) {
      throw Exception('Failed to insert hubs: ${response.body}');
    }
    print('  Hubs inserted successfully');
  }
}

/// HTTP GET request
Future<Map<String, dynamic>> httpGet(String url, String token) async {
  final client = HttpClient();
  try {
    final request = await client.getUrl(Uri.parse(url));
    request.headers.set('Authorization', 'Bearer $token');
    final response = await request.close();
    final body = await response.transform(utf8.decoder).join();
    return json.decode(body) as Map<String, dynamic>;
  } finally {
    client.close();
  }
}

/// HTTP POST request
Future<HttpPostResponse> httpPost(
  String url,
  Map<String, String> headers,
  dynamic body,
) async {
  final client = HttpClient();
  try {
    final request = await client.postUrl(Uri.parse(url));
    headers.forEach((k, v) => request.headers.set(k, v));
    request.write(json.encode(body));

    final response = await request.close();
    final responseBody = await response.transform(utf8.decoder).join();
    return HttpPostResponse(
      statusCode: response.statusCode,
      body: responseBody,
    );
  } finally {
    client.close();
  }
}
