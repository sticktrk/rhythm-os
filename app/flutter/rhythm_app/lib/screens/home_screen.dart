import 'package:flutter/material.dart';

/// Landing page placeholder.
class HomeScreen extends StatelessWidget {
  const HomeScreen({super.key});

  @override
  Widget build(BuildContext context) {
    return Scaffold(
      appBar: AppBar(
        title: const Text('Rhythm Lighting'),
        backgroundColor: const Color(0xFF16213E),
      ),
      body: const Center(
        child: Text(
          'Home',
          style: TextStyle(color: Colors.white54),
        ),
      ),
    );
  }
}
