import 'package:flutter/material.dart';
import '../layout/geometry.dart';
import '../proto/ids.dart';
import '../proto/layout.dart';
import '../session/mirror.dart';
import 'pane_view.dart';
import 'theme.dart';

/// Renders a workspace's layout tree as nested Row/Column splits with pane leaves.
class WorkspaceView extends StatelessWidget {
  final MutableWorkspace workspace;
  const WorkspaceView({super.key, required this.workspace});

  @override
  Widget build(BuildContext context) => _node(workspace.layout);

  Widget _node(LayoutNode n) {
    if (n is LayoutSplit) {
      final (wa, wb) = flexWeights(n.ratio);
      final children = <Widget>[
        Expanded(flex: wa, child: _node(n.first)),
        const SizedBox(width: OzTheme.gutter, height: OzTheme.gutter),
        Expanded(flex: wb, child: _node(n.second)),
      ];
      return n.orientation == SplitOrientation.horizontal
          ? Row(children: children)
          : Column(children: children);
    }
    final pane = n as LayoutPane;
    return PaneView(
        pane: _findPane(pane.id), active: pane.id == workspace.activePane);
  }

  MutablePane? _findPane(PaneId id) {
    for (final p in workspace.panes) {
      if (p.pane == id) return p;
    }
    return null;
  }
}
