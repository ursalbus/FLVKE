import 'package:flutter/foundation.dart';
import 'package:supabase_flutter/supabase_flutter.dart';

// Authentication service/state
class AuthState extends ChangeNotifier {
  User? _user;
  User? get user => _user;
  String? _accessToken; // Store the access token
  String? get accessToken => _accessToken;

  final SupabaseClient _supabase = Supabase.instance.client;

  AuthState() {
    // Restore session if available (useful for hot restarts, though full persistence handled by Supabase)
    _user = _supabase.auth.currentUser;
    _accessToken = _supabase.auth.currentSession?.accessToken;
    print("AuthState Initialized: User: ${_user?.id}, Token: ${_accessToken != null}");

    _supabase.auth.onAuthStateChange.listen((data) {
      final previousToken = _accessToken;
      _user = data.session?.user;
      _accessToken = data.session?.accessToken; // Store access token on auth change

      // Only notify if user or token presence actually changes
      if (_user != data.session?.user || (_accessToken != null) != (previousToken != null)) {
          print("Auth State Changed: User: ${_user?.id}, Token: ${_accessToken != null}");
          notifyListeners();
      }
    });
  }

  Future<String?> signInWithEmail(String email, String password) async {
    try {
      final AuthResponse res = await _supabase.auth.signInWithPassword(
        email: email,
        password: password,
      );
       // State change listener will handle updating _user, _accessToken and notifying
      // _user = res.user;
      // _accessToken = res.session?.accessToken;
      // notifyListeners();
      return null; // Sign in successful
    } on AuthException catch (e) {
      print("Sign In Error: ${e.message}");
      return e.message; // Return error message
    } catch (e) {
      print("Sign In Error: $e");
      return 'An unexpected error occurred.';
    }
  }

  Future<String?> signUpWithEmail(String email, String password) async {
     try {
      // Note: Supabase email auth typically requires email confirmation.
      // For simplicity here, we'll assume it's enabled or auto-confirmed.
      final AuthResponse res = await _supabase.auth.signUp(
        email: email,
        password: password,
      );
       // Sign up might not immediately create a session depending on email verification settings
       // State change listener handles updates
      // _user = res.user;
      // _accessToken = res.session?.accessToken;
      // notifyListeners();
      return null; // Sign up successful (or verification email sent)
    } on AuthException catch (e) {
      print("Sign Up Error: ${e.message}");
      return e.message; // Return error message
    } catch (e) {
      print("Sign Up Error: $e");
      return 'An unexpected error occurred.';
    }
  }

  Future<void> signOut() async {
    await _supabase.auth.signOut();
    // State change listener handles updates
    // _user = null;
    // _accessToken = null;
    // notifyListeners();
  }
} 