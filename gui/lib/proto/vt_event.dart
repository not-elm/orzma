/// A per-surface VT control event (mirror of the Title/Cwd subset of `ozmux_vt::VtEvent`).
sealed class VtEvent {
  const VtEvent();
  static VtEvent fromJson(Object j) {
    if (j is String) return UnknownVtEvent(j);
    final m = j as Map<String, dynamic>;
    final key = m.keys.first;
    final v = m[key];
    switch (key) {
      case 'TitleChanged':
        // NOTE: TitleChanged carries Option<String>; null is an OSC 2 title reset.
        return TitleChanged(v as String?);
      case 'CurrentDir':
        return CurrentDir(v as String);
      default:
        return UnknownVtEvent(key);
    }
  }
}

/// The window title changed (`null` = reset).
class TitleChanged extends VtEvent { final String? title; const TitleChanged(this.title); }
/// The current working directory changed (OSC 7).
class CurrentDir extends VtEvent { final String path; const CurrentDir(this.path); }
/// An unhandled VT event (Bell/Clipboard/Mode/ChildExit etc.).
class UnknownVtEvent extends VtEvent { final String tag; const UnknownVtEvent(this.tag); }
