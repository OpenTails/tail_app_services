import 'package:dio/dio.dart';
import 'package:dio_cache_interceptor/dio_cache_interceptor.dart';
import 'package:http_cache_hive_store/http_cache_hive_store.dart';

final cacheOptions = CacheOptions(
  // A default store is required for interceptor.
  store: HiveCacheStore(null, hiveBoxName: "dioCache"),
  hitCacheOnErrorCodes: const [500],

  hitCacheOnNetworkFailure: true,
  maxStale: const Duration(days: 7),
);

Dio? _dio;
Future<Dio> initDio() async {
  if (_dio != null) {
    return _dio!;
  }

  final Dio dio = Dio();
  // Global options

  //dio.interceptors.add(DioCacheInterceptor(options: cacheOptions));
  _dio = dio;
  return dio;
}