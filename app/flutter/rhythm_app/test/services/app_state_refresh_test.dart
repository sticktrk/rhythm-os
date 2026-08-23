import 'dart:async';

import 'package:flutter_test/flutter_test.dart';
import 'package:rhythm_app/services/app_state_refresh.dart';

void main() {
  test(
    'ignores a cloud layout restore that completes for a stale scope',
    () async {
      final applyCompleter = Completer<bool>();
      var currentScope = 'scope-a';

      final restore = AppStateRefresh.completeCloudLayoutRestoreForTesting(
        apply: () => applyCompleter.future,
        isCurrentScope: () => currentScope == 'scope-a',
      );

      currentScope = 'scope-b';
      applyCompleter.complete(true);

      expect(await restore, isFalse);
    },
  );
}
