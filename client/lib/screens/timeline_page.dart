import 'package:flutter/material.dart';
import 'package:provider/provider.dart';
import 'package:intl/intl.dart'; // For date formatting

import '../state/auth_state.dart';
import '../state/timeline_state.dart';
import '../state/balance_state.dart';
import '../state/theme_state.dart'; // Import ThemeState
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
    final themeState = context.watch<ThemeState>(); // Watch ThemeState

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
    // Adjust exposure color based on theme
    final bool isDark = Theme.of(context).brightness == Brightness.dark;
    final exposureColor = exposureRatio > 0.8 
        ? (isDark ? Colors.orange : Colors.orangeAccent)
        : (isDark ? Colors.blue : Colors.blueAccent);

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
            // Add Theme Toggle Switch
            Padding(
              padding: const EdgeInsets.symmetric(horizontal: 8.0),
              child: Switch(
                value: themeState.isDarkMode,
                onChanged: (value) {
                   themeState.toggleTheme();
                },
                thumbIcon: MaterialStateProperty.resolveWith<Icon?>((states) {
                    if (states.contains(MaterialState.selected)) {
                       return const Icon(Icons.dark_mode, color: Colors.black54);
                    } 
                    return const Icon(Icons.light_mode, color: Colors.yellow);
                 }),
              ),
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
      body: RefreshIndicator(
        onRefresh: () async {
          // Handle refresh: Maybe reconnect WebSocket or request fresh data?
          print("Pull to refresh triggered.");
           final token = authState.accessToken;
           if (token != null && _webSocketService.status != WebSocketStatus.connecting) {
             _webSocketService.disconnect(); // Disconnect first
             await Future.delayed(const Duration(milliseconds: 100)); // Small delay
             _webSocketService.connect(token); // Reconnect
           } else if (token == null) {
             print("Cannot refresh: No token");
             authState.signOut(); // Sign out if token lost
           }
        },
        child: Column(
          children: [
            // Display error messages if any
            if (timelineState.error != null)
              Padding(
                padding: const EdgeInsets.all(8.0),
                child: Container(
                   color: Colors.red[100],
                   padding: const EdgeInsets.all(8.0),
                   child: Row(
                      children: [
                        Icon(Icons.error_outline, color: Colors.red[800]),
                        const SizedBox(width: 8),
                        Expanded(child: Text(timelineState.error!, style: TextStyle(color: Colors.red[900]))),
                        IconButton(
                           icon: Icon(Icons.close, size: 16, color: Colors.red[900]),
                           onPressed: () => timelineState.setError(null), // Clear error
                        )
                      ],
                   ),
                ),
              ),
            // Display loading indicator
            if (timelineState.isLoading)
              const Center(child: Padding(
                 padding: EdgeInsets.all(16.0),
                 child: CircularProgressIndicator(),
              )),
            // Display timeline posts
            Expanded(
              child: ListView.builder(
                itemCount: timelineState.posts.length,
                itemBuilder: (context, index) {
                  final post = timelineState.posts[index];
                  // Use the PostWidget here
                  return PostWidget(post: post, key: ValueKey(post.id)); // Use ValueKey for efficient updates
                },
              ),
            ),
          ],
        ),
      ),
      floatingActionButton: FloatingActionButton(
        onPressed: _showCreatePostDialog,
        tooltip: 'Create Post',
        child: const Icon(Icons.add),
      ),
    );
  }

  // Helper to build styled chips for the AppBar bottom
  Widget _buildStatChip(String label, String value, [Color? valueColor]) {
    // Use theme context here to ensure colors adapt
    final theme = Theme.of(context);
    final defaultColor = theme.appBarTheme.foregroundColor ?? (theme.brightness == Brightness.dark ? Colors.white70 : Colors.black87);

     return Chip(
      label: Text('$label: $value'),
      labelStyle: TextStyle(
         fontSize: 11, // Smaller font size
         color: valueColor ?? defaultColor,
      ),
      backgroundColor: theme.appBarTheme.backgroundColor?.withOpacity(0.1), // Subtle background
      padding: const EdgeInsets.symmetric(horizontal: 5.0, vertical: 0), // Reduced padding
      visualDensity: VisualDensity.compact, // Make chip smaller
      materialTapTargetSize: MaterialTapTargetSize.shrinkWrap, // Reduce tap area
      side: BorderSide.none, // Remove border
    );
  }
} 