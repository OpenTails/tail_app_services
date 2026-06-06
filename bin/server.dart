import 'package:logging/logging.dart';
import 'package:shelf/shelf.dart';
import 'package:shelf/shelf_io.dart' as shelf_io;

import 'routes.dart';

Logger _requestLogger = Logger("Shelf");
void main(List<String> args) async {
  Logger.root.level = Level.ALL;
  var service = Services();
  var router = service.router;
  var handler = const Pipeline()
      .addMiddleware(
        logRequests(
          logger: (message, isError) {
            if (isError) {
              _requestLogger.severe(message);
            } else {
              _requestLogger.info(message);
            }
          },
        ),
      )
      .addHandler(router.call);
  var server = await shelf_io.serve(handler, '0.0.0.0', 8080);
  server.autoCompress = true;
}
