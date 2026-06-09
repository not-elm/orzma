import 'package:flutter/material.dart';
import '../session/mirror.dart';
import 'tab_bar.dart';
import 'theme.dart';

/// One pane: bordered (accent when active), an always-on tab bar, and a
/// placeholder body where terminal content will later render.
class PaneView extends StatelessWidget {
  final MutablePane? pane;
  final bool active;
  const PaneView({super.key, required this.pane, required this.active});

  @override
  Widget build(BuildContext context) {
    final p = pane;
    final body = Container(
      decoration: BoxDecoration(
        color: OzTheme.paneBg,
        border: Border.all(
            color: active ? OzTheme.accent : OzTheme.border,
            width: OzTheme.paneBorderWidth),
        borderRadius: BorderRadius.circular(4),
      ),
      clipBehavior: Clip.antiAlias,
      child: p == null
          ? const SizedBox.shrink()
          : Column(children: [
              TabBarRow(pane: p),
              Expanded(child: _placeholder(p)),
            ]),
    );
    return active
        ? body
        : Opacity(opacity: OzTheme.inactiveOpacity, child: body);
  }

  Widget _placeholder(MutablePane p) {
    final s = _activeSurface(p);
    return Center(
      child: Column(mainAxisSize: MainAxisSize.min, children: [
        Text('● ${s?.kind.label ?? 'terminal'}',
            style: const TextStyle(
                color: OzTheme.muted, fontSize: 12, fontFamily: OzTheme.mono)),
        if (s != null)
          Text(s.cwd,
              style: const TextStyle(
                  color: OzTheme.muted, fontSize: 10, fontFamily: OzTheme.mono)),
      ]),
    );
  }

  MutableSurface? _activeSurface(MutablePane p) {
    for (final s in p.surfaces) {
      if (s.surface == p.activeSurface) return s;
    }
    return p.surfaces.isNotEmpty ? p.surfaces.first : null;
  }
}
