import 'package:meta/meta.dart'; // For @immutable
import 'dart:math'; // For math operations used in PostDetail

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
  final double? liquidationPrice; // Renamed field

  const PositionDetail({
    required this.postId,
    required this.size,
    required this.averagePrice,
    required this.unrealizedPnl,
    this.liquidationPrice, // Updated constructor parameter
  });

  factory PositionDetail.fromJson(Map<String, dynamic> json) {
    // Log the incoming value for debugging
    final rawLiqPrice = json['liquidation_price']; // Use new field name
    print("PositionDetail.fromJson: Parsing post ${json['post_id']}, liquidation_price raw value: $rawLiqPrice");

    final double? parsedLiqPrice = (rawLiqPrice as num?)?.toDouble();
    print("PositionDetail.fromJson: Parsed post ${json['post_id']}, liquidation_price parsed value: $parsedLiqPrice");

    return PositionDetail(
      postId: json['post_id'] as String,
      size: (json['size'] as num).toDouble(), 
      averagePrice: (json['average_price'] as num).toDouble(),
      unrealizedPnl: (json['unrealized_pnl'] as num).toDouble(),
      liquidationPrice: parsedLiqPrice, // Use the parsed price value
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
            exposure: (json['exposure'] as num).toDouble(),
            equity: (json['equity'] as num).toDouble(),
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
        if (json.containsKey('post_id')) { // Check if it looks like a PositionDetail
          return PositionUpdateMessage(
              position: PositionDetail.fromJson(json)
          );
        } else {
          print("Received position_update message with unexpected format: $json");
          return UnknownMessage(type: type, data: json);
        }
      case 'realized_pnl_update':
        return RealizedPnlUpdateMessage(
          totalRealizedPnl: (json['total_realized_pnl'] as num).toDouble()
        );
      case 'exposure_update':
        return ExposureUpdateMessage(exposure: (json['exposure'] as num).toDouble());
      case 'equity_update':
        return EquityUpdateMessage(equity: (json['equity'] as num).toDouble());
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
  final double exposure;
  final double equity;
  final double total_realized_pnl;
  final List<PositionDetail> positions;
  const UserSyncMessage({
    required this.balance,
    required this.exposure,
    required this.equity,
    required this.total_realized_pnl,
    required this.positions
  });
}

class RealizedPnlUpdateMessage extends ServerMessage {
  final double totalRealizedPnl;
  const RealizedPnlUpdateMessage({required this.totalRealizedPnl});
}

class ExposureUpdateMessage extends ServerMessage {
  final double exposure;
  const ExposureUpdateMessage({required this.exposure});
}

class EquityUpdateMessage extends ServerMessage {
  final double equity;
  const EquityUpdateMessage({required this.equity});
}

class UnknownMessage extends ServerMessage {
   final String type;
   final Map<String, dynamic> data;
   const UnknownMessage({required this.type, required this.data});
} 