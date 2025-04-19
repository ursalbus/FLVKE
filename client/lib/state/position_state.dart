import 'package:flutter/foundation.dart';
import 'dart:collection'; // For UnmodifiableMapView

import '../models/models.dart';
import '../utils/bonding_curve.dart'; // For EPSILON

// Added PositionState
class PositionState extends ChangeNotifier {
  Map<String, PositionDetail> _positions = {}; // PostID -> PositionDetail
  bool _isSynced = false;
  String? _error;

  // Expose an unmodifiable view of the positions map
  UnmodifiableMapView<String, PositionDetail> get positions => UnmodifiableMapView(_positions);
  bool get isSynced => _isSynced;
  String? get error => _error;

  void handleServerMessage(ServerMessage message) {
    print("PositionState handling: ${message.runtimeType}");
    _error = null;
    bool changed = false;

    if (message is UserSyncMessage) {
       final newPositions = { for (var p in message.positions) p.postId : p };
       // Log the received positions including liquidationPrice
       print("PositionState: Received UserSync with ${newPositions.length} positions:");
       for (final pos in newPositions.values) {
           print("  - Post ${pos.postId}: Size=${pos.size}, AvgPrc=${pos.averagePrice}, uPnL=${pos.unrealizedPnl}, LiqPrice=${pos.liquidationPrice}");
       }

       // Always update the position map on UserSync, as it represents the full state.
       // Remove the complex and potentially buggy map comparison.
       // if (!_isSynced || !_mapEquals(_positions, newPositions)) { 
           _positions = newPositions;
           _isSynced = true;
           print("PositionState: Synced/Updated ${_positions.length} positions map. Preparing to notify listeners.");
           changed = true;
       // }
    } else if (message is PositionUpdateMessage) {
       final postId = message.position.postId;
       // Remove position if size is near zero, otherwise update/add
       if (message.position.size.abs() < EPSILON) {
           if (_positions.containsKey(postId)) {
              _positions.remove(postId);
              print("PositionState: Removed position for $postId (size near zero).");
              changed = true;
           }
       } else {
          // Update only if the new details are different from the old ones
          if (!_positions.containsKey(postId) || !_positionDetailEquals(_positions[postId]!, message.position)){
               _positions[postId] = message.position;
               print("PositionState: Updated position for $postId");
               changed = true;
          }
       }
    } else if (message is ErrorMessage) {
       if (_error != message.message) {
           _error = message.message;
           print("PositionState received error: ${message.message}");
           changed = true;
       }
    }

    if (changed) {
        notifyListeners();
    }
  }
}

// Helper to compare maps (could be more sophisticated)
bool _mapEquals<K, V>(Map<K, V> map1, Map<K, V> map2) {
  if (map1.length != map2.length) return false;
  for (final k in map1.keys) {
    if (!map2.containsKey(k) || map1[k] != map2[k]) { // Basic comparison, needs deep compare for objects
       if (map1[k] is PositionDetail && map2[k] is PositionDetail) {
           if (!_positionDetailEquals(map1[k] as PositionDetail, map2[k] as PositionDetail)) {
               return false;
           }
       } else if (map1[k] != map2[k]) {
           return false;
       }
    }
  }
  return true;
}

// Helper to compare PositionDetail objects (implement == and hashCode in PositionDetail instead?)
bool _positionDetailEquals(PositionDetail p1, PositionDetail p2) {
  return p1.postId == p2.postId &&
         (p1.size - p2.size).abs() < EPSILON &&
         (p1.averagePrice - p2.averagePrice).abs() < EPSILON &&
         (p1.unrealizedPnl - p2.unrealizedPnl).abs() < EPSILON;
} 