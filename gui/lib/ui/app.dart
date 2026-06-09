import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import '../session/mirror.dart';
import '../session/session.dart';
import 'status_strip.dart';
import 'theme.dart';
import 'workspace_view.dart';

/// The app shell: routes hardware key chords to layout commands and renders the
/// active workspace above a bottom status strip.
class OzmuxHome extends StatefulWidget {
  final Session session;
  const OzmuxHome({super.key, required this.session});
  @override
  State<OzmuxHome> createState() => _OzmuxHomeState();
}

class _OzmuxHomeState extends State<OzmuxHome> {
  final FocusNode _focus = FocusNode();

  @override
  void initState() {
    super.initState();
    _focus.requestFocus();
  }

  @override
  void dispose() {
    _focus.dispose();
    super.dispose();
  }

  KeyEventResult _onKey(FocusNode node, KeyEvent event) {
    if (event is! KeyDownEvent) return KeyEventResult.ignored;
    final mods = HardwareKeyboard.instance.logicalKeysPressed;
    return widget.session.dispatchShortcut(event.logicalKey, mods)
        ? KeyEventResult.handled
        : KeyEventResult.ignored;
  }

  @override
  Widget build(BuildContext context) => Focus(
        focusNode: _focus,
        onKeyEvent: _onKey,
        child: ListenableBuilder(
          listenable: widget.session,
          builder: (context, _) {
            final st = widget.session.state;
            return Column(children: [
              Expanded(
                child: ColoredBox(
                  color: const Color(0xFF0E0F15),
                  child: st == null
                      ? const Center(
                          child: Text('connecting…',
                              style: TextStyle(color: OzTheme.muted, fontFamily: OzTheme.mono)))
                      : _workspace(st),
                ),
              ),
              StatusStrip(
                state: st == null ? ConnState.connecting : ConnState.connected,
                workspaces:
                    st?.workspaces.map((w) => w.name).toList() ?? const [],
                activeWorkspace: st == null ? 0 : _activeIndex(st),
              ),
            ]);
          },
        ),
      );

  Widget _workspace(SessionState st) {
    final ws = _active(st);
    if (ws == null) return const SizedBox.shrink();
    return Padding(
        padding: const EdgeInsets.all(4),
        child: WorkspaceView(workspace: ws));
  }

  MutableWorkspace? _active(SessionState st) {
    for (final w in st.workspaces) {
      if (w.workspace == st.activeWorkspace) return w;
    }
    return st.workspaces.isEmpty ? null : st.workspaces.first;
  }

  int _activeIndex(SessionState st) {
    for (int i = 0; i < st.workspaces.length; i++) {
      if (st.workspaces[i].workspace == st.activeWorkspace) return i;
    }
    return 0;
  }
}
