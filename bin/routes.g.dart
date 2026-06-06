// GENERATED CODE - DO NOT MODIFY BY HAND

part of 'routes.dart';

// **************************************************************************
// ShelfRouterGenerator
// **************************************************************************

Router _$ServicesRouter(Services service) {
  final router = Router();
  router.add('GET', r'/tailblog', service.getBlog);
  router.add('GET', r'/coshub', service.getCoshub);
  return router;
}
