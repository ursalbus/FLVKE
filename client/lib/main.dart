import 'dart:convert'; // Needed for jsonEncode
import 'package:flutter/material.dart';
import 'package:supabase_flutter/supabase_flutter.dart';
import 'package:provider/provider.dart';
import 'package:web_socket_channel/web_socket_channel.dart';
import 'package:intl/intl.dart'; // For date formatting
import 'dart:math'; // For math operations

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
  final double supply; // <-- Changed to double

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
      supply: (json['supply'] as num).toDouble(), // <-- Parse as num -> double
    );
  }
}

// Added PositionDetail class (matching server)
@immutable
class PositionDetail {
  final String postId;
  final double size; // <-- Changed to double
  final double averagePrice;
  final double unrealizedPnl;

  const PositionDetail({
    required this.postId,
    required this.size,
    required this.averagePrice,
    required this.unrealizedPnl,
  });

  factory PositionDetail.fromJson(Map<String, dynamic> json) {
    return PositionDetail(
      postId: json['post_id'] as String,
      size: (json['size'] as num).toDouble(), // <-- Parse as num -> double
      averagePrice: (json['average_price'] as num).toDouble(),
      unrealizedPnl: (json['unrealized_pnl'] as num).toDouble(),
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
      case 'user_sync':
        final positionsList = (json['positions'] as List)
            .map((posJson) => PositionDetail.fromJson(posJson as Map<String, dynamic>))
            .toList();
        return UserSyncMessage(
            balance: (json['balance'] as num).toDouble(),
            total_realized_pnl: (json['total_realized_pnl'] as num? ?? 0.0).toDouble(),
            positions: positionsList,
        );
      case 'new_post':
        final post = Post.fromJson(json['post'] as Map<String, dynamic>);
        return NewPostMessage(post: post);
      case 'market_update':
        return MarketUpdateMessage(
          postId: json['post_id'] as String,
          price: (json['price'] as num).toDouble(),
          supply: (json['supply'] as num).toDouble() // <-- Parse as num -> double
        );
      case 'balance_update':
        return BalanceUpdateMessage(balance: (json['balance'] as num).toDouble());
      case 'position_update':
        return PositionUpdateMessage(
          position: PositionDetail.fromJson(json as Map<String, dynamic>)
        );
      case 'realized_pnl_update':
        return RealizedPnlUpdateMessage(
          totalRealizedPnl: (json['total_realized_pnl'] as num).toDouble()
        );
      case 'error':
        return ErrorMessage(message: json['message'] as String);
      default:
        print("Received unknown server message type: $type");
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
  final double supply; // <-- Changed to double
  const MarketUpdateMessage({
    required this.postId,
    required this.price,
    required this.supply
  });
}

class BalanceUpdateMessage extends ServerMessage {
  final double balance;
  const BalanceUpdateMessage({required this.balance});
}

class PositionUpdateMessage extends ServerMessage {
  final PositionDetail position;
  const PositionUpdateMessage({required this.position});
}

class UserSyncMessage extends ServerMessage {
  final double balance;
  final double total_realized_pnl;
  final List<PositionDetail> positions;
  const UserSyncMessage({required this.balance, required this.total_realized_pnl, required this.positions});
}

class RealizedPnlUpdateMessage extends ServerMessage {
  final double totalRealizedPnl;
  const RealizedPnlUpdateMessage({required this.totalRealizedPnl});
}

class UnknownMessage extends ServerMessage {
   final String type;
   final Map<String, dynamic> data;
   const UnknownMessage({required this.type, required this.data});
}

// --- Bonding Curve Calculation Helpers (Dart mirror of Rust logic) ---

const double _BONDING_CURVE_EPSILON = 1e-9;
const double _EPSILON = 1e-9; // General purpose epsilon

// Integral of P(s) from 0 to s, for s > 0
// Int(1 + sqrt(x) dx) = x + (2/3)x^(3/2)
double _integralPos(double s) {
  if (s <= _BONDING_CURVE_EPSILON) { // Treat s<=0 as 0
    return 0.0;
  } else {
    return s + (2.0 / 3.0) * pow(s, 1.5);
  }
}

// Integral of P(s) from s to 0, for s < 0. Result is >= 0.
// 2*sqrt(|s|) - 2*ln(1+sqrt(|s|))
double _integralNegToZero(double s) {
  if (s >= -_BONDING_CURVE_EPSILON) { // Treat s>=0 as 0
    return 0.0;
  } else {
    final t = s.abs(); // t = |s|
    return 2.0 * sqrt(t) - 2.0 * log(1.0 + sqrt(t));
  }
}

// Calculate the cost (definite integral) of changing supply from s1 to s2
// Cost = Integral[s1, s2] P(x) dx
//      = Integral[0, s2] P(x) dx - Integral[0, s1] P(x) dx
double calculateBondingCurveCost(double s1, double s2) {
  if (s1.isNaN || s1.isInfinite || s2.isNaN || s2.isInfinite) {
    return double.nan;
  }

  final integralAtS2 = (s2 > _BONDING_CURVE_EPSILON)
      ? _integralPos(s2)
      : (s2 < -_BONDING_CURVE_EPSILON ? -_integralNegToZero(s2) : 0.0);

  final integralAtS1 = (s1 > _BONDING_CURVE_EPSILON)
      ? _integralPos(s1)
      : (s1 < -_BONDING_CURVE_EPSILON ? -_integralNegToZero(s1) : 0.0);

  return integralAtS2 - integralAtS1;
}

// --- Services ---

// WebSocket Service
enum WebSocketStatus { disconnected, connecting, connected, error }

class WebSocketService extends ChangeNotifier {
  WebSocketChannel? _channel;
  WebSocketStatus _status = WebSocketStatus.disconnected;
  String? _connectionError;
  // Use a list of handlers now
  final List<Function(ServerMessage)> _messageHandlers;

  WebSocketStatus get status => _status;
  String? get connectionError => _connectionError;

   // Constructor takes a list of handlers
  WebSocketService({required List<Function(ServerMessage)> onMessageReceivedHandlers})
      : _messageHandlers = onMessageReceivedHandlers;

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
            // Call all registered handlers
            for (final handler in _messageHandlers) {
                handler(serverMessage);
            }
          } catch (e, stackTrace) {
            print("Error decoding/handling server message: $e\n$stackTrace");
            // Optionally notify TimelineState about the error
             for (final handler in _messageHandlers) {
                handler(ErrorMessage(message: "Failed to parse server message: $e"));
             }
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

// Added BalanceState
class BalanceState extends ChangeNotifier {
   double _balance = 1000.0; // Default initial balance
   double _totalRealizedPnl = 0.0; // Added
   String? _error;

   double get balance => _balance;
   double get totalRealizedPnl => _totalRealizedPnl; // Added getter
   String? get error => _error;

   void handleServerMessage(ServerMessage message) {
     print("BalanceState handling: ${message.runtimeType}");
     bool changed = false;
     if (message is UserSyncMessage) { // Handle initial sync
       if (_balance != message.balance) {
           _balance = message.balance;
           changed = true;
       }
       if (_totalRealizedPnl != message.total_realized_pnl) {
            _totalRealizedPnl = message.total_realized_pnl;
            changed = true;
       }
        print("BalanceState Synced: Bal: ${_balance.toStringAsFixed(4)}, RPnl: ${_totalRealizedPnl.toStringAsFixed(4)}");
     } else if (message is BalanceUpdateMessage) {
        if (_balance != message.balance) {
            _balance = message.balance;
            _error = null;
             print("BalanceState updated: Bal: ${_balance.toStringAsFixed(4)}");
            changed = true;
        }
     } else if (message is RealizedPnlUpdateMessage) { // Added
         if (_totalRealizedPnl != message.totalRealizedPnl) {
            _totalRealizedPnl = message.totalRealizedPnl;
            _error = null;
            print("BalanceState updated: RPnl: ${_totalRealizedPnl.toStringAsFixed(4)}");
            changed = true;
         }
     } else if (message is ErrorMessage) {
         // Optionally handle errors related to balance if server sends specific ones
          if (_error != message.message) {
               _error = message.message;
               print("BalanceState received error: ${message.message}");
               changed = true;
          }
     }

     if (changed) {
         notifyListeners();
     }
   }
}

// Added PositionState
class PositionState extends ChangeNotifier {
  Map<String, PositionDetail> _positions = {}; // PostID -> PositionDetail
  String? _error;

  Map<String, PositionDetail> get positions => _positions;
  String? get error => _error;

  void handleServerMessage(ServerMessage message) {
    print("PositionState handling: ${message.runtimeType}");
    _error = null;
    if (message is UserSyncMessage) {
       _positions = { for (var p in message.positions) p.postId : p };
        print("PositionState: Synced ${_positions.length} positions.");
        notifyListeners();
    } else if (message is PositionUpdateMessage) {
       _positions[message.position.postId] = message.position;
        print("PositionState: Updated position for ${message.position.postId}");
        notifyListeners();
    } else if (message is ErrorMessage) {
       _error = message.message;
        print("PositionState received error: ${message.message}");
       notifyListeners();
    }
     // Add logic here if positions should be removed when size is 0?
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
        ChangeNotifierProvider(create: (_) => BalanceState()),
        ChangeNotifierProvider(create: (_) => PositionState()), // Provide PositionState
        // Update ProxyProvider to include PositionState
        ChangeNotifierProxyProvider3<TimelineState, BalanceState, PositionState, WebSocketService>(
           create: (context) {
              final timelineState = context.read<TimelineState>();
              final balanceState = context.read<BalanceState>();
              final positionState = context.read<PositionState>(); // Read PositionState
              print("ProxyProvider creating initial WebSocketService with handlers");
              return WebSocketService(onMessageReceivedHandlers: [
                  timelineState.handleServerMessage,
                  balanceState.handleServerMessage,
                  positionState.handleServerMessage, // Add handler
              ]);
           },
           update: (context, timelineState, balanceState, positionState, previousWebSocketService) {
              print("ProxyProvider updating WebSocketService... Reusing previous: ${previousWebSocketService != null}");
              // Reuse previous instance
              return previousWebSocketService ?? WebSocketService(onMessageReceivedHandlers: [
                 timelineState.handleServerMessage,
                 balanceState.handleServerMessage,
                 positionState.handleServerMessage, // Add handler
              ]);
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
    final wsStatus = _webSocketService.status;
    // Consume BalanceState to display balance
    final balanceState = Provider.of<BalanceState>(context);

    // Format currency
    final balanceFormatted = NumberFormat.currency(symbol: '\$', decimalDigits: 2).format(balanceState.balance);
    final rpnlFormatted = NumberFormat.currency(symbol: '\$', decimalDigits: 2).format(balanceState.totalRealizedPnl);
    final rpnlColor = balanceState.totalRealizedPnl >= 0 ? Colors.green[700] : Colors.red[700];

    return Scaffold(
      appBar: AppBar(
        title: Text(
          // Example: Bal: $1000.00 | RPnl: $50.00 (WS: connected)
          'Bal: $balanceFormatted | RPnl: ', 
          style: const TextStyle(fontSize: 14), 
        ),
         titleSpacing: 0, // Reduce spacing if title is long
         actions: [
             // Display RPnl colored
             Padding(
               padding: const EdgeInsets.only(right: 4.0), // Add padding if needed
               child: Center(
                 child: Text(
                    rpnlFormatted,
                    style: TextStyle(fontSize: 14, color: rpnlColor, fontWeight: FontWeight.bold)
                 ),
               ),
             ),
             // Separator
             const Padding(
                padding: EdgeInsets.symmetric(horizontal: 4.0),
                child: Text('|', style: TextStyle(fontSize: 14)),
             ),
             // WS Status
             Padding(
               padding: const EdgeInsets.only(right: 4.0),
               child: Center(child: Text('WS: ${wsStatus.name}', style: const TextStyle(fontSize: 14))), 
             ),
            // Logout Button
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
class PostWidget extends StatefulWidget { // <-- Changed to StatefulWidget
  final Post post;

  const PostWidget({required this.post, super.key});

  @override
  State<PostWidget> createState() => _PostWidgetState();
}

class _PostWidgetState extends State<PostWidget> { // <-- Added State class
  final _quantityController = TextEditingController(text: '1.0'); // Default to 1.0
  final _quantityFocusNode = FocusNode();

  double _buyCost = 0.0;
  double _sellProceeds = 0.0;
  bool _isQuantityValid = true; // Track validity for button enabling

  @override
  void initState() {
    super.initState();
    _quantityController.addListener(_calculateCosts);
    // Calculate initial costs based on default quantity
    WidgetsBinding.instance.addPostFrameCallback((_) => _calculateCosts());
  }

  @override
  void dispose() {
    _quantityController.removeListener(_calculateCosts);
    _quantityController.dispose();
    _quantityFocusNode.dispose();
    super.dispose();
  }

  @override
  void didUpdateWidget(covariant PostWidget oldWidget) {
    super.didUpdateWidget(oldWidget);
    // Recalculate costs if the post supply has changed
    if (oldWidget.post.supply != widget.post.supply) {
       print("Post supply changed (${oldWidget.post.supply} -> ${widget.post.supply}), recalculating costs.");
      _calculateCosts();
    }
  }

  void _calculateCosts() {
    final quantityText = _quantityController.text.trim();
    final quantity = double.tryParse(quantityText);
    
    if (quantity == null || quantity <= 0) {
       if (mounted) { // Check if widget is still mounted
            setState(() {
               _buyCost = 0.0;
               _sellProceeds = 0.0;
               _isQuantityValid = false;
            });
       }
      return;
    }

    final currentSupply = widget.post.supply;
    
    // Calculate cost to buy
    final buyEndSupply = currentSupply + quantity;
    final buyCost = calculateBondingCurveCost(currentSupply, buyEndSupply);

    // Calculate proceeds to sell
    final sellEndSupply = currentSupply - quantity;
    final sellProceeds = calculateBondingCurveCost(sellEndSupply, currentSupply); // Integral from end to start
    
     if (mounted) { // Check if widget is still mounted
        setState(() {
           _buyCost = buyCost.isNaN ? 0.0 : buyCost;
           _sellProceeds = sellProceeds.isNaN ? 0.0 : sellProceeds;
           _isQuantityValid = !buyCost.isNaN && !sellProceeds.isNaN;
        });
     }
  }


  // Helper function to parse quantity and handle errors
  double? _parseQuantity() {
    final quantityText = _quantityController.text.trim();
    final quantity = double.tryParse(quantityText);
    if (quantity == null || quantity <= 0) {
       // Use setState here? Maybe not, as _calculateCosts handles UI state
       // Only show snackbar if trying to submit invalid qty?
       // ScaffoldMessenger.of(context).showSnackBar(
       //  SnackBar(content: Text('Invalid quantity: $quantityText. Must be a positive number.'), backgroundColor: Colors.red),
       // );
      return null;
    }
    return quantity;
  }

   // Helper function to send buy/sell message
  void _sendTradeMessage(String type) {
      if (!_isQuantityValid) { // Use state variable to check validity
         ScaffoldMessenger.of(context).showSnackBar(
            const SnackBar(content: Text('Invalid quantity entered.'), backgroundColor: Colors.red),
         );
         return;
      }
      
      final quantity = _parseQuantity(); // Should be valid if _isQuantityValid is true
      if (quantity == null) return; // Should not happen, but defensive check

      final wsService = Provider.of<WebSocketService>(context, listen: false);
      if (wsService.status == WebSocketStatus.connected) {
          final message = {
              'type': type, // 'buy' or 'sell'
              'post_id': widget.post.id,
              'quantity': quantity,
          };
          wsService.sendMessage(message);
          // Optionally clear or reset quantity after sending
          // _quantityController.text = '1.0';
          _quantityFocusNode.unfocus(); // Hide keyboard
      } else {
            ScaffoldMessenger.of(context).showSnackBar(
            const SnackBar(content: Text('Not connected'), backgroundColor: Colors.orange),
        );
      }
  }


  @override
  Widget build(BuildContext context) {
     // Formatting for display
     final formattedDate = DateFormat.yMd().add_jms().format(widget.post.timestamp.toLocal());
     final formattedPrice = NumberFormat.currency(symbol: '\$', decimalDigits: 4).format(widget.post.price); // Show more precision for price
     final formattedSupply = widget.post.supply.toStringAsFixed(4); // Show precision for supply

     // Consume PositionState to get user's position details for *this* post
     final positionState = Provider.of<PositionState>(context);
     final positionDetail = positionState.positions[widget.post.id];
     final currentPositionSize = positionDetail?.size ?? 0.0; // Default to 0 if no position

     // Consume BalanceState to get user's balance
     final balanceState = Provider.of<BalanceState>(context);
     final currentBalance = balanceState.balance;

     // --- Button Disabling Logic ---
     bool canBuy = _isQuantityValid && _buyCost <= currentBalance;

     // Can sell if: quantity is valid AND (either user is long OR (user is flat/short AND has enough balance for collateral))
     bool canSell = _isQuantityValid && 
         (currentPositionSize > _EPSILON || // User is long (can always reduce/close)
          (currentPositionSize <= _EPSILON && _sellProceeds <= currentBalance) // User is flat/short, check collateral
         );
     // --- End Button Disabling Logic ---

    return Card(
      margin: const EdgeInsets.symmetric(vertical: 8.0, horizontal: 12.0),
      elevation: 2,
      child: Padding(
        padding: const EdgeInsets.all(12.0),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            // Post Content
            Text(widget.post.content, style: Theme.of(context).textTheme.bodyMedium),
            const SizedBox(height: 8),
            // Author and Timestamp
            Text(
              'By: ${widget.post.userId} \nAt: $formattedDate', // Consider fetching/displaying usernames later
              style: Theme.of(context).textTheme.bodySmall?.copyWith(color: Colors.grey[600]),
            ),
            const Divider(height: 16, thickness: 1),
            // Market Info
            Row(
               mainAxisAlignment: MainAxisAlignment.spaceBetween,
               children: [
                  Text('Price: $formattedPrice', style: Theme.of(context).textTheme.titleMedium),
                  Text('Supply: $formattedSupply', style: Theme.of(context).textTheme.bodyMedium),
               ]
            ),
             const SizedBox(height: 8),
             // Display Position Info if it exists
             if (positionDetail != null && positionDetail.size.abs() > 1e-9) // Use epsilon for double comparison
                _buildPositionInfo(context, positionDetail),

              // Quantity Input and Action Buttons Row
             Padding(
               padding: const EdgeInsets.only(top: 8.0),
               child: Row(
                   // mainAxisAlignment: MainAxisAlignment.end,
                   children: [
                      // Quantity Input Field
                       SizedBox(
                          width: 100, // Constrain width of text field
                          child: TextField(
                             controller: _quantityController,
                             focusNode: _quantityFocusNode,
                             decoration: const InputDecoration(
                                labelText: 'Quantity',
                                border: OutlineInputBorder(),
                                isDense: true, // Make it more compact
                                contentPadding: EdgeInsets.symmetric(horizontal: 8.0, vertical: 10.0),
                             ),
                             keyboardType: const TextInputType.numberWithOptions(decimal: true),
                             // InputFormatters? Optional, for stricter input
                             textAlign: TextAlign.right,
                           ),
                       ),
                       const Spacer(), // Push buttons to the right
                       // Buy Button
                       ElevatedButton(
                          onPressed: canBuy ? () => _sendTradeMessage('buy') : null, // Use calculated canBuy
                          style: ElevatedButton.styleFrom(
                            backgroundColor: Colors.green[100],
                            disabledBackgroundColor: Colors.grey[300], // Style for disabled state
                            foregroundColor: canBuy ? Colors.black : Colors.grey[700], // Text color
                          ),
                          child: Text('Buy (\$${_buyCost.toStringAsFixed(2)})') // Show cost with 2 decimal places
                       ),
                       const SizedBox(width: 8),
                       // Sell Button
                       ElevatedButton(
                           onPressed: canSell ? () => _sendTradeMessage('sell') : null, // Use calculated canSell
                           style: ElevatedButton.styleFrom(
                             backgroundColor: Colors.red[100],
                             disabledBackgroundColor: Colors.grey[300], // Style for disabled state
                             foregroundColor: canSell ? Colors.black : Colors.grey[700], // Text color
                           ),
                           child: Text('Sell (\$${_sellProceeds.toStringAsFixed(2)})') // Show proceeds with 2 decimal places
                       ),
                   ],
               ),
             )
          ],
        ),
      ),
    );
  }

  // Helper widget to display position details
  Widget _buildPositionInfo(BuildContext context, PositionDetail detail) {
      final avgPriceFormatted = NumberFormat.currency(symbol: '\$', decimalDigits: 4).format(detail.averagePrice);
      final sizeFormatted = detail.size.toStringAsFixed(4); // Show precision

      // Use PNL directly from the detail object (sent by server)
      final pnlFormatted = NumberFormat.currency(symbol: '\$', decimalDigits: 2).format(detail.unrealizedPnl);
      final pnlColor = detail.unrealizedPnl >= 0 ? Colors.green[700] : Colors.red[700];

      return Container(
          padding: const EdgeInsets.symmetric(vertical: 8.0, horizontal: 4.0),
          margin: const EdgeInsets.only(bottom: 8.0),
          decoration: BoxDecoration(
              border: Border.all(color: Colors.blueGrey.shade100),
              borderRadius: BorderRadius.circular(4.0),
              color: Colors.grey[50],
          ),
          child: Column(
             crossAxisAlignment: CrossAxisAlignment.start,
             children: [
                 Text(
                     'Your Position:',
                     style: Theme.of(context).textTheme.titleSmall?.copyWith(fontWeight: FontWeight.bold)
                 ),
                 const SizedBox(height: 4),
                 Row(
                    mainAxisAlignment: MainAxisAlignment.spaceBetween,
                    children: [
                        Text('Size: $sizeFormatted'), // Use formatted size
                        Text('Avg Price: $avgPriceFormatted'),
                    ],
                 ),
                 const SizedBox(height: 4),
                 // Display PNL directly
                  Row(
                     mainAxisAlignment: MainAxisAlignment.end, // Align PNL to the right
                     children: [
                         Text(
                             'Unrealized PNL: $pnlFormatted',
                             style: TextStyle(color: pnlColor, fontWeight: FontWeight.bold),
                         ),
                     ],
                 ),
             ],
          ),
    );
  }
}
