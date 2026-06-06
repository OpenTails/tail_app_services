import 'dart:async';

import 'package:dio/dio.dart';
import 'package:logging/logging.dart';
import 'package:tail_app_shared/tail_app_shared.dart';
import 'package:wordpress_client/wordpress_client.dart';
import '../requestHelpers.dart';
// Used as MediaDetails isn't exported
// ignore: implementation_imports
import 'package:wordpress_client/src/responses/properties/media_details.dart';

Logger _logger = Logger("TailBlog");

WordpressClient? client;


Timer? requestCooldown;
bool shouldRefresh = true;
BlogResponse? cachedResponse;

Future<BlogResponse> getTailBlogPosts() async {
  if (cachedResponse != null) {
    updateTailBlogPosts();
    return cachedResponse!;
  } else {
    return await updateTailBlogPosts();
  }
}
Future<BlogResponse> updateTailBlogPosts() async {
  if (!shouldRefresh && cachedResponse != null) {
    return cachedResponse!;
  }
  List<Post> wordpressPosts = await getBlogPosts();
  List<BlogPost> mappedPosts = await getPosts(wordpressPosts);
  if (mappedPosts.isEmpty && cachedResponse != null) {
    return cachedResponse!;
  }
  cachedResponse = BlogResponse(timestamp: DateTime.now(), posts: mappedPosts);
  requestCooldown = Timer(Duration(minutes: 10), () {
    shouldRefresh = true;
    requestCooldown?.cancel();
    requestCooldown = null;
  },);
  return cachedResponse!;
}
Future<List<Post>> getBlogPosts() async {
  // Slug, Sticky, and Author are not used
  final ListPostRequest request = ListPostRequest(
    page: 1,
    perPage: 20,
    order: Order.desc,
    queryParameters: {'_embed': 'true', '_fields': '_links.wp:featuredmedia,id,link,title,date,featured_media'},
    context: RequestContext.embed,
  );
  client ??= await getWordpressClient();
  final WordpressResponse<List<Post>> wordpressPostResponse = await client!.posts.list(request);
  List<Post>? data = wordpressPostResponse.dataOrNull();
  return data ?? [];
}

Future<List<BlogPost>> getPosts(List<Post> wordpressPosts) async {
  List<BlogPost> results = [];
  if (wordpressPosts.isNotEmpty) {
    for (Post post in wordpressPosts) {
      results.add(BlogPost(title: post.title!.parsedText, publishDate: post.date!, url: post.link, imageId: post.featuredMedia, imageUrl: await getImageURL(post)));
    }
  }
  //filter out posts that are missing a thumbnail
  results = results.where((element) => element.imageUrl != null).toList();
  return results;
}

Future<String?> getImageURL(Post post) async {
  try {
    Map<String, dynamic>? json = post.self['_embedded']?['wp:featuredmedia']?[0]?['media_details'];
    if (json == null || json.isEmpty) {
      _logger.warning("No featured media for post ${post.title}");
      return null;
    }
    MediaDetails? mediaDetails = MediaDetails.fromJson(json);

    // if the post does not have a featured image, or if the featured image does not have the compressed versions created by wordpress.
    if (mediaDetails.sizes == null) {
      return null;
    }
    if (mediaDetails.sizes!.containsKey('medium')) {
      // matches the app blog image size
      return mediaDetails.sizes!['medium']!.sourceUrl;
    } else if (mediaDetails.sizes!.containsKey('thumbnail')) {
      // smaller fallback
      return mediaDetails.sizes!['thumbnail']!.sourceUrl;
    } else if (mediaDetails.sizes!.containsKey('full')) {
      // when all else fails
      return mediaDetails.sizes!['full']!.sourceUrl;
    } else if (mediaDetails.sizes!.isEmpty && mediaDetails.file != null) {
      _logger.warning("Loading full size image for post: ${post.title}.");
      return "https://thetailcompany.com/wp-content/uploads/${mediaDetails.file}";
    }
  } catch (e) {
    _logger.severe("Unable to load featured media for post ${post.title}. $e");
  }
  return null;
}

Future<WordpressClient> getWordpressClient() async {
  if (client != null) {
    return client!;
  }
  Dio dio = await initDio();
  return WordpressClient.fromDioInstance(baseUrl: Uri.parse('https://thetailcompany.com/wp-json/wp/v2'), instance: dio);
}
