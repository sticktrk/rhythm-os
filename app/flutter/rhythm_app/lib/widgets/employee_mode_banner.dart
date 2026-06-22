import 'package:flutter/material.dart';

class EmployeeModeBanner extends StatelessWidget {
  const EmployeeModeBanner({
    super.key,
    required this.label,
    required this.onExit,
    this.closing = false,
  });

  static const _supportBlue = Color(0xFF4CC9F0);

  final String label;
  final VoidCallback onExit;
  final bool closing;

  @override
  Widget build(BuildContext context) {
    final topInset = MediaQuery.of(context).padding.top;
    return Material(
      color: Colors.transparent,
      child: Container(
        padding: EdgeInsets.fromLTRB(16, topInset + 6, 8, 6),
        decoration: BoxDecoration(
          color: const Color(0xFF101827),
          border: Border(
            bottom: BorderSide(
              color: _supportBlue.withValues(alpha: 0.36),
              width: 1,
            ),
          ),
          boxShadow: [
            BoxShadow(
              color: _supportBlue.withValues(alpha: 0.10),
              blurRadius: 14,
              offset: const Offset(0, 2),
            ),
          ],
        ),
        child: Row(
          children: [
            const Icon(
              Icons.admin_panel_settings_rounded,
              size: 17,
              color: _supportBlue,
            ),
            const SizedBox(width: 10),
            Expanded(
              child: Text(
                'EMPLOYEE SUPPORT - $label',
                overflow: TextOverflow.ellipsis,
                style: const TextStyle(
                  color: Color(0xFFE6EDF3),
                  fontSize: 11,
                  fontWeight: FontWeight.w800,
                  letterSpacing: 1.4,
                ),
              ),
            ),
            TextButton.icon(
              onPressed: closing ? null : onExit,
              style: TextButton.styleFrom(
                foregroundColor: _supportBlue,
                padding:
                    const EdgeInsets.symmetric(horizontal: 12, vertical: 6),
                minimumSize: const Size(0, 32),
                tapTargetSize: MaterialTapTargetSize.shrinkWrap,
                shape: RoundedRectangleBorder(
                  borderRadius: BorderRadius.circular(8),
                ),
              ),
              icon: closing
                  ? const SizedBox(
                      width: 14,
                      height: 14,
                      child: CircularProgressIndicator(strokeWidth: 2),
                    )
                  : const Icon(Icons.close_rounded, size: 16),
              label: Text(
                closing ? 'Closing' : 'Close',
                style: const TextStyle(
                  fontSize: 13,
                  fontWeight: FontWeight.w600,
                  letterSpacing: 0,
                ),
              ),
            ),
          ],
        ),
      ),
    );
  }
}
