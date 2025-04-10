import 'dart:convert'; // Needed for jsonEncode
import 'package:flutter/material.dart';
import 'package:supabase_flutter/supabase_flutter.dart';
import 'package:provider/provider.dart';
import 'package:web_socket_channel/web_socket_channel.dart';
import 'package:intl/intl.dart'; // For date formatting

// --- Constants ---
const String WEBSOCKET_URL = 'ws://localhost:8080/ws'; // Placeholder for server URL

// --- Data Models (Matching Server) ---

@immutable // Make models immutable
class Post {
  final String id; // Use String for UUIDs in Dart
  final String userId;
  final String content;
  final DateTime timestamp;
  final double price; // Server calculates and includes this
  final int supply;

  const Post({
    required this.id,
    required this.userId,
    required this.content,
    required this.timestamp,
    required this.price,
    required this.supply,
  });

  // Factory constructor for JSON deserialization
  factory Post.fromJson(Map<String, dynamic> json) {
    return Post(
      id: json['id'] as String,
      userId: json['user_id'] as String,
      content: json['content'] as String,
      // Server sends ISO 8601 string
      timestamp: DateTime.parse(json['timestamp'] as String),
      // Server ensures price is sent
      price: (json['price'] as num).toDouble(),
      supply: json['supply'] as int,
    );
  }
}

// Represents messages received from the server
@immutable
abstract class ServerMessage {
  const ServerMessage();

  factory ServerMessage.fromJson(Map<String, dynamic> json) {
    final type = json['type'] as String;
    switch (type) {
      case 'initial_state':
        final postsList = (json['posts'] as List)
            .map((postJson) => Post.fromJson(postJson as Map<String, dynamic>))
            .toList();
        return InitialStateMessage(posts: postsList);
      case 'new_post':
        final post = Post.fromJson(json['post'] as Map<String, dynamic>);
        return NewPostMessage(post: post);
      case 'error':
        return ErrorMessage(message: json['message'] as String);
      case 'market_update':
        return MarketUpdateMessage(
          postId: json['post_id'] as String,
          price: (json['price'] as num).toDouble(),
          supply: json['supply'] as int
        );
      // Add other cases for MarketUpdate etc. later
      default:
        print("Received unknown server message type: $type");
        // Return a specific unknown type or throw an error
         return UnknownMessage(type: type, data: json);
    }
  }
}

class InitialStateMessage extends ServerMessage {
  final List<Post> posts;
  const InitialStateMessage({required this.posts});
}

class NewPostMessage extends ServerMessage {
  final Post post;
  const NewPostMessage({required this.post});
}

class ErrorMessage extends ServerMessage {
  final String message;
  const ErrorMessage({required this.message});
}

class MarketUpdateMessage extends ServerMessage {
  final String postId;
  final double price;
  final int supply;
  const MarketUpdateMessage({
    required this.postId,
    required this.price,
    required this.supply
  });
}

class UnknownMessage extends ServerMessage {
   final String type;
   final Map<String, dynamic> data;
   const UnknownMessage({required this.type, required this.data});
}

// --- Services ---

// WebSocket Service
enum WebSocketStatus { disconnected, connecting, connected, error }

class WebSocketService extends ChangeNotifier {
  WebSocketChannel? _channel;
  WebSocketStatus _status = WebSocketStatus.disconnected;
  String? _connectionError;
  final Function(ServerMessage) _onMessageReceived; // Callback

  WebSocketStatus get status => _status;
  String? get connectionError => _connectionError;

  // Constructor now requires a callback function
  WebSocketService({required Function(ServerMessage) onMessageReceived})
      : _onMessageReceived = onMessageReceived;

  Future<void> connect(String accessToken) async {
    if (_status == WebSocketStatus.connected || _status == WebSocketStatus.connecting) {
      print("WebSocket already connecting or connected.");
      return;
    }
    print("Attempting WS connect...");
    _status = WebSocketStatus.connecting;
    _connectionError = null;
    notifyListeners();

    final uri = Uri.parse("$WEBSOCKET_URL?token=$accessToken");

    try {
      _channel = WebSocketChannel.connect(uri);
      _status = WebSocketStatus.connected;
      print("WebSocket connected.");
      notifyListeners(); // Notify about successful connection

      _channel!.stream.listen(
        (message) {
          print('Received raw from WS: $message');
          try {
            final decodedJson = jsonDecode(message as String) as Map<String, dynamic>;
            final serverMessage = ServerMessage.fromJson(decodedJson);
             print('Decoded ServerMessage: ${serverMessage.runtimeType}');
            // Call the callback function provided by TimelineState
            _onMessageReceived(serverMessage);
          } catch (e, stackTrace) {
            print("Error decoding/handling server message: $e\n$stackTrace");
            // Optionally notify TimelineState about the error
             _onMessageReceived(ErrorMessage(message: "Failed to parse server message: $e"));
          }
        },
        onDone: () {
          print("WebSocket closed by server.");
          _handleDisconnect(notify: true, error: 'Connection closed by server');
        },
        onError: (error) {
          print("WebSocket stream error: $error");
          _handleDisconnect(notify: true, error: error.toString());
        },
        cancelOnError: true,
      );

    } catch (e) {
      print("WebSocket connect error: $e");
      _handleDisconnect(notify: true, error: e.toString());
    }
     // No need for final notifyListeners here, handled in _handleDisconnect or above
  }

  void sendMessage(Map<String, dynamic> message) {
    if (_status != WebSocketStatus.connected || _channel == null) {
      print("WS not connected. Cannot send.");
      return;
    }
    final encodedMessage = jsonEncode(message);
    print("Sending to WS: $encodedMessage");
    _channel!.sink.add(encodedMessage);
  }

  void disconnect() {
    print("Manual WS disconnect.");
    _handleDisconnect(notify: true, error: null); // No error on manual disconnect
  }

  void _handleDisconnect({required bool notify, String? error}) {
    if (_status == WebSocketStatus.disconnected) return; // Already disconnected

    if (_channel != null) {
      _channel!.sink.close().catchError((e) {
        // Log sink closing errors, but don't stop the disconnect process
        print("Error closing WebSocket sink: $e");
      });
      _channel = null;
    }

    if (error != null) {
        _status = WebSocketStatus.error;
        _connectionError = error;
        print("WebSocket disconnected with error: $error");
    } else {
        _status = WebSocketStatus.disconnected;
        _connectionError = null;
         print("WebSocket disconnected cleanly.");
    }

    if (notify) notifyListeners();
  }

  @override
  void dispose() {
    print("Disposing WebSocketService");
    disconnect();
    super.dispose();
  }
}

// --- State Management ---

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
    if (message is InitialStateMessage) {
      _posts = List<Post>.from(message.posts); // Create a mutable copy
       _posts.sort((a, b) => b.timestamp.compareTo(a.timestamp)); // Sort newest first
      _isLoading = false;
      print("TimelineState: Received initial state with ${_posts.length} posts.");
    } else if (message is NewPostMessage) {
       // Avoid duplicates if message somehow arrives multiple times
       if (!_posts.any((p) => p.id == message.post.id)) {
          _posts.insert(0, message.post); // Add new post to the beginning
           print("TimelineState: Added new post ${message.post.id}.");
       }
    } else if (message is MarketUpdateMessage) {
        final index = _posts.indexWhere((p) => p.id == message.postId);
        if (index != -1) {
            // Create a new Post object with updated values
            final originalPost = _posts[index];
            _posts[index] = Post(
                id: originalPost.id,
                userId: originalPost.userId,
                content: originalPost.content,
                timestamp: originalPost.timestamp,
                price: message.price, // Update price
                supply: message.supply, // Update supply
            );
            print("TimelineState: Updated post ${message.postId} - Price: ${message.price}, Supply: ${message.supply}");
        } else {
             print("TimelineState: Received MarketUpdate for unknown post ${message.postId}");
        }
    } else if (message is ErrorMessage) {
       print("TimelineState: Received server error: ${message.message}");
      _error = message.message; // Store the error message
    } else if (message is UnknownMessage) {
        print("TimelineState: Received unknown message type: ${message.type}");
        _error = "Received unknown message type: ${message.type}";
    }
    // Handle MarketUpdate later

    notifyListeners(); // Update UI
  }

   void setLoading(bool loading) {
      if (_isLoading != loading) {
         _isLoading = loading;
         notifyListeners();
      }
   }

   void setError(String? errorMsg) {
      if (_error != errorMsg) {
         _error = errorMsg;
         notifyListeners();
      }
   }
}

// Authentication service/state
class AuthState extends ChangeNotifier {
  User? _user;
  User? get user => _user;
  String? _accessToken; // Store the access token
  String? get accessToken => _accessToken;

  final SupabaseClient _supabase = Supabase.instance.client;

  AuthState() {
    _supabase.auth.onAuthStateChange.listen((data) {
      _user = data.session?.user;
      _accessToken = data.session?.accessToken; // Store access token on auth change
      print("Auth State Changed: User: ${_user?.id}, Token: ${_accessToken != null}");
      notifyListeners();
    });
  }

  Future<String?> signInWithEmail(String email, String password) async {
    try {
      final AuthResponse res = await _supabase.auth.signInWithPassword(
        email: email,
        password: password,
      );
      _user = res.user;
      _accessToken = res.session?.accessToken;
      notifyListeners();
      return null; // Sign in successful
    } on AuthException catch (e) {
      print("Sign In Error: ${e.message}");
      return e.message; // Return error message
    } catch (e) {
      print("Sign In Error: $e");
      return 'An unexpected error occurred.';
    }
  }

  Future<String?> signUpWithEmail(String email, String password) async {
     try {
      // Note: Supabase email auth typically requires email confirmation.
      // For simplicity here, we'll assume it's enabled or auto-confirmed.
      final AuthResponse res = await _supabase.auth.signUp(
        email: email,
        password: password,
      );
       // Sign up might not immediately create a session depending on email verification settings
      _user = res.user;
      _accessToken = res.session?.accessToken;
      notifyListeners();
      return null; // Sign up successful (or verification email sent)
    } on AuthException catch (e) {
      print("Sign Up Error: ${e.message}");
      return e.message; // Return error message
    } catch (e) {
      print("Sign Up Error: $e");
      return 'An unexpected error occurred.';
    }
  }

  Future<void> signOut() async {
    await _supabase.auth.signOut();
    _user = null;
    _accessToken = null;
    notifyListeners();
  }
}

Future<void> main() async {
  WidgetsFlutterBinding.ensureInitialized();

  // IMPORTANT: Replace with your Supabase URL and Anon Key
  await Supabase.initialize(
    url: 'https://ayjbspnnvjqhhbioeapo.supabase.co',
    anonKey: 'eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJpc3MiOiJzdXBhYmFzZSIsInJlZiI6ImF5amJzcG5udmpxaGhiaW9lYXBvIiwicm9sZSI6ImFub24iLCJpYXQiOjE3NDQxMjY5NzQsImV4cCI6MjA1OTcwMjk3NH0.KkBaeBQrjfruaLRAXiLu9xvloCgAfjQe5FmEcf98djQ',
  );

  runApp(
    MultiProvider(
      providers: [
        ChangeNotifierProvider(create: (_) => AuthState()),
        ChangeNotifierProvider(create: (_) => TimelineState()),
        ChangeNotifierProxyProvider<TimelineState, WebSocketService>(
           create: (context) {
               // Initial creation - get the handler from TimelineState
               final timelineState = context.read<TimelineState>();
               print("ProxyProvider creating initial WebSocketService");
               return WebSocketService(onMessageReceived: timelineState.handleServerMessage);
           },
           update: (context, timelineState, previousWebSocketService) {
              // IMPORTANT: Reuse the previous instance if it exists!
              // Only update the callback if necessary (though in this setup, the handler itself doesn't change)
              print("ProxyProvider updating WebSocketService... Reusing previous: ${previousWebSocketService != null}");
              // In this specific case, the handler function reference passed to the constructor
              // IS the same instance throughout the lifecycle of TimelineState,
              // so we don't strictly need to update anything here.
              // We just return the existing service to prevent disposal.
              return previousWebSocketService ?? WebSocketService(onMessageReceived: timelineState.handleServerMessage);
            },
         ),
      ],
      child: const MyApp(),
    ),
  );
}

class MyApp extends StatelessWidget {
  const MyApp({super.key});

  @override
  Widget build(BuildContext context) {
    return MaterialApp(
      title: 'FLVKE',
      theme: ThemeData(
        colorScheme: ColorScheme.fromSeed(seedColor: Colors.deepPurple),
        useMaterial3: true,
        textTheme: const TextTheme( // Example: Customize text theme
           bodyMedium: TextStyle(fontSize: 16.0),
           titleMedium: TextStyle(fontSize: 18.0, fontWeight: FontWeight.bold),
        ),
      ),
      home: Consumer<AuthState>(
        builder: (context, authState, _) {
          if (authState.user == null) {
            return const LoginPage(); // Show login if not authenticated
          } else {
            return const TimelinePage(); // Show timeline if authenticated
          }
        },
      ),
    );
  }
}

// --- Placeholder Screens ---

class LoginPage extends StatefulWidget {
  const LoginPage({super.key});

  @override
  State<LoginPage> createState() => _LoginPageState();
}

class _LoginPageState extends State<LoginPage> {
  final _emailController = TextEditingController();
  final _passwordController = TextEditingController();
  bool _isLoading = false;

  @override
  void dispose() {
    _emailController.dispose();
    _passwordController.dispose();
    super.dispose();
  }

  Future<void> _handleSignIn() async {
    setState(() => _isLoading = true);
    final authState = Provider.of<AuthState>(context, listen: false);
    final error = await authState.signInWithEmail(
      _emailController.text.trim(),
      _passwordController.text.trim(),
    );
    if (mounted) {
       setState(() => _isLoading = false);
      if (error != null) {
        ScaffoldMessenger.of(context).showSnackBar(
          SnackBar(content: Text('Sign In Failed: $error'), backgroundColor: Colors.red),
        );
      }
      // No need for else, listener in main will handle navigation
    }
  }

   Future<void> _handleSignUp() async {
    setState(() => _isLoading = true);
    final authState = Provider.of<AuthState>(context, listen: false);
     final error = await authState.signUpWithEmail(
      _emailController.text.trim(),
      _passwordController.text.trim(),
    );

     if (mounted) {
       setState(() => _isLoading = false);
      if (error != null) {
         ScaffoldMessenger.of(context).showSnackBar(
          SnackBar(content: Text('Sign Up Failed: $error'), backgroundColor: Colors.red),
        );
      } else {
         // Optionally show a success message or prompt to check email
         ScaffoldMessenger.of(context).showSnackBar(
           const SnackBar(content: Text('Sign Up successful! Please check your email if verification is required.'), backgroundColor: Colors.green),
         );
      }
    }
  }

  @override
  Widget build(BuildContext context) {
    return Scaffold(
      appBar: AppBar(title: const Text('Login / Sign Up')),
      body: Padding(
        padding: const EdgeInsets.all(16.0),
        child: Column(
          mainAxisAlignment: MainAxisAlignment.center,
          children: [
            TextField(
              controller: _emailController,
              decoration: const InputDecoration(labelText: 'Email'),
              keyboardType: TextInputType.emailAddress,
              enabled: !_isLoading,
            ),
            const SizedBox(height: 8),
            TextField(
              controller: _passwordController,
              decoration: const InputDecoration(labelText: 'Password'),
              obscureText: true,
               enabled: !_isLoading,
            ),
            const SizedBox(height: 20),
            if (_isLoading)
              const CircularProgressIndicator()
            else ...[
              ElevatedButton(
                onPressed: _handleSignIn,
                child: const Text('Login'),
              ),
              const SizedBox(height: 8),
              TextButton(
                onPressed: _handleSignUp,
                child: const Text('Sign Up'),
              ),
             ]
          ],
        ),
      ),
    );
  }
}

class TimelinePage extends StatefulWidget {
  const TimelinePage({super.key});

  @override
  State<TimelinePage> createState() => _TimelinePageState();
}

class _TimelinePageState extends State<TimelinePage> {
  late final WebSocketService _webSocketService;
  late final AuthState _authState;
  late final TimelineState _timelineState;

  final _postContentController = TextEditingController();

  @override
  void initState() {
    super.initState();
    _authState = Provider.of<AuthState>(context, listen: false);
    _webSocketService = Provider.of<WebSocketService>(context, listen: false);
    _timelineState = Provider.of<TimelineState>(context, listen: false);

    final token = _authState.accessToken;
    if (token != null) {
      print("TimelinePage: Initializing, attempting WS connect.");
      _timelineState.setLoading(true); // Show loading initially
      // Connect WebSocket
      WidgetsBinding.instance.addPostFrameCallback((_) {
        if (mounted) {
           _webSocketService.connect(token).catchError((e) {
               print("TimelinePage: Error during initial connect: $e");
                if (mounted) {
                 _timelineState.setError("Failed to connect: $e");
                 _timelineState.setLoading(false);
                }
           });
        }
      });
    } else {
      print("TimelinePage: No access token found on init!");
      _timelineState.setError("Authentication token not found.");
      _timelineState.setLoading(false);
    }

    // Listen to WebSocket status for UI feedback
    _webSocketService.addListener(_onWebSocketStatusChanged);
  }

  void _onWebSocketStatusChanged() {
    if (mounted) {
      final status = _webSocketService.status;
      final error = _webSocketService.connectionError;
      print("TimelinePage: WS Status Changed: $status, Error: $error");

      // Update TimelineState based on WebSocket status
      if (status == WebSocketStatus.error && error != null) {
         _timelineState.setError("WebSocket Error: $error");
          _timelineState.setLoading(false); // Stop loading on error
      } else if (status == WebSocketStatus.disconnected && error == null) {
          // Handle clean disconnects if needed
      } else if (status == WebSocketStatus.connecting) {
           _timelineState.setLoading(true);
           _timelineState.setError(null);
      } else if (status == WebSocketStatus.connected) {
           _timelineState.setError(null);
           // Loading state turned off by InitialState message handling
      }

      // Rebuild UI to show status in AppBar via setState
      setState(() {});
    }
  }

  @override
  void dispose() {
    print("Disposing TimelinePage");
    _webSocketService.removeListener(_onWebSocketStatusChanged);
    _postContentController.dispose();
    super.dispose();
  }

  void _showCreatePostDialog() {
     if (_webSocketService.status != WebSocketStatus.connected) {
         ScaffoldMessenger.of(context).showSnackBar(
            const SnackBar(content: Text('Not connected to server.'), backgroundColor: Colors.orange),
         );
         return;
     }
    showDialog(
      context: context,
      builder: (context) {
        return AlertDialog(
          title: const Text('Create New Post'),
          content: TextField(
            controller: _postContentController,
            decoration: const InputDecoration(hintText: "What's happening?"),
            autofocus: true,
            maxLines: 3,
          ),
          actions: [
            TextButton(
              onPressed: () => Navigator.pop(context),
              child: const Text('Cancel'),
            ),
            TextButton(
              onPressed: () {
                final content = _postContentController.text.trim();
                if (content.isNotEmpty) {
                  final message = {
                    'type': 'create_post',
                    'content': content,
                  };
                  _webSocketService.sendMessage(message);
                  _postContentController.clear();
                  Navigator.pop(context);
                } else {
                   ScaffoldMessenger.of(context).showSnackBar(
                     const SnackBar(content: Text('Post content cannot be empty.'), backgroundColor: Colors.red),
                   );
                }
              },
              child: const Text('Post'),
            ),
          ],
        );
      },
    );
  }

  @override
  Widget build(BuildContext context) {
    final authState = Provider.of<AuthState>(context, listen: false);
    // Use a Consumer for wsStatus if AppBar title needs to react,
    // or rely on the setState in _onWebSocketStatusChanged as currently implemented.
    final wsStatus = _webSocketService.status;

    return Scaffold(
      appBar: AppBar(
        title: Text('Timeline (WS: ${wsStatus.name})'), // Shows connection status
        actions: [
          IconButton(
            icon: const Icon(Icons.logout),
            onPressed: () async {
               await authState.signOut();
            },
          ),
        ],
      ),
      body: Column( // Main body structure
        children: [
          // Error display area
          Consumer<TimelineState>(
            builder: (context, timelineState, _) {
              if (timelineState.error != null) {
                return Container(
                    color: Colors.red[100],
                    padding: const EdgeInsets.all(8.0),
                    child: Row(
                      children: [
                         Icon(Icons.error_outline, color: Colors.red[700]),
                         const SizedBox(width: 8),
                         Expanded(child: Text('Error: ${timelineState.error!}')),
                      ],
                    ),
                );
              } else {
                 return const SizedBox.shrink(); // No error, show nothing
              }
            }
          ),
          // Loading indicator or Post list area
          Expanded(
            child: Consumer<TimelineState>(
               builder: (context, timelineState, _) {
                  if (timelineState.isLoading) {
                     return const Center(child: CircularProgressIndicator());
                  } else if (timelineState.posts.isEmpty) {
                     return const Center(child: Text('No posts yet. Create one!'));
                  } else {
                     // Display the list of posts
                     return ListView.builder(
                        itemCount: timelineState.posts.length,
                        itemBuilder: (context, index) {
                           final post = timelineState.posts[index];
                           return PostWidget(post: post);
                        },
                     );
                  }
               }
            ),
          ),
        ],
      ),
      // Floating action button to create posts
      floatingActionButton: FloatingActionButton(
        onPressed: _showCreatePostDialog,
        tooltip: 'New Post',
        backgroundColor: wsStatus == WebSocketStatus.connected
            ? Theme.of(context).colorScheme.primary
            : Colors.grey, // Disable visually if not connected
        child: const Icon(Icons.add),
      ),
    );
  }
}

// --- Custom Widgets ---

// Widget to display a single post
class PostWidget extends StatelessWidget {
  final Post post;

  const PostWidget({required this.post, super.key});

  @override
  Widget build(BuildContext context) {
     // Formatting for display
     final formattedDate = DateFormat.yMd().add_jms().format(post.timestamp.toLocal());
     final formattedPrice = NumberFormat.currency(symbol: '\$', decimalDigits: 2).format(post.price);

    return Card(
      margin: const EdgeInsets.symmetric(vertical: 8.0, horizontal: 12.0),
      elevation: 2,
      child: Padding(
        padding: const EdgeInsets.all(12.0),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            // Post Content
            Text(post.content, style: Theme.of(context).textTheme.bodyMedium),
            const SizedBox(height: 8),
            // Author and Timestamp
            Text(
              'By: ${post.userId} \nAt: $formattedDate', // Consider fetching/displaying usernames later
              style: Theme.of(context).textTheme.bodySmall?.copyWith(color: Colors.grey[600]),
            ),
            const Divider(height: 16, thickness: 1),
            // Market Info
            Row(
               mainAxisAlignment: MainAxisAlignment.spaceBetween,
               children: [
                  Text('Price: $formattedPrice', style: Theme.of(context).textTheme.titleMedium),
                  Text('Supply: ${post.supply}', style: Theme.of(context).textTheme.bodyMedium),
               ]
            ),
             const SizedBox(height: 8),
            // Action Buttons
             Row(
                 mainAxisAlignment: MainAxisAlignment.end,
                 children: [
                     ElevatedButton(
                        onPressed: () {
                          // Access WebSocketService via context
                          final wsService = Provider.of<WebSocketService>(context, listen: false);
                          if (wsService.status == WebSocketStatus.connected) {
                              final buyMessage = {
                                  'type': 'buy',
                                  'post_id': post.id
                              };
                              wsService.sendMessage(buyMessage);
                          } else {
                               ScaffoldMessenger.of(context).showSnackBar(
                                const SnackBar(content: Text('Not connected'), backgroundColor: Colors.orange),
                            );
                          }
                        },
                        child: const Text('Buy')
                     ),
                     const SizedBox(width: 8),
                     ElevatedButton(
                         onPressed: () {
                            final wsService = Provider.of<WebSocketService>(context, listen: false);
                            if (wsService.status == WebSocketStatus.connected) {
                                final sellMessage = {
                                    'type': 'sell',
                                    'post_id': post.id
                                };
                                wsService.sendMessage(sellMessage);
                             } else {
                                ScaffoldMessenger.of(context).showSnackBar(
                                const SnackBar(content: Text('Not connected'), backgroundColor: Colors.orange),
                            );
                          }
                        },
                        child: const Text('Sell')
                     ),
                 ],
             )
          ],
        ),
      ),
    );
  }
}
