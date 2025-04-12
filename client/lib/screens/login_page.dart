import 'package:flutter/material.dart';
import 'package:provider/provider.dart';

import '../state/auth_state.dart';

class LoginPage extends StatefulWidget {
  const LoginPage({super.key});

  @override
  State<LoginPage> createState() => _LoginPageState();
}

class _LoginPageState extends State<LoginPage> {
  final _emailController = TextEditingController();
  final _passwordController = TextEditingController();
  final _formKey = GlobalKey<FormState>(); // Add form key for validation
  bool _isLoading = false;

  @override
  void dispose() {
    _emailController.dispose();
    _passwordController.dispose();
    super.dispose();
  }

  Future<void> _handleSignIn() async {
    // Validate form
    if (!_formKey.currentState!.validate()) return;

    setState(() => _isLoading = true);
    final authState = Provider.of<AuthState>(context, listen: false);
    final error = await authState.signInWithEmail(
      _emailController.text.trim(),
      _passwordController.text.trim(),
    );
    if (mounted) {
       setState(() => _isLoading = false);
      if (error != null) {
        ScaffoldMessenger.of(context).showSnackBar(
          SnackBar(content: Text('Sign In Failed: $error'), backgroundColor: Colors.red),
        );
      }
      // No need for else, listener in main will handle navigation
    }
  }

   Future<void> _handleSignUp() async {
    // Validate form
    if (!_formKey.currentState!.validate()) return;

    setState(() => _isLoading = true);
    final authState = Provider.of<AuthState>(context, listen: false);
     final error = await authState.signUpWithEmail(
      _emailController.text.trim(),
      _passwordController.text.trim(),
    );

     if (mounted) {
       setState(() => _isLoading = false);
      if (error != null) {
         ScaffoldMessenger.of(context).showSnackBar(
          SnackBar(content: Text('Sign Up Failed: $error'), backgroundColor: Colors.red),
        );
      } else {
         // Optionally show a success message or prompt to check email
         ScaffoldMessenger.of(context).showSnackBar(
           const SnackBar(content: Text('Sign Up successful! Please check your email if verification is required.'), backgroundColor: Colors.green),
         );
      }
    }
  }

  @override
  Widget build(BuildContext context) {
    return Scaffold(
      appBar: AppBar(title: const Text('Login / Sign Up')),
      body: Center( // Center the content
        child: SingleChildScrollView( // Allow scrolling on small screens
          padding: const EdgeInsets.all(24.0), // Increase padding
          child: Form( // Wrap with Form widget
             key: _formKey,
             child: Column(
               mainAxisAlignment: MainAxisAlignment.center,
               crossAxisAlignment: CrossAxisAlignment.stretch, // Stretch buttons
               children: [
                 TextFormField( // Use TextFormField for validation
                   controller: _emailController,
                   decoration: const InputDecoration(
                     labelText: 'Email',
                     prefixIcon: Icon(Icons.email),
                     border: OutlineInputBorder(),
                    ),
                   keyboardType: TextInputType.emailAddress,
                   enabled: !_isLoading,
                   validator: (value) {
                      if (value == null || value.isEmpty) {
                        return 'Please enter your email';
                      }
                      // Basic email format check
                      if (!RegExp(r"^[a-zA-Z0-9.+-]+@[a-zA-Z0-9-]+\.[a-zA-Z0-9-.]+").hasMatch(value)) {
                         return 'Please enter a valid email address';
                      }
                      return null;
                   },
                 ),
                 const SizedBox(height: 16), // Increase spacing
                 TextFormField( // Use TextFormField for validation
                   controller: _passwordController,
                   decoration: const InputDecoration(
                      labelText: 'Password',
                      prefixIcon: Icon(Icons.lock),
                      border: OutlineInputBorder(),
                    ),
                   obscureText: true,
                   enabled: !_isLoading,
                   validator: (value) {
                      if (value == null || value.isEmpty) {
                         return 'Please enter your password';
                      }
                      if (value.length < 6) { // Example minimum length
                          return 'Password must be at least 6 characters long';
                      }
                      return null;
                   },
                 ),
                 const SizedBox(height: 32), // Increase spacing
                 if (_isLoading)
                   const Center(child: CircularProgressIndicator())
                 else ...[
                   ElevatedButton(
                     onPressed: _handleSignIn,
                     style: ElevatedButton.styleFrom(
                        padding: const EdgeInsets.symmetric(vertical: 12), // Make buttons taller
                     ),
                     child: const Text('Login'),
                   ),
                   const SizedBox(height: 12), // Increase spacing
                   OutlinedButton( // Use OutlinedButton for Sign Up
                     onPressed: _handleSignUp,
                     style: OutlinedButton.styleFrom(
                        padding: const EdgeInsets.symmetric(vertical: 12), // Make buttons taller
                     ),
                     child: const Text('Sign Up'),
                   ),
                 ]
               ],
             ),
          )
        ),
      ),
    );
  }
} 