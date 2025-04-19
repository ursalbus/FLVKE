import 'package:flutter/material.dart';

class ThemeState extends ChangeNotifier {
  ThemeMode _themeMode = ThemeMode.light; // Default to light theme

  ThemeMode get themeMode => _themeMode;

  bool get isDarkMode => _themeMode == ThemeMode.dark;

  void toggleTheme() {
    _themeMode = isDarkMode ? ThemeMode.light : ThemeMode.dark;
    print("Theme toggled to: ${_themeMode.name}");
    notifyListeners();
  }
} 