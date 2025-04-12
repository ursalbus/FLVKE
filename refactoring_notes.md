# Refactoring Notes & Code Observations

This file documents potential areas of redundancy or opportunities for further refinement identified during the refactoring of `client/lib/main.dart`.

1.  **Client-Side Bonding Curve Logic Duplication:**
    *   The Dart functions in `client/lib/utils/bonding_curve.dart` mirror bonding curve calculations that must also exist on the Rust server.
    *   This client-side logic is necessary for estimating trade costs before execution but requires careful synchronization if the server-side logic changes.

2.  **`handleServerMessage` Pattern Repetition:**
    *   State classes (`TimelineState`, `BalanceState`, `PositionState`) all use a similar `handleServerMessage` pattern (check type, update state, notify listeners).
    *   While clear, this is a repeated pattern. For larger apps, more abstract message dispatching might be considered.

3.  **State Comparison Helpers (`PositionState`):**
    *   `client/lib/state/position_state.dart` uses helper functions (`_mapEquals`, `_positionDetailEquals`) for state comparison.
    *   Improvement: Implement `operator ==` and `hashCode` directly in the data model classes (e.g., `PositionDetail`) for cleaner comparisons.

4.  **UI Element Styling (Minor):**
    *   Widgets like `PostWidget` have some inline button styling (e.g., enabled/disabled colors, padding).
    *   While `main.dart` defines base themes, further centralization using `ThemeData` could reduce widget-specific style overrides if desired for more consistency. 