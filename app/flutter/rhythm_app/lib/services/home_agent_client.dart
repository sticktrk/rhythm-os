import 'dart:convert';
import 'dart:io';

import 'package:crypto/crypto.dart';
import 'package:dio/dio.dart';
import 'package:flutter/foundation.dart';
import 'package:image/image.dart' as img;
import 'package:image_picker/image_picker.dart';
import 'package:supabase_flutter/supabase_flutter.dart' show FunctionException;
import '../backend/backend.dart';
import 'customer_assistant_service.dart';

/// The caller chooses the photo. The original asset and its metadata never
/// enter the conversation or the network transport.
Future<Uint8List?> pickLightingPhoto(ImageSource source) async {
  final file = await ImagePicker().pickImage(
      source: source,
      maxWidth: 1280,
      maxHeight: 1280,
      imageQuality: 85,
      requestFullMetadata: false);
  if (file == null) return null;
  try {
    if (await file.length() > 8 * 1024 * 1024) {
      throw const FormatException('Image too large');
    }
    return await compute(normalizeLightingPhoto, await file.readAsBytes());
  } finally {
    // Mobile pickers copy selected assets into the application's temporary
    // directory. Remove that copy only, preserving the library original.
    if (!kIsWeb && (Platform.isAndroid || Platform.isIOS)) {
      try {
        final cache = await Directory.systemTemp.resolveSymbolicLinks();
        final copy = await File(file.path).resolveSymbolicLinks();
        if (copy.startsWith('$cache/')) await File(copy).delete();
      } on FileSystemException {
        // The picker or OS may already have reclaimed this temporary file.
      }
    }
  }
}

Uint8List normalizeLightingPhoto(Uint8List input) {
  final decoded = img.decodeImage(input);
  if (decoded == null) throw const FormatException('Unreadable photo');
  final oriented = img.bakeOrientation(decoded);
  final scaled = oriented.width > 1280 || oriented.height > 1280
      ? img.copyResize(oriented,
          width: oriented.width >= oriented.height ? 1280 : null,
          height: oriented.height > oriented.width ? 1280 : null)
      : oriented;
  final clean = img.Image.fromBytes(
      width: scaled.width,
      height: scaled.height,
      bytes: scaled.getBytes(order: img.ChannelOrder.rgb).buffer,
      numChannels: 3);
  final bytes = img.encodeJpg(clean, quality: 80);
  if (bytes.length > 1500000) throw const FormatException('Photo too large');
  return bytes;
}

class HomeAgentClient {
  HomeAgentClient(
      this.endpoint, this.home, this.instance, this.token, this.current,
      {Dio? transport})
      : _http = transport ??
            Dio(BaseOptions(
                connectTimeout: const Duration(seconds: 12),
                receiveTimeout: const Duration(seconds: 55),
                sendTimeout: const Duration(seconds: 20),
                followRedirects: false));
  final Uri endpoint;
  final String home, instance;
  final String? Function() token;
  final bool Function() current;
  final Dio _http;
  String? session;

  static Future<HomeAgentClient?> discover(
      String home, String instance, bool Function() current) async {
    final auth = BackendProvider.instance.auth;
    if (auth is! SupabaseAuthBackend || !current()) return null;
    const override = String.fromEnvironment('RHYTHM_LIGHTING_AGENT_URL');
    String? configured = override;
    if (override.isEmpty) {
      try {
        final data = (await auth.client.functions
                .invoke('lighting-agent-access', body: {'action': 'discover'}))
            .data;
        configured = data is Map ? data['endpoint'] as String? : null;
      } on FunctionException catch (e) {
        if (e.status == 401 || e.status == 403) rethrow;
        return null;
      } catch (_) {
        return null;
      }
    }
    final uri = Uri.tryParse(configured ?? '');
    if (uri == null ||
        uri.scheme != 'https' ||
        uri.host.isEmpty ||
        uri.userInfo.isNotEmpty ||
        !current()) {
      return null;
    }
    final client = HomeAgentClient(uri, home, instance,
        () => auth.client.auth.currentSession?.accessToken, current);
    try {
      final state = await client.call('open');
      client.session = state['session'] as String;
      return client;
    } on DioException catch (e) {
      client.close();
      final body = e.response?.data;
      final legacyCohort = e.response?.statusCode == 403 &&
          body is Map &&
          body['error'] == 'legacy_chat';
      if (e.response == null ||
          (e.response?.statusCode ?? 0) >= 500 ||
          legacyCohort ||
          e.response?.statusCode == 404) {
        return null;
      }
      rethrow;
    } catch (_) {
      client.close();
      rethrow;
    }
  }

  String reference(String kind, String id) =>
      '${kind}_${sha256.convert(utf8.encode(jsonEncode([
                session,
                kind,
                id
              ]))).toString().substring(0, 24)}';
  Future<CustomerAction> call(String method,
      [CustomerAction body = const {}, Uint8List? photo]) async {
    final bearer = token();
    if (!current() || bearer == null) {
      throw StateError('Account or installation changed');
    }
    final data = photo == null
        ? body
        : FormData.fromMap({
            'turn': jsonEncode(body),
            'photo': MultipartFile.fromBytes(photo,
                filename: 'lighting.jpg',
                contentType: DioMediaType('image', 'jpeg'))
          });
    final reply = await _http.postUri(endpoint.resolve('/v1/$method'),
        data: data,
        options: Options(headers: {
          'authorization': 'Bearer $bearer',
          'x-rhythm-home': home,
          'x-rhythm-installation': instance,
          if (session != null) 'x-rhythm-session': session
        }, contentType: photo == null ? 'application/json' : null));
    if (!current() || reply.data is! Map) {
      throw StateError('Conversation changed');
    }
    return CustomerAction.from(reply.data as Map);
  }

  void close() => _http.close(force: true);
}
