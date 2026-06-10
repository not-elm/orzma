import 'dart:async';
import 'dart:io';
import 'connection.dart';

/// Ensures the ozmux daemon is running and returns an attached connection.
/// A successful Unix-socket connect IS the liveness proof (the daemon accepts
/// only when bound); on the returned connection the cold-attach `Welcome`
/// arrives first.
class DaemonLauncher {
  /// Path of the Unix-domain socket to connect to.
  final String socketPath;

  /// Path to the ozmux binary.
  final String binaryPath;

  /// Creates a launcher for the given socket path, defaulting to the binary
  /// resolved by `_defaultBinary`.
  DaemonLauncher({required this.socketPath, String? binaryPath})
      : binaryPath = binaryPath ?? _defaultBinary();

  /// Connects to a live daemon, or spawns `ozmux run` (detached) and polls until
  /// it accepts (~3s), then returns the attached connection.
  Future<DaemonConnection> ensureRunning() async {
    final existing = await _tryConnect();
    if (existing != null) return existing;
    try {
      await Process.start(binaryPath, ['run'], mode: ProcessStartMode.detached);
    } on ProcessException catch (e) {
      throw StateError(
          'could not start the ozmux daemon binary "$binaryPath" ($e). '
          'Set OZMUX_BIN to the daemon binary path or add `ozmux` to PATH.');
    }
    final deadline = DateTime.now().add(const Duration(seconds: 3));
    while (DateTime.now().isBefore(deadline)) {
      await Future<void>.delayed(const Duration(milliseconds: 100));
      final c = await _tryConnect();
      if (c != null) return c;
    }
    throw StateError('ozmux daemon did not start at $socketPath');
  }

  Future<DaemonConnection?> _tryConnect() async {
    try {
      return await DaemonConnection.connect(socketPath);
    } on SocketException {
      return null;
    }
  }

  /// The daemon binary: `$OZMUX_BIN` if set, else `ozmux` (PATH / dev: set
  /// OZMUX_BIN to `<repo>/target/debug/ozmux`).
  static String _defaultBinary() {
    final env = Platform.environment['OZMUX_BIN'];
    return (env != null && env.isNotEmpty) ? env : 'ozmux';
  }
}
