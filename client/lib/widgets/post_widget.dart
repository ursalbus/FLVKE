import 'package:flutter/material.dart';
import 'package:provider/provider.dart';
import 'package:intl/intl.dart';
import 'dart:math'; // For pow, log, and min

import '../models/models.dart';
import '../state/balance_state.dart';
import '../state/position_state.dart';
import '../services/websocket_service.dart';
import '../utils/bonding_curve.dart'; // Import bonding curve helpers and EPSILON

// Widget to display a single post
class PostWidget extends StatefulWidget {
  final Post post;

  const PostWidget({required this.post, super.key});

  @override
  State<PostWidget> createState() => _PostWidgetState();
}

class _PostWidgetState extends State<PostWidget> {
  final _quantityController = TextEditingController(text: '1.0'); // Default to 1.0
  final _quantityFocusNode = FocusNode();

  // State for calculated costs (used for display)
  double _buyCost = 0.0;
  double _sellProceeds = 0.0;
  // State for input validity and affordability checks
  bool _isQuantityValid = true;
  bool _canAffordBuy = false;
  bool _canAffordSell = false;
  String? _buyDisabledReason;
  String? _sellDisabledReason;

  // Hold references to states to remove listeners in dispose
  late BalanceState _balanceState;
  late PositionState _positionState;

  @override
  void initState() {
    super.initState();
    // Get state references (don't listen here, listen in didChangeDependencies or use context.read)
    _balanceState = Provider.of<BalanceState>(context, listen: false);
    _positionState = Provider.of<PositionState>(context, listen: false);

    _quantityController.addListener(_updateChecks);
    _balanceState.addListener(_updateChecks);
    _positionState.addListener(_updateChecks);

    // Calculate initial costs and checks after the first frame
    WidgetsBinding.instance.addPostFrameCallback((_) => _updateChecks());
  }

  @override
  void dispose() {
    _quantityController.removeListener(_updateChecks);
    _balanceState.removeListener(_updateChecks);
    _positionState.removeListener(_updateChecks);
    _quantityController.dispose();
    _quantityFocusNode.dispose();
    super.dispose();
  }

  @override
  void didUpdateWidget(covariant PostWidget oldWidget) {
    super.didUpdateWidget(oldWidget);
    // Recalculate if the post data relevant to calculation has changed
    if (oldWidget.post.supply != widget.post.supply || oldWidget.post.price != widget.post.price) {
       print("Post data changed (Supply: ${oldWidget.post.supply} -> ${widget.post.supply}, Price: ${oldWidget.post.price} -> ${widget.post.price}), recalculating checks.");
      _updateChecks();
    }
  }

  // Combined function to calculate costs and check affordability
  void _updateChecks() {
    final quantityText = _quantityController.text.trim();
    final quantity = double.tryParse(quantityText);
    bool isValid = false;
    double buyCost = 0.0;
    double sellProceeds = 0.0;
    bool canBuy = false;
    bool canSell = false;
    String? buyReason = 'Enter a valid quantity';
    String? sellReason = 'Enter a valid quantity';

    if (quantity != null && quantity > EPSILON) {
        isValid = true; // Quantity format is valid
        buyReason = null; // Reset reason if quantity is valid
        sellReason = null;

        final currentSupply = widget.post.supply;

        // Calculate cost to buy
        final buyEndSupply = currentSupply + quantity;
        buyCost = calculateBondingCurveCost(currentSupply, buyEndSupply);

        // Calculate proceeds to sell (integral from end to start)
        final sellEndSupply = currentSupply - quantity;
        sellProceeds = calculateBondingCurveCost(sellEndSupply, currentSupply);

        if (buyCost.isNaN || sellProceeds.isNaN) {
            isValid = false; // Calculation failed
            buyReason = 'Calculation error';
            sellReason = 'Calculation error';
        } else {
            // Proceed with affordability checks only if cost calculation succeeded

             // Get necessary state (read here, don't listen)
            final balanceState = context.read<BalanceState>();
            final positionState = context.read<PositionState>();

            if (!balanceState.isSynced || !positionState.isSynced) {
                buyReason = 'Waiting for server sync...';
                sellReason = 'Waiting for server sync...';
            } else {
                final currentBalance = balanceState.balance;
                final currentRealizedPnl = balanceState.totalRealizedPnl;
                final currentExposure = balanceState.exposure;
                final positionDetail = positionState.positions[widget.post.id];
                final oldSize = positionDetail?.size ?? 0.0;
                 // Reconstruct basis from average price and size
                final oldTotalCostBasis = (positionDetail?.averagePrice ?? 0.0) * oldSize;

                final availableCollateral = currentBalance + currentRealizedPnl;

                // --- Check Buy Affordability ---
                try {
                    double deltaExposureBuy = 0.0;
                    // --- Mimic server delta_exposure logic for BUY ---
                    if (oldSize < -EPSILON) { // Covering short
                        final reductionAmount = min(quantity, oldSize.abs());
                        if (reductionAmount > EPSILON) {
                            final avgShortBasisPerShare = (oldSize.abs() > EPSILON) ? (oldTotalCostBasis / oldSize) : 0.0;
                            final exposureReduction = reductionAmount * avgShortBasisPerShare.abs();
                            deltaExposureBuy -= exposureReduction;
                        }
                    }
                    if (oldSize >= -EPSILON) { // Flat or long
                        deltaExposureBuy += buyCost.abs(); // Cost to buy increases exposure
                    } else if (quantity > oldSize.abs()) { // Covered short AND opened long
                        final supplyAtZeroCrossing = currentSupply + oldSize.abs();
                        final costForLongPart = calculateBondingCurveCost(supplyAtZeroCrossing, buyEndSupply);
                        deltaExposureBuy += costForLongPart.abs();
                    }
                    // --- End mimic ---

                    final potentialExposureAfterBuy = currentExposure + deltaExposureBuy;

                    if (potentialExposureAfterBuy <= availableCollateral + EPSILON) { // Add epsilon for float comparison
                        canBuy = true;
                    } else {
                        buyReason = 'Collateral Req: ${potentialExposureAfterBuy.toStringAsFixed(2)} > Avail: ${availableCollateral.toStringAsFixed(2)}';
                    }
                } catch (e) {
                    buyReason = 'Buy check error: $e';
                }


                // --- Check Sell Affordability ---
                try {
                    double deltaExposureSell = 0.0;
                    // --- Mimic server delta_exposure logic for SELL ---
                    if (oldSize > EPSILON) { // Closing long
                        final reductionAmount = min(quantity, oldSize);
                        if (reductionAmount > EPSILON) {
                            final avgLongBasisPerShare = (oldSize.abs() > EPSILON) ? (oldTotalCostBasis / oldSize) : 0.0;
                            final exposureReduction = reductionAmount * avgLongBasisPerShare.abs();
                            deltaExposureSell -= exposureReduction;
                        }
                    }
                    if (oldSize <= EPSILON) { // Flat or short
                         // Selling increases exposure by the proceeds (which act as negative basis)
                        deltaExposureSell += sellProceeds.abs();
                    } else if (quantity > oldSize) { // Closed long AND opened short
                        final supplyAtZeroCrossing = currentSupply - oldSize;
                        final proceedsForShortPart = calculateBondingCurveCost(sellEndSupply, supplyAtZeroCrossing);
                        deltaExposureSell += proceedsForShortPart.abs();
                    }
                    // --- End mimic ---

                    final potentialExposureAfterSell = currentExposure + deltaExposureSell;

                    if (potentialExposureAfterSell <= availableCollateral + EPSILON) { // Add epsilon for float comparison
                        canSell = true;
                    } else {
                         sellReason = 'Collateral Req: ${potentialExposureAfterSell.toStringAsFixed(2)} > Avail: ${availableCollateral.toStringAsFixed(2)}';
                    }
                } catch (e) {
                     sellReason = 'Sell check error: $e';
                }
            } // End if synced
        } // End if cost calc valid
    } else {
       // Handle case where quantity is not valid positive number
       isValid = false;
       buyReason = quantity == null ? 'Invalid quantity' : 'Quantity must be positive';
       sellReason = buyReason;
    }


    if (mounted) { // Check if widget is still mounted
        setState(() {
           _buyCost = isValid ? buyCost : 0.0;
           _sellProceeds = isValid ? sellProceeds : 0.0;
           _isQuantityValid = isValid;
           _canAffordBuy = canBuy;
           _buyDisabledReason = buyReason;
           _canAffordSell = canSell;
           _sellDisabledReason = sellReason;
        });
    }
  }


  // Helper function to parse quantity (already incorporated in _calculateCosts)
  // double? _parseQuantity() { ... }

   // Helper function to send buy/sell message
  void _sendTradeMessage(String type) {
      // Use the state flags which are updated by _updateChecks
      if (!_isQuantityValid) {
         ScaffoldMessenger.of(context).showSnackBar(
            const SnackBar(content: Text('Invalid quantity entered.'), backgroundColor: Colors.red),
         );
         return;
      }
      // Additional check based on affordability state
      if (type == 'buy' && !_canAffordBuy) {
          ScaffoldMessenger.of(context).showSnackBar(
            SnackBar(content: Text('Cannot buy: ${_buyDisabledReason ?? 'Unknown reason'}'), backgroundColor: Colors.orange),
         );
         return;
      }
       if (type == 'sell' && !_canAffordSell) {
          ScaffoldMessenger.of(context).showSnackBar(
            SnackBar(content: Text('Cannot sell: ${_sellDisabledReason ?? 'Unknown reason'}'), backgroundColor: Colors.orange),
         );
         return;
      }


      final quantity = double.tryParse(_quantityController.text.trim());
      if (quantity == null) return; // Should not happen if _isQuantityValid is true

      final wsService = Provider.of<WebSocketService>(context, listen: false);
      if (wsService.status == WebSocketStatus.connected) {
          final message = {
              'type': type, // 'buy' or 'sell'
              'post_id': widget.post.id,
              'quantity': quantity,
          };
          wsService.sendMessage(message);
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
     final formattedPrice = NumberFormat.currency(symbol: r'$', decimalDigits: 4).format(widget.post.price);
     final formattedSupply = widget.post.supply.toStringAsFixed(4);

     // Read position state for display only (don't need to watch if checks done elsewhere)
     final positionState = context.read<PositionState>();
     final positionDetail = positionState.positions[widget.post.id];

    // The actual enabling/disabling logic now relies on state variables
    // _isQuantityValid, _canAffordBuy, _canAffordSell

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
             // Use EPSILON for floating point comparison
             if (positionDetail != null && positionDetail.size.abs() > EPSILON)
                _buildPositionInfo(context, positionDetail),

              // Quantity Input and Action Buttons Row
             Padding(
               padding: const EdgeInsets.only(top: 8.0),
               child: Row(
                   children: [
                      // Quantity Input Field
                       SizedBox(
                          width: 100, // Constrain width of text field
                          child: TextField(
                             controller: _quantityController,
                             focusNode: _quantityFocusNode,
                             decoration: InputDecoration(
                                labelText: 'Quantity',
                                border: const OutlineInputBorder(),
                                isDense: true, // Make it more compact
                                contentPadding: const EdgeInsets.symmetric(horizontal: 8.0, vertical: 10.0),
                                errorText: !_isQuantityValid && _quantityController.text.isNotEmpty ? 'Invalid' : null,
                             ),
                             keyboardType: const TextInputType.numberWithOptions(decimal: true),
                             textAlign: TextAlign.right,
                           ),
                       ),
                       const Spacer(), // Push buttons to the right

                       // Buy Button (Reason shown inside when disabled)
                       ElevatedButton(
                         // Enable button only if quantity is valid AND affordability check passed
                         onPressed: _isQuantityValid && _canAffordBuy ? () => _sendTradeMessage('buy') : null,
                         style: ElevatedButton.styleFrom(
                           backgroundColor: Colors.green[100],
                           disabledBackgroundColor: Colors.grey[300],
                           foregroundColor: _isQuantityValid && _canAffordBuy ? Colors.green[900] : Colors.grey[700],
                           padding: const EdgeInsets.symmetric(horizontal: 12, vertical: 8),
                         ),
                         child: Column(
                            mainAxisSize: MainAxisSize.min, // Fit content
                            children: [
                               const Text('Buy'),
                               // Show cost if enabled, otherwise show reason
                               Text(
                                 _isQuantityValid && _canAffordBuy
                                  ? '(\$${_buyCost.toStringAsFixed(2)})'
                                  : (_buyDisabledReason ?? '').replaceAll('>', '\n>'), // Attempt to wrap reason
                                 style: TextStyle(
                                   fontSize: 10,
                                   color: _isQuantityValid && _canAffordBuy ? Colors.green[900] : Colors.grey[700]
                                 ),
                                 textAlign: TextAlign.center, // Center reason text
                                 softWrap: true,
                               ),
                            ]
                         ),
                       ),
                       const SizedBox(width: 8), // Add spacing

                       // Sell Button (Reason shown inside when disabled)
                       ElevatedButton(
                          // Enable button only if quantity is valid AND affordability check passed
                         onPressed: _isQuantityValid && _canAffordSell ? () => _sendTradeMessage('sell') : null,
                         style: ElevatedButton.styleFrom(
                           backgroundColor: Colors.red[100],
                           disabledBackgroundColor: Colors.grey[300],
                           foregroundColor: _isQuantityValid && _canAffordSell ? Colors.red[900] : Colors.grey[700],
                           padding: const EdgeInsets.symmetric(horizontal: 12, vertical: 8),
                         ),
                         child: Column(
                            mainAxisSize: MainAxisSize.min, // Fit content
                            children: [
                               const Text('Sell'),
                                // Show proceeds if enabled, otherwise show reason
                                Text(
                                  _isQuantityValid && _canAffordSell
                                  ? '(\$${_sellProceeds.toStringAsFixed(2)})' // Sell proceeds should be positive
                                  : (_sellDisabledReason ?? '').replaceAll('>', '\n>'), // Attempt to wrap reason
                                  style: TextStyle(
                                    fontSize: 10,
                                    color: _isQuantityValid && _canAffordSell ? Colors.red[900] : Colors.grey[700]
                                  ),
                                  textAlign: TextAlign.center, // Center reason text
                                  softWrap: true,
                                ),
                            ]
                         ),
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
  Widget _buildPositionInfo(BuildContext context, PositionDetail positionDetail) {
      final formattedSize = positionDetail.size.toStringAsFixed(4);
      final formattedAvgPrice = NumberFormat.currency(symbol: r'$', decimalDigits: 4).format(positionDetail.averagePrice.abs());
      final formattedUnrealizedPnl = NumberFormat.currency(symbol: r'$', decimalDigits: 2).format(positionDetail.unrealizedPnl);
      final pnlColor = positionDetail.unrealizedPnl >= 0 ? Colors.green : Colors.red;

       return Padding(
         padding: const EdgeInsets.only(top: 4.0, bottom: 4.0),
         child: Column(
             crossAxisAlignment: CrossAxisAlignment.start,
             children: [
                 const Divider(),
                 Text(
                     'Position: $formattedSize @ $formattedAvgPrice',
                     style: Theme.of(context).textTheme.bodyMedium?.copyWith(fontWeight: FontWeight.bold),
                 ),
                 Text(
                     'Unrealized P&L: $formattedUnrealizedPnl',
                     style: Theme.of(context).textTheme.bodyMedium?.copyWith(color: pnlColor),
                 ),
                 const Divider(),
             ],
         ),
       );
  }
} 