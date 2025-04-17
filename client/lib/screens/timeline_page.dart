import 'package:flutter/material.dart';
import 'package:provider/provider.dart';
import 'package:intl/intl.dart'; // For date formatting

import '../state/auth_state.dart';
import '../state/timeline_state.dart';
import '../state/balance_state.dart';
import '../services/websocket_service.dart';
import '../widgets/post_widget.dart'; // Import PostWidget

class TimelinePage extends StatefulWidget {
  const TimelinePage({super.key});

  @override
  State<TimelinePage> createState() => _TimelinePageState();
}

class _TimelinePageState extends State<TimelinePage> {
  late final WebSocketService _webSocketService;
  late final AuthState _authState;
  late final TimelineState _timelineState;
  // No need for BalanceState here if only used in build method

  final _postContentController = TextEditingController();

  @override
  void initState() {
    super.initState();
    _authState = Provider.of<AuthState>(context, listen: false);
    _webSocketService = Provider.of<WebSocketService>(context, listen: false);
    _timelineState = Provider.of<TimelineState>(context, listen: false);

    // Listen to WebSocket status for UI feedback and timeline state updates
    _webSocketService.addListener(_onWebSocketStatusChanged);

    _connectWebSocketIfNeeded();
  }

  void _connectWebSocketIfNeeded() {
     final token = _authState.accessToken;
     if (token != null && _webSocketService.status != WebSocketStatus.connected && _webSocketService.status != WebSocketStatus.connecting) {
       print("TimelinePage: Attempting WS connect.");
       _timelineState.setLoading(true); // Show loading initially
       // Use WidgetsBinding.instance.addPostFrameCallback to ensure context is ready if needed,
       // but often direct call is fine in initState if providers are already set up.
       _webSocketService.connect(token).catchError((e) {
           print("TimelinePage: Error during initial connect: $e");
           if (mounted) {
             _timelineState.setError("Failed to connect: $e");
             _timelineState.setLoading(false);
           }
       });
     } else if (token == null) {
       print("TimelinePage: No access token found on init!");
       if (mounted) {
           _timelineState.setError("Authentication token not found.");
           _timelineState.setLoading(false);
       }
     } else {
         print("TimelinePage: WS already connecting or connected.");
         // If already connected, ensure loading state reflects reality
         if (_webSocketService.status == WebSocketStatus.connected && _timelineState.isLoading) {
             // If WS is connected but timeline is still loading, it might mean
             // we reconnected but haven't received InitialState yet.
             // Or, it might be safe to assume loading is false if WS is connected.
             // Let the handleServerMessage in TimelineState manage loading=false on InitialState.
         }
     }
  }

  void _onWebSocketStatusChanged() {
    if (mounted) {
      final status = _webSocketService.status;
      final error = _webSocketService.connectionError;
      print("TimelinePage: WS Status Changed: $status, Error: $error");

      // Update TimelineState based on WebSocket status
      // Let TimelineState handle errors received via messages
      // Only set timeline error specifically for connection issues here
      if (status == WebSocketStatus.error && error != null) {
         _timelineState.setError("WebSocket Connection Error: $error");
         // _timelineState.setLoading(false); // setLoading also handles this
      } else if (status == WebSocketStatus.disconnected) {
          // Handle clean disconnects or errors leading to disconnect
          final disconnectError = error ?? "Disconnected";
           _timelineState.setError("WebSocket Disconnected: $disconnectError");
           // _timelineState.setLoading(false); // setLoading handles this
           // Optionally attempt reconnect after a delay?
      } else if (status == WebSocketStatus.connecting) {
           _timelineState.setLoading(true); // Ensure loading is true
           // _timelineState.setError(null); // setLoading handles this
      } else if (status == WebSocketStatus.connected) {
           // Clear connection-specific errors from timeline state
           if (_timelineState.error?.startsWith("WebSocket") ?? false) {
               _timelineState.setError(null);
           }
           // Loading state turned off by InitialState message handling in TimelineState
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
    // Don't disconnect WebSocket here, let it live with the service
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
        // Use a local TextEditingController for the dialog
        final dialogContentController = TextEditingController();
        return AlertDialog(
          title: const Text('Create New Post'),
          content: TextField(
            controller: dialogContentController,
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
                final content = dialogContentController.text.trim();
                if (content.isNotEmpty) {
                  final message = {
                    'type': 'create_post',
                    'content': content,
                  };
                  // Access WS Service via Provider if not keeping a local ref
                  // Provider.of<WebSocketService>(context, listen: false).sendMessage(message);
                  _webSocketService.sendMessage(message);
                  dialogContentController.clear(); // Clear dialog controller
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
    ).then((_) {
       // Dispose the dialog controller when the dialog is closed
       // dialogContentController.dispose(); // This approach has issues
       // It's often simpler to manage the dialog controller within the builder
       // or use the main page controller if appropriate (but clearing needed)
    });
     // Clear the main controller after showing dialog to prevent reuse
     _postContentController.clear();
  }

  @override
  Widget build(BuildContext context) {
    // Listen to necessary states here
    final authState = context.read<AuthState>(); // Read is often enough if only used for actions
    final wsStatus = context.watch<WebSocketService>().status; // Watch for UI updates
    final balanceState = context.watch<BalanceState>(); // Watch for balance updates
    final timelineState = context.watch<TimelineState>(); // Watch for posts, loading, errors

    // Format currency values
    final numberFormat = NumberFormat.currency(symbol: '\$', decimalDigits: 2);
    final equityFormatted = numberFormat.format(balanceState.equity);
    final balanceFormatted = numberFormat.format(balanceState.balance);
    final exposureFormatted = numberFormat.format(balanceState.exposure);
    // final marginFormatted = numberFormat.format(balanceState.margin); // Removed
    final rpnlFormatted = numberFormat.format(balanceState.totalRealizedPnl);
    final rpnlColor = balanceState.totalRealizedPnl >= 0 ? Colors.green[700] : Colors.red[700];

    // Calculate exposure ratio for the progress bar
    final availableCollateral = balanceState.balance + balanceState.totalRealizedPnl;
    final exposureRatio = (availableCollateral > 0 && balanceState.exposure >= 0)
        ? (balanceState.exposure / availableCollateral).clamp(0.0, 1.0)
        : 0.0;
    final exposureColor = exposureRatio > 0.8 ? Colors.orangeAccent : Colors.blueAccent;

    // Format the new value
    final availableCollateralFormatted = numberFormat.format(availableCollateral);

    return Scaffold(
      appBar: AppBar(
        // Use AppBar's bottom property for the summary and progress bar
        bottom: PreferredSize(
          preferredSize: const Size.fromHeight(38.0), // Reduced height slightly
          child: Padding(
            padding: const EdgeInsets.symmetric(horizontal: 8.0, vertical: 2.0), // Reduced vertical padding
            child: Column(
              mainAxisAlignment: MainAxisAlignment.center, // Center content vertically
              children: [
                 // Row for key financial figures
                 Wrap( // Use Wrap for better spacing on smaller screens
                   spacing: 6.0, // Slightly reduced Horizontal spacing
                   runSpacing: 1.0, // Reduced Vertical spacing if wraps
                   alignment: WrapAlignment.center,
                   children: [
                     _buildStatChip('Equity', equityFormatted),
                     _buildStatChip('Balance', balanceFormatted),
                     _buildStatChip('Available Collateral', availableCollateralFormatted),
                     _buildStatChip('Exposure', exposureFormatted),
                     _buildStatChip('RPnL', rpnlFormatted, rpnlColor),
                   ],
                 ),
                 // const SizedBox(height: 4.0), // Removing SizedBox or making it smaller
                 // Exposure Indicator
                 Tooltip(
                    message: 'Exposure / (Balance + Realized PnL)',
                    child: LinearProgressIndicator(
                       value: exposureRatio,
                       backgroundColor: Colors.grey[300],
                       valueColor: AlwaysStoppedAnimation<Color>(exposureColor),
                       minHeight: 6, // Make it a bit thicker
                    ),
                 ),
              ],
            ),
          ),
        ),
        // Keep WS Status and Logout in actions
        actions: [
             Padding(
               padding: const EdgeInsets.symmetric(horizontal: 8.0),
               child: Center(child: Text('WS: ${wsStatus.name}', style: const TextStyle(fontSize: 14))), // Use status name
             ),
           IconButton(
             icon: const Icon(Icons.logout),
             tooltip: 'Logout', // Add tooltip
             onPressed: () async {
                context.read<WebSocketService>().disconnect();
                await authState.signOut();
             },
           ),
         ],
      ),
      body: Column( // Main body structure
        children: [
          // Error display area - now uses timelineState directly
          if (timelineState.error != null)
            _buildErrorBanner(timelineState.error!),

          // Loading indicator or Post list area
          Expanded(
            child: _buildTimelineContent(timelineState),
          ),
        ],
      ),
      // Floating action button to create posts
      floatingActionButton: FloatingActionButton(
        onPressed: wsStatus == WebSocketStatus.connected ? _showCreatePostDialog : null,
        tooltip: 'New Post',
        backgroundColor: wsStatus == WebSocketStatus.connected
            ? Theme.of(context).colorScheme.primary
            : Colors.grey, // Disable visually if not connected
        child: const Icon(Icons.add),
      ),
    );
  }

  // Helper widget to build styled stat chips for the AppBar
  Widget _buildStatChip(String label, String value, [Color? valueColor]) {
     return Chip(
        padding: const EdgeInsets.symmetric(horizontal: 3.0, vertical: 0.0), // Reduced padding
        labelPadding: const EdgeInsets.symmetric(horizontal: 3.0), // Reduced padding
        materialTapTargetSize: MaterialTapTargetSize.shrinkWrap,
        label: Text(
          '$label: $value',
          style: TextStyle(
              fontSize: 12,
              fontWeight: FontWeight.bold,
              color: valueColor,
           ),
        ),
        backgroundColor: Colors.grey[200],
        side: BorderSide(color: Colors.grey[400]!),
     );
  }

  // Helper to build the error banner
  Widget _buildErrorBanner(String errorMessage) {
     return Container(
         color: Colors.red[100],
         padding: const EdgeInsets.all(8.0),
         child: Row(
           children: [
              Icon(Icons.error_outline, color: Colors.red[700]),
              const SizedBox(width: 8),
              Expanded(child: Text(errorMessage)), // Use the passed message
           ],
         ),
     );
  }

  // Helper to build the main content area (loading/empty/list)
  Widget _buildTimelineContent(TimelineState timelineState) {
     if (timelineState.isLoading) {
        return const Center(child: CircularProgressIndicator());
     // Show error here only if not loading and error exists? Or rely on banner?
     // } else if (timelineState.error != null) {
     //    return Center(child: Text('Error: ${timelineState.error}')); // Can duplicate banner
     } else if (timelineState.posts.isEmpty) {
        return const Center(child: Text('No posts yet. Create one!'));
     } else {
        // Display the list of posts
        return RefreshIndicator( // Add pull-to-refresh
           onRefresh: () async {
              // Reconnect or send a refresh request? Currently, reconnects.
              print("Pull to refresh triggered.");
              // _webSocketService.disconnect(); // Force disconnect? Risky.
              // Give some time for potential disconnect message
              // await Future.delayed(const Duration(milliseconds: 100));
              _connectWebSocketIfNeeded(); // Attempt connection
           },
           child: ListView.builder(
             // Add keys if needed for performance/state preservation
             // key: const PageStorageKey<String>('timelineList'),
             physics: const AlwaysScrollableScrollPhysics(), // Ensure scrollable even with few items for RefreshIndicator
             itemCount: timelineState.posts.length,
             itemBuilder: (context, index) {
                final post = timelineState.posts[index];
                // Use ValueKey for better list performance if post IDs are stable
                return PostWidget(key: ValueKey(post.id), post: post);
             },
           ),
        );
     }
  }
} 