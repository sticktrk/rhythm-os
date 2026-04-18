import 'package:dio/dio.dart';
import 'package:mocktail/mocktail.dart';

class MockDio extends Mock implements Dio {
  final _interceptors = Interceptors();

  @override
  BaseOptions get options => BaseOptions(baseUrl: 'http://test/');

  @override
  Interceptors get interceptors => _interceptors;
}
