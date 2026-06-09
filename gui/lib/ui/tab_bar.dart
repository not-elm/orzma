import 'package:flutter/material.dart';
import '../session/mirror.dart';
import 'theme.dart';

/// A pane's surface tabs (always shown). The active tab gets an accent top-stripe.
class TabBarRow extends StatelessWidget {
  final MutablePane pane;
  const TabBarRow({super.key, required this.pane});

  @override
  Widget build(BuildContext context) => Container(
        height: OzTheme.tabBarHeight,
        color: OzTheme.chrome,
        child: Row(
          children: [
            for (final s in pane.surfaces)
              _tab(s.surface == pane.activeSurface, s.kind.label),
          ],
        ),
      );

  Widget _tab(bool on, String label) => Container(
        padding: const EdgeInsets.symmetric(horizontal: 8),
        decoration: BoxDecoration(
          color: on ? OzTheme.tabActiveBg : null,
          border: Border(
            top: BorderSide(
                color: on ? OzTheme.accent : Colors.transparent, width: 2),
            right: const BorderSide(color: OzTheme.border, width: 1),
          ),
        ),
        alignment: Alignment.center,
        child: Text(label,
            style: TextStyle(
                color: on ? OzTheme.text : OzTheme.muted,
                fontSize: 10,
                fontFamily: OzTheme.mono)),
      );
}
