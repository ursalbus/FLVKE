import 'package:flutter/foundation.dart';

import '../models/models.dart';

// Added BalanceState
class BalanceState extends ChangeNotifier {
   // Initialize with reasonable defaults or indicate loading state
   double _balance = 0.0; // Start at 0 until synced
   double _totalRealizedPnl = 0.0;
   double _exposure = 0.0; // Added
   double _equity = 0.0; // Added
   bool _isSynced = false; // Track if we have received initial state
   String? _error;

   double get balance => _balance;
   double get totalRealizedPnl => _totalRealizedPnl;
   double get exposure => _exposure; // Added
   double get equity => _equity; // Added
   bool get isSynced => _isSynced; // Expose sync status
   String? get error => _error;

   void handleServerMessage(ServerMessage message) {
     print("BalanceState handling: ${message.runtimeType}");
     bool changed = false;
     String? previousError = _error;
     _error = null; // Clear error on new message (unless it's an error message)

     if (message is UserSyncMessage) { // Handle initial sync
       if (!_isSynced ||
           _balance != message.balance ||
           _totalRealizedPnl != message.total_realized_pnl ||
           _exposure != message.exposure || // Added check
           _equity != message.equity) { // Added check
           _balance = message.balance;
           _totalRealizedPnl = message.total_realized_pnl;
           _exposure = message.exposure; // Added assignment
           _equity = message.equity; // Added assignment
           _isSynced = true; // Mark as synced
           changed = true;
           print("BalanceState Synced: Bal: ${_balance.toStringAsFixed(4)}, RPnl: ${_totalRealizedPnl.toStringAsFixed(4)}, Exp: ${_exposure.toStringAsFixed(4)}, Equity: ${_equity.toStringAsFixed(4)}");
       }
     } else if (message is BalanceUpdateMessage) {
        if (_balance != message.balance) {
            _balance = message.balance;
            print("BalanceState updated: Bal: ${_balance.toStringAsFixed(4)}");
            changed = true;
        }
     } else if (message is RealizedPnlUpdateMessage) {
         if (_totalRealizedPnl != message.totalRealizedPnl) {
            _totalRealizedPnl = message.totalRealizedPnl;
            print("BalanceState updated: RPnl: ${_totalRealizedPnl.toStringAsFixed(4)}");
            changed = true;
         }
     } else if (message is ExposureUpdateMessage) { // Added
         if (_exposure != message.exposure) {
             _exposure = message.exposure;
             print("BalanceState updated: Exposure: ${_exposure.toStringAsFixed(4)}");
             changed = true;
         }
     } else if (message is EquityUpdateMessage) { // Added
         if (_equity != message.equity) {
             _equity = message.equity;
             print("BalanceState updated: Equity: ${_equity.toStringAsFixed(4)}");
             changed = true;
         }
     } else if (message is ErrorMessage) {
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