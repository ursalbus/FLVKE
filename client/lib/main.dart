import 'package:flutter/material.dart';
import 'package:supabase_flutter/supabase_flutter.dart' hide AuthState;
import 'package:provider/provider.dart';

// Import themes and states
import 'theme.dart';
import 'state/theme_state.dart';
import 'state/auth_state.dart';
import 'state/timeline_state.dart';
import 'state/balance_state.dart';
import 'state/position_state.dart';
import 'services/websocket_service.dart';
import 'screens/login_page.dart';
import 'screens/timeline_page.dart';

Future<void> main() async {
  WidgetsFlutterBinding.ensureInitialized();

  // TODO: Load these from environment variables (.env file) instead of hardcoding or using fromEnvironment defaults.
  // Using String.fromEnvironment is better than hardcoding but still not ideal for secrets.
  const supabaseUrl = String.fromEnvironment('SUPABASE_URL', defaultValue: 'https://ayjbspnnvjqhhbioeapo.supabase.co');
  const supabaseAnonKey = String.fromEnvironment('SUPABASE_ANON_KEY', defaultValue: 'eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJpc3MiOiJzdXBhYmFzZSIsInJlZiI6ImF5amJzcG5udmpxaGhiaW9lYXBvIiwicm9sZSI6ImFub24iLCJpYXQiOjE3NDQxMjY5NzQsImV4cCI6MjA1OTcwMjk3NH0.KkBaeBQrjfruaLRAXiLu9xvloCgAfjQe5FmEcf98djQ');

  if (supabaseUrl.isEmpty || supabaseAnonKey.isEmpty || supabaseUrl == 'YOUR_SUPABASE_URL' || supabaseAnonKey == 'YOUR_SUPABASE_ANON_KEY') {
     print("---");
     print("Error: Supabase URL/Anon Key not configured.");
     print("Ensure SUPABASE_URL and SUPABASE_ANON_KEY are set via --dart-define");
     print("or replace the default values in main.dart (NOT recommended for production).");
     print("---");
     // Consider showing an error screen or exiting gracefully
  }

  try {
      await Supabase.initialize(
        url: supabaseUrl,
        anonKey: supabaseAnonKey,
      );
  } catch (e) {
     print("Error initializing Supabase: $e");
     // Display an error UI to the user?
     return; // Exit if Supabase fails to initialize
  }


  runApp(
    MultiProvider(
      providers: [
        // State Providers (independent)
        ChangeNotifierProvider(create: (_) => AuthState()),
        ChangeNotifierProvider(create: (_) => TimelineState()),
        ChangeNotifierProvider(create: (_) => BalanceState()),
        ChangeNotifierProvider(create: (_) => PositionState()),
        ChangeNotifierProvider(create: (_) => ThemeState()), // Provide ThemeState

        // WebSocket Service Provider (depends on states for message handlers)
        ChangeNotifierProxyProvider3<TimelineState, BalanceState, PositionState, WebSocketService>(
           create: (context) {
              print("ProxyProvider creating initial WebSocketService...");
              final timelineState = context.read<TimelineState>();
              final balanceState = context.read<BalanceState>();
              final positionState = context.read<PositionState>();
              return WebSocketService(onMessageReceivedHandlers: [
                  timelineState.handleServerMessage,
                  balanceState.handleServerMessage,
                  positionState.handleServerMessage,
              ]);
           },
           update: (context, timelineState, balanceState, positionState, previousWebSocketService) {
              print("ProxyProvider updating WebSocketService... Reusing previous: ${previousWebSocketService != null}");
              // Return the previous instance to avoid creating a new one unnecessarily.
              return previousWebSocketService ??
                  WebSocketService(onMessageReceivedHandlers: [ // Should ideally not happen if create worked
                    timelineState.handleServerMessage,
                    balanceState.handleServerMessage,
                    positionState.handleServerMessage,
                  ]);
            },
            // Provider automatically handles calling dispose on ChangeNotifiers like WebSocketService
         ),
      ],
      // Use Consumer<ThemeState> to rebuild MaterialApp when theme changes
      child: Consumer<ThemeState>(
        builder: (context, themeState, _) {
          return const MyApp(); // MyApp itself doesn't need themeState directly
        }
      ),
    ),
  );
}

class MyApp extends StatelessWidget {
  const MyApp({super.key});

  @override
  Widget build(BuildContext context) {
    // Get the theme state to determine the mode
    final themeState = Provider.of<ThemeState>(context);

    return MaterialApp(
      title: 'FLVKE',
      theme: lightTheme, // Use light theme data from theme.dart
      darkTheme: darkTheme, // Use dark theme data from theme.dart
      themeMode: themeState.themeMode, // Control theme mode via state

      home: Consumer<AuthState>(
        builder: (context, authState, _) {
          if (authState?.user != null && authState?.accessToken != null) {
            print("User authenticated, showing TimelinePage.");
            return const TimelinePage();
          } else {
             print("User not authenticated, showing LoginPage.");
            return const LoginPage();
          }
        },
      ),
      debugShowCheckedModeBanner: false,
    );
  }
} 