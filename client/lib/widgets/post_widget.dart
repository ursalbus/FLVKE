import 'package:flutter/material.dart';
import 'package:provider/provider.dart';
import 'package:intl/intl.dart';
import 'dart:math'; // For pow and log

import '../models/models.dart';
import '../state/balance_state.dart';
import '../state/position_state.dart';
import '../services/websocket_service.dart';
import '../utils/bonding_curve.dart'; // Import bonding curve helpers

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
    // Recalculate costs if the post data relevant to cost calculation has changed
    if (oldWidget.post.supply != widget.post.supply || oldWidget.post.price != widget.post.price) {
       print("Post data changed (Supply: ${oldWidget.post.supply} -> ${widget.post.supply}, Price: ${oldWidget.post.price} -> ${widget.post.price}), recalculating costs.");
      _calculateCosts();
    }
  }

  void _calculateCosts() {
    final quantityText = _quantityController.text.trim();
    final quantity = double.tryParse(quantityText);
    bool isValid = false;
    double buyCost = 0.0;
    double sellProceeds = 0.0;

    if (quantity != null && quantity > 0) {
        final currentSupply = widget.post.supply;

        // Calculate cost to buy
        final buyEndSupply = currentSupply + quantity;
        buyCost = calculateBondingCurveCost(currentSupply, buyEndSupply);

        // Calculate proceeds to sell
        final sellEndSupply = currentSupply - quantity;
        // Need to handle potential invalid sell (e.g., supply cannot go below a certain point if defined)
        // For now, assume selling is always possible mathematically, but cost is negative integral
        sellProceeds = calculateBondingCurveCost(currentSupply, sellEndSupply);
        // Cost function returns Integral[s1, s2]. Selling means s2 < s1.
        // We want the *proceeds* which is the money received, so it should be positive.
        // Cost = Integral[s1, s2] = I(s2) - I(s1).
        // Proceeds = -Cost = I(s1) - I(s2).
        // Let's redefine sellProceeds = calculateBondingCurveCost(sellEndSupply, currentSupply);
        sellProceeds = calculateBondingCurveCost(sellEndSupply, currentSupply); // Integral from end to start

        isValid = !buyCost.isNaN && !sellProceeds.isNaN;
    }

    if (mounted) { // Check if widget is still mounted
        setState(() {
           _buyCost = isValid ? buyCost : 0.0;
           _sellProceeds = isValid ? sellProceeds : 0.0;
           _isQuantityValid = isValid;
        });
    }
  }


  // Helper function to parse quantity (already incorporated in _calculateCosts)
  // double? _parseQuantity() { ... }

   // Helper function to send buy/sell message
  void _sendTradeMessage(String type) {
      // No need to parse again if _isQuantityValid depends on successful parsing
      if (!_isQuantityValid) {
         ScaffoldMessenger.of(context).showSnackBar(
            const SnackBar(content: Text('Invalid quantity entered.'), backgroundColor: Colors.red),
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

     // Consume states needed for logic/display
     final positionState = context.watch<PositionState>(); // Watch for position updates
     final balanceState = context.watch<BalanceState>(); // Watch for balance updates

     final positionDetail = positionState.positions[widget.post.id];
     final currentPositionSize = positionDetail?.size ?? 0.0; // Default to 0 if no position
     final currentBalance = balanceState.balance;

     // --- Button Disabling Logic ---
     // Ensure quantity is valid before checking balance/position
     bool canBuy = _isQuantityValid && (_buyCost <= currentBalance);
     // Can sell if: quantity is valid AND (user is long OR user has enough balance for sell operation)
     // If user is long, they can always sell up to their position size.
     // If user is flat or short, selling requires collateral (covered by balance check here)
     // Note: Server will ultimately validate margin requirements.
     // Let's simplify: Can sell if quantity is valid AND proceeds are calculable (which _isQuantityValid checks)
     // AND (either they have a long position OR they have enough balance to cover potential negative proceeds - unlikely with this curve)
     // The primary constraint for selling (going short) is margin, which isn't directly checked here yet.
     // For now, we check if they have a long position OR if the *cost* of the operation (sellProceeds) can be covered by balance.
     // This isn't perfect collateral check, but simpler for client-side estimate.
     bool canSell = _isQuantityValid &&
                    (currentPositionSize > EPSILON || // User is long enough to cover sell qty (approximate)
                     _sellProceeds <= currentBalance // Or user has balance to cover the operation cost/collateral (approximate)
                    );
      // Let's refine canSell: You can always sell if you are long.
      // If you are flat or short, you need sufficient balance to cover the *cost* of the operation.
      // calculateBondingCurveCost(s_current, s_current - qty) should give the cost.
      // If cost is negative (get money), always allowed (ignoring margin for now).
      // If cost is positive (pay money), need balance.
      // Let's recalculate sellCost explicitly for clarity
      double sellCost = 0.0;
      final quantity = double.tryParse(_quantityController.text.trim());
      if (quantity != null && quantity > 0) {
          sellCost = calculateBondingCurveCost(widget.post.supply, widget.post.supply - quantity);
      }

      canSell = _isQuantityValid &&
                ( (currentPositionSize >= quantity! - EPSILON) || // Have enough existing position to sell
                  (sellCost <= currentBalance) ); // Or enough balance to cover the cost of selling

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
                                errorText: !_isQuantityValid && _quantityController.text.isNotEmpty ? 'Invalid' : null, // Show simple error indication
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
                            foregroundColor: canBuy ? Colors.green[900] : Colors.grey[700], // Text color
                            padding: const EdgeInsets.symmetric(horizontal: 12, vertical: 8), // Adjust padding
                          ),
                          child: Column( // Display Buy and Cost vertically
                             children: [
                                const Text('Buy'),
                                Text('(\$${_buyCost.toStringAsFixed(2)})', style: const TextStyle(fontSize: 10)),
                             ],
                           )
                       ),
                       const SizedBox(width: 8),
                       // Sell Button
                       ElevatedButton(
                           onPressed: canSell ? () => _sendTradeMessage('sell') : null, // Use calculated canSell
                           style: ElevatedButton.styleFrom(
                             backgroundColor: Colors.red[100],
                             disabledBackgroundColor: Colors.grey[300], // Style for disabled state
                             foregroundColor: canSell ? Colors.red[900] : Colors.grey[700], // Text color
                             padding: const EdgeInsets.symmetric(horizontal: 12, vertical: 8), // Adjust padding
                           ),
                           child: Column( // Display Sell and Proceeds vertically
                             children: [
                               const Text('Sell'),
                               Text('(+\$${_sellProceeds.toStringAsFixed(2)})', style: const TextStyle(fontSize: 10)), // Show positive for proceeds
                             ],
                           )
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