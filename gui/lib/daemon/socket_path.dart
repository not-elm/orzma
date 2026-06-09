import 'dart:io';

/// The ozmux daemon socket path — mirror of `ozmux_server::socket_path`
/// (`${TMPDIR:-/tmp}/ozmux-<uid>/default.sock`). The usable `sun_path` length is
/// 103 (the 104-byte slot holds a trailing NUL); `$TMPDIR` must be absolute (an
/// empty/relative value falls back to /tmp). Must match the Rust formula exactly.
String ozmuxSocketPath() {
  final uid = _uid();
  final tmp = Platform.environment['TMPDIR'];
  if (tmp != null && tmp.startsWith('/')) {
    final p = '${_stripTrailingSlash(tmp)}/ozmux-$uid/default.sock';
    if (p.length <= 103) return p;
  }
  return '/tmp/ozmux-$uid/default.sock';
}

int _uid() => int.parse(Process.runSync('id', ['-u']).stdout.toString().trim());

String _stripTrailingSlash(String s) =>
    s.endsWith('/') ? s.substring(0, s.length - 1) : s;
