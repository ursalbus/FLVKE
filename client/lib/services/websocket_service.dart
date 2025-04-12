import 'dart:async';
import 'dart:convert';
import 'package:flutter/foundation.dart'; // For ChangeNotifier
import 'package:web_socket_channel/web_socket_channel.dart';

import '../models/models.dart';
import '../constants.dart'; // Import constants

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

    // Use WEBSOCKET_URL from constants.dart
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
                // Send a generic ErrorMessage to all handlers
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
      // Optionally throw an error or return a status
      // throw Exception("WebSocket not connected.");
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
    disconnect(); // Ensure disconnection happens before super.dispose()
    super.dispose();
  }
} 