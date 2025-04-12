import 'package:flutter/foundation.dart';

import '../models/models.dart';

// Manages the timeline posts state
class TimelineState extends ChangeNotifier {
  List<Post> _posts = [];
  bool _isLoading = true; // Initially loading
  String? _error;

  List<Post> get posts => _posts;
  bool get isLoading => _isLoading;
  String? get error => _error;

  // This method will be passed to WebSocketService
  void handleServerMessage(ServerMessage message) {
    print("TimelineState handling: ${message.runtimeType}");
    _error = null; // Clear previous errors on new message
    bool changed = false;
    if (message is InitialStateMessage) {
      _posts = List<Post>.from(message.posts); // Create a mutable copy
       _posts.sort((a, b) => b.timestamp.compareTo(a.timestamp)); // Sort newest first
      _isLoading = false;
      print("TimelineState: Received initial state with ${_posts.length} posts.");
      changed = true;
    } else if (message is NewPostMessage) {
       // Avoid duplicates if message somehow arrives multiple times
       if (!_posts.any((p) => p.id == message.post.id)) {
          _posts.insert(0, message.post); // Add new post to the beginning
           print("TimelineState: Added new post ${message.post.id}.");
           changed = true;
       }
    } else if (message is MarketUpdateMessage) {
        final index = _posts.indexWhere((p) => p.id == message.postId);
        if (index != -1) {
            // Create a new Post object with updated values
            final originalPost = _posts[index];
            // Check if price or supply actually changed to avoid unnecessary updates
            if (originalPost.price != message.price || originalPost.supply != message.supply) {
                _posts[index] = Post(
                    id: originalPost.id,
                    userId: originalPost.userId,
                    content: originalPost.content,
                    timestamp: originalPost.timestamp,
                    price: message.price, // Update price
                    supply: message.supply, // Update supply
                );
                print("TimelineState: Updated post ${message.postId} - Price: ${message.price}, Supply: ${message.supply}");
                changed = true;
            }
        } else {
             print("TimelineState: Received MarketUpdate for unknown post ${message.postId}");
             // Optionally set an error or log more verbosely
        }
    } else if (message is ErrorMessage) {
       print("TimelineState: Received server error: ${message.message}");
       if (_error != message.message) { // Only update if error message is different
          _error = message.message;
          // Optionally set loading to false if an error occurs during loading
          if (_isLoading) _isLoading = false;
          changed = true;
       }
    } else if (message is UnknownMessage) {
        print("TimelineState: Received unknown message type: ${message.type}");
        final errorMsg = "Received unknown message type: ${message.type}";
         if (_error != errorMsg) { // Only update if error message is different
             _error = errorMsg;
             changed = true;
         }
    }

    if (changed) {
        notifyListeners(); // Update UI
    }
  }

   void setLoading(bool loading) {
      if (_isLoading != loading) {
         _isLoading = loading;
         if (loading) _error = null; // Clear error when starting to load
         notifyListeners();
      }
   }

   void setError(String? errorMsg) {
      if (_error != errorMsg) {
         _error = errorMsg;
         if (errorMsg != null) _isLoading = false; // Stop loading if error occurs
         notifyListeners();
      }
   }
} 