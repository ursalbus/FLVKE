import 'package:flutter/material.dart';

// Define light theme
final ThemeData lightTheme = ThemeData(
  brightness: Brightness.light,
  primarySwatch: Colors.deepPurple, // Example primary color
  scaffoldBackgroundColor: Colors.grey[100], // Light background
  cardColor: Colors.white,
  appBarTheme: AppBarTheme(
    backgroundColor: Colors.deepPurple[400],
    foregroundColor: Colors.white, // Title/icon color
  ),
  buttonTheme: ButtonThemeData(
     buttonColor: Colors.deepPurple[300],
     textTheme: ButtonTextTheme.primary,
  ),
   elevatedButtonTheme: ElevatedButtonThemeData(
      style: ElevatedButton.styleFrom(
         backgroundColor: Colors.deepPurple[300], // Button background
         foregroundColor: Colors.white, // Button text
      ),
   ),
   // Define other properties like text themes if needed
);

// Define dark theme
final ThemeData darkTheme = ThemeData(
  brightness: Brightness.dark,
  primarySwatch: Colors.deepPurple, // Can use the same primary
  scaffoldBackgroundColor: Colors.grey[900], // Dark background
  cardColor: Colors.grey[850], // Slightly lighter card background
  appBarTheme: AppBarTheme(
    backgroundColor: Colors.grey[900],
    foregroundColor: Colors.white, // Title/icon color
  ),
  buttonTheme: ButtonThemeData(
     buttonColor: Colors.deepPurple[300], // Keep button color?
     textTheme: ButtonTextTheme.primary,
  ),
  elevatedButtonTheme: ElevatedButtonThemeData(
      style: ElevatedButton.styleFrom(
         backgroundColor: Colors.deepPurple[300],
         foregroundColor: Colors.white,
      ),
   ),
  textTheme: const TextTheme(
    bodyMedium: TextStyle(color: Colors.white70), // Default text color
    bodySmall: TextStyle(color: Colors.white60),
    titleMedium: TextStyle(color: Colors.white), // Example title color
  ),
  inputDecorationTheme: InputDecorationTheme(
    labelStyle: TextStyle(color: Colors.white70), // Input label color
    hintStyle: TextStyle(color: Colors.white54),
    border: OutlineInputBorder(borderSide: BorderSide(color: Colors.white38)),
    enabledBorder: OutlineInputBorder(borderSide: BorderSide(color: Colors.white38)),
    focusedBorder: OutlineInputBorder(borderSide: BorderSide(color: Colors.deepPurple[300]!)),
  ),
  dividerColor: Colors.white24,
  // Define other properties like text themes
); 