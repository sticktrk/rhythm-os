import 'package:dio/dio.dart';
import 'package:logging/logging.dart';

/// Dio interceptor that routes HTTP activity through [package:logging].
class RhythmLogInterceptor extends Interceptor {
  final Logger _log;

  RhythmLogInterceptor(this._log);

  @override
  void onRequest(RequestOptions options, RequestInterceptorHandler handler) {
    _log.fine('${options.method} ${options.uri}');
    handler.next(options);
  }

  @override
  void onResponse(Response response, ResponseInterceptorHandler handler) {
    _log.fine(
        '${response.statusCode} ${response.requestOptions.method} '
        '${response.requestOptions.uri}');
    handler.next(response);
  }

  @override
  void onError(DioException err, ErrorInterceptorHandler handler) {
    final status = err.response?.statusCode;
    _log.warning(
      '${err.type.name} ${err.requestOptions.method} '
      '${err.requestOptions.uri}'
      '${status != null ? ' [$status]' : ''}',
      err,
    );
    handler.next(err);
  }
}
