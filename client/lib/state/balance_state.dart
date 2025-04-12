import 'package:flutter/foundation.dart';

import '../models/models.dart';

// Added BalanceState
class BalanceState extends ChangeNotifier {
   // Initialize with reasonable defaults or indicate loading state
   double _balance = 0.0; // Start at 0 until synced
   double _totalRealizedPnl = 0.0;
   double _margin = 0.0;
   bool _isSynced = false; // Track if we have received initial state
   String? _error;

   double get balance => _balance;
   double get totalRealizedPnl => _totalRealizedPnl; // Added getter
   double get margin => _margin; // Added getter
   bool get isSynced => _isSynced; // Expose sync status
   String? get error => _error;

   void handleServerMessage(ServerMessage message) {
     print("BalanceState handling: ${message.runtimeType}");
     bool changed = false;
     String? previousError = _error;
     _error = null; // Clear error on new message (unless it's an error message)

     if (message is UserSyncMessage) { // Handle initial sync
       if (!_isSynced || _balance != message.balance || _totalRealizedPnl != message.total_realized_pnl || _margin != message.margin) {
           _balance = message.balance;
           _totalRealizedPnl = message.total_realized_pnl;
           _margin = message.margin;
           _isSynced = true; // Mark as synced
           changed = true;
           print("BalanceState Synced: Bal: ${_balance.toStringAsFixed(4)}, RPnl: ${_totalRealizedPnl.toStringAsFixed(4)}, Margin: ${_margin.toStringAsFixed(4)}");
       }
     } else if (message is BalanceUpdateMessage) {
        if (_balance != message.balance) {
            _balance = message.balance;
            print("BalanceState updated: Bal: ${_balance.toStringAsFixed(4)}");
            changed = true;
        }
     } else if (message is RealizedPnlUpdateMessage) { // Added
         if (_totalRealizedPnl != message.totalRealizedPnl) {
            _totalRealizedPnl = message.totalRealizedPnl;
            print("BalanceState updated: RPnl: ${_totalRealizedPnl.toStringAsFixed(4)}");
            changed = true;
         }
     } else if (message is MarginUpdateMessage) { // Added handler
         if (_margin != message.margin) {
             _margin = message.margin;
             print("BalanceState updated: Margin: ${_margin.toStringAsFixed(4)}");
             changed = true;
         }
     } else if (message is ErrorMessage) {
         // Optionally handle errors related to balance if server sends specific ones
          // For now, just log and store generic errors
         _error = message.message;
         print("BalanceState received error: ${message.message}");
          if (previousError != _error) { // Only notify if error actually changes
             changed = true;
          }
     }

     if (changed) {
         notifyListeners();
     }
   }
} 