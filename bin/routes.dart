import 'dart:convert';

import 'package:shelf/shelf.dart';
import 'package:shelf_router/shelf_router.dart';
import 'package:tail_app_shared/tail_app_shared.dart';
import 'routes/blog.dart';
import 'routes/coshub.dart';
part 'routes.g.dart';

class Services {
  Services();
  JsonEncoder jsonEncoder = JsonEncoder();
  @Route.get('/tailblog')
  Future<Response> getBlog(Request request) async {
    BlogResponse blogResponse = await getTailBlogPosts();
    return Response.ok(jsonEncoder.convert(blogResponse.toJson()),headers: {'Content-Type': 'Application/Json'});
  }
  @Route.get('/coshub')
  Future<Response> getCoshub(Request request) async {
    CoshubResponse coshubResponse = await getCoshubResponse();
    return Response.ok(jsonEncoder.convert(coshubResponse.toJson()),headers: {'Content-Type': 'Application/Json'});
  }
  // Create router using the generate function defined in 'userservice.g.dart'.
  Router get router => _$ServicesRouter(this);
}