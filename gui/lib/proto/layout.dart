import 'ids.dart';

/// Split axis for a layout split node.
enum SplitOrientation { horizontal, vertical }

/// What a surface renders (mirror of `ozmux_mux::SurfaceKind`).
sealed class SurfaceKind {
  const SurfaceKind();

  /// Parses the externally-tagged serde representation.
  static SurfaceKind fromJson(dynamic j) {
    if (j == 'Terminal') return const TerminalKind();
    final map = j as Map<String, dynamic>;
    if (map.containsKey('Extension')) {
      return ExtensionKind(map['Extension']['entry'] as String);
    }
    if (map.containsKey('Browser')) {
      final b = map['Browser'] as Map<String, dynamic>;
      return BrowserKind(b['initial_url'] as String?);
    }
    return const TerminalKind();
  }

  /// Short display label for this surface type.
  String get label;
}

/// A PTY-backed terminal surface.
class TerminalKind extends SurfaceKind {
  const TerminalKind();

  @override
  String get label => 'terminal';
}

/// An extension-rendered surface.
class ExtensionKind extends SurfaceKind {
  /// HTML entry point for the extension.
  final String entry;

  const ExtensionKind(this.entry);

  @override
  String get label => 'extension';
}

/// An embedded-browser surface.
class BrowserKind extends SurfaceKind {
  /// Optional starting URL for the browser pane.
  final String? initialUrl;

  const BrowserKind(this.initialUrl);

  @override
  String get label => 'browser';
}

/// A binary layout tree node (mirror of `ozmux_mux::LayoutNode`).
sealed class LayoutNode {
  const LayoutNode();

  /// Parses the externally-tagged serde representation.
  static LayoutNode fromJson(Map<String, dynamic> j) {
    if (j.containsKey('Split')) {
      final s = j['Split'] as Map<String, dynamic>;
      return LayoutSplit(
        id: SplitId.fromJson(s['id'] as Map<String, dynamic>),
        orientation: s['orientation'] == 'Horizontal'
            ? SplitOrientation.horizontal
            : SplitOrientation.vertical,
        ratio: (s['ratio'] as num).toDouble(),
        first: LayoutNode.fromJson(s['first'] as Map<String, dynamic>),
        second: LayoutNode.fromJson(s['second'] as Map<String, dynamic>),
      );
    }
    final p = j['Pane'] as Map<String, dynamic>;
    return LayoutPane(
      id: PaneId.fromJson(p['id'] as Map<String, dynamic>),
      surfaceKind: SurfaceKind.fromJson(p['surface_kind']),
    );
  }
}

/// An internal split node with two children weighted by `ratio` (first child fraction).
class LayoutSplit extends LayoutNode {
  /// Server-issued id for this split node.
  final SplitId id;

  /// Axis along which this node splits its children.
  final SplitOrientation orientation;

  /// Fraction of space allocated to `first` (0.0–1.0).
  final double ratio;

  /// Left / top child.
  final LayoutNode first;

  /// Right / bottom child.
  final LayoutNode second;

  const LayoutSplit({
    required this.id,
    required this.orientation,
    required this.ratio,
    required this.first,
    required this.second,
  });
}

/// A leaf pane node hosting a surface of `surfaceKind`.
class LayoutPane extends LayoutNode {
  /// Server-issued id for this pane.
  final PaneId id;

  /// The kind of surface this pane renders.
  final SurfaceKind surfaceKind;

  const LayoutPane({required this.id, required this.surfaceKind});
}

/// A layout node address (mirror of `ozmux_mux::NodeId`).
sealed class NodeId {
  const NodeId();

  /// Parses the externally-tagged serde representation.
  static NodeId fromJson(Map<String, dynamic> j) =>
      j.containsKey('Split')
          ? NodeSplit(SplitId.fromJson(j['Split'] as Map<String, dynamic>))
          : NodePane(PaneId.fromJson(j['Pane'] as Map<String, dynamic>));
}

/// A split-node address.
class NodeSplit extends NodeId {
  /// The referenced split's id.
  final SplitId id;

  const NodeSplit(this.id);

  @override
  bool operator ==(Object other) => other is NodeSplit && other.id == id;

  @override
  int get hashCode => Object.hash('split', id);
}

/// A pane-node address.
class NodePane extends NodeId {
  /// The referenced pane's id.
  final PaneId id;

  const NodePane(this.id);

  @override
  bool operator ==(Object other) => other is NodePane && other.id == id;

  @override
  int get hashCode => Object.hash('pane', id);
}
