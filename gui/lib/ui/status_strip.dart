import 'package:flutter/material.dart';
import 'theme.dart';

/// Connection state shown on the status strip.
enum ConnState { connecting, connected, error }

/// The bottom status strip: connection state (left) + workspace chips (right).
class StatusStrip extends StatelessWidget {
  final ConnState state;
  final String detail;
  final List<String> workspaces;
  final int activeWorkspace;
  const StatusStrip({
    super.key,
    required this.state,
    this.detail = '',
    this.workspaces = const [],
    this.activeWorkspace = 0,
  });

  @override
  Widget build(BuildContext context) {
    final (dot, color, label) = switch (state) {
      ConnState.connected => ('●', OzTheme.ok, 'connected'),
      ConnState.connecting => ('◌', OzTheme.warn, detail.isEmpty ? 'connecting…' : detail),
      ConnState.error => ('✕', OzTheme.err, detail.isEmpty ? 'error' : detail),
    };
    return Container(
      color: OzTheme.chrome,
      padding: const EdgeInsets.symmetric(horizontal: 10, vertical: 4),
      child: Row(children: [
        Text('$dot ', style: TextStyle(color: color, fontFamily: OzTheme.mono, fontSize: 11)),
        Text(label, style: const TextStyle(color: OzTheme.muted, fontFamily: OzTheme.mono, fontSize: 11)),
        const Spacer(),
        for (int i = 0; i < workspaces.length; i++) _chip(workspaces[i], i == activeWorkspace),
      ]),
    );
  }

  Widget _chip(String name, bool on) => Container(
        margin: const EdgeInsets.only(left: 6),
        padding: const EdgeInsets.symmetric(horizontal: 8, vertical: 1),
        decoration: BoxDecoration(
          color: on ? OzTheme.accent : null,
          borderRadius: BorderRadius.circular(9),
          border: Border.all(color: OzTheme.border),
        ),
        child: Text(name,
            style: TextStyle(
                color: on ? const Color(0xFF0E0F15) : OzTheme.muted,
                fontFamily: OzTheme.mono,
                fontSize: 10)),
      );
}
