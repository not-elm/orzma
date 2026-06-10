import 'package:flutter/material.dart';
import 'daemon/launcher.dart';
import 'daemon/socket_path.dart';
import 'session/session.dart';
import 'ui/app.dart';
import 'ui/theme.dart';

Future<void> main() async {
  WidgetsFlutterBinding.ensureInitialized();
  runApp(const OzmuxApp());
}

/// Root app: boots the daemon connection, then renders the multiplexer layout.
class OzmuxApp extends StatefulWidget {
  const OzmuxApp({super.key});
  @override
  State<OzmuxApp> createState() => _OzmuxAppState();
}

class _OzmuxAppState extends State<OzmuxApp> {
  Session? _session;
  String _status = 'starting daemon…';
  bool _error = false;

  @override
  void initState() {
    super.initState();
    _boot();
  }

  Future<void> _boot() async {
    try {
      final conn =
          await DaemonLauncher(socketPath: ozmuxSocketPath()).ensureRunning();
      if (!mounted) {
        await conn.close();
        return;
      }
      setState(() => _session = Session.fromConnection(conn));
    } catch (e) {
      if (!mounted) return;
      setState(() {
        _error = true;
        _status = 'daemon error: $e';
      });
    }
  }

  @override
  void dispose() {
    _session?.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) => MaterialApp(
        title: 'ozmux',
        debugShowCheckedModeBanner: false,
        theme: ThemeData.dark(useMaterial3: true),
        home: Scaffold(
          backgroundColor: const Color(0xFF0E0F15),
          body: _session == null
              ? Center(
                  child: Text(_status,
                      style: TextStyle(
                          color: _error ? OzTheme.err : OzTheme.muted,
                          fontFamily: OzTheme.mono)))
              : OzmuxHome(session: _session!),
        ),
      );
}
