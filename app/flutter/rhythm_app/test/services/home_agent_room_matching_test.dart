import 'dart:convert';

import 'package:flutter_test/flutter_test.dart';
import 'package:rhythm_app/services/home_agent_catalog.dart';
import 'package:rhythm_app/services/home_agent_client.dart';
import 'package:rhythm_app/services/lighting_chat_service.dart';
import 'package:rhythm_sdk/rhythm_sdk.dart';

void main() {
  late HomeAgentClient client;
  setUp(() {
    client = HomeAgentClient(Uri.https('fixture.invalid'), 'home', 'fixture',
        () => 'token', () => true)
      ..session = 'session';
  });
  tearDown(() => client.close());

  Future<HomeAgentCatalog> load(Map<String, String> names) =>
      HomeAgentCatalog.load(
          LightingChatHub([
            for (final entry in names.entries)
              RhythmTopologyNode(
                  id: entry.key, name: entry.value, kind: RhythmNodeKind.room)
          ], {
            'adjust_room_brightness'
          }, (op, args) async {
            expect(op, 'explain_room_output');
            return {'room_id': args['room_id']};
          }, () {}),
          client);

  test('the semantic query uses complete words and prefers full names',
      () async {
    final catalog = await load({
      'secret-study': 'Upstairs Kitchen',
      'secret-kitchen': 'Downstairs Kitchen',
      'secret-a': 'Sun-Room',
      'secret-b': 'Kitchenette',
      'secret-c': 'Den',
      'secret-d': 'The Den',
      'secret-e': 'Guest Den',
    });
    expect(catalog.roomsMatching('kItCheN').map((r) => r.name),
        ['Downstairs Kitchen', 'Upstairs Kitchen']);
    expect(catalog.roomsMatching('the Sun Room').single.name, 'Sun-Room');
    expect(catalog.roomsMatching('the den').single.name, 'The Den');
    expect(catalog.roomsMatching('my den').single.name, 'Den');
    for (final noMatch in ['kitch', 'secret-a', 'study', 'room', '']) {
      expect(catalog.roomsMatching(noMatch), isEmpty, reason: noMatch);
    }
  });

  test('duplicate labels survive reordering and keep real numbered names',
      () async {
    final names = {
      'secret-z': 'Office',
      'secret-a': 'Office',
      'secret-b': 'Office 1'
    };
    final first = await load(names);
    final second = await load(Map.fromEntries(names.entries.toList().reversed));
    for (final catalog in [first, second]) {
      final choices = catalog.ambiguousRooms('Dim the Office');
      expect(choices.map((room) => room.id), ['secret-a', 'secret-z']);
      expect(choices.map(catalog.targetLabel), ['Office (2)', 'Office (3)']);
      expect(catalog.roomAnswer('Office', choices), isNull);
      expect(catalog.roomAnswer('the second one please', choices), choices[1]);
      expect(catalog.roomAnswer('option 1', choices), choices[0]);
      expect(catalog.roomAnswer('office (3)', choices), choices[1]);
      expect(catalog.roomAnswer('dim option 2', choices), isNull);
      expect(catalog.roomAnswer('3', choices), isNull);
      expect(catalog.ambiguousRooms('Dim the Office (3)'), isEmpty);
      expect(catalog.redact('Dim Office (3)'), 'Dim ${choices[1].ref}');
      expect(catalog.redact('Dim Office'), 'Dim [ambiguous target]');
      expect(catalog.redact('Dim Office', choices[0].ref),
          'Dim ${choices[0].ref}');
      expect(catalog.roomsMatching('office 1').single.id, 'secret-b');
      expect(
          catalog.hub.mentionedRooms('Dim Office (2)').single.id, 'secret-a');
      expect(catalog.hub.mentionedRooms('Dim Office 2%').map((r) => r.id),
          unorderedEquals(['secret-a', 'secret-z']));
      expect(LightingChatHub.roomAnswerNumber('Option 8, please'), 8);
      expect(LightingChatHub.roomAnswerNumber('the third one'), 3);
      expect(LightingChatHub.roomAnswerNumber('Dim Office (2)'), isNull);
    }
  });

  test('typed answers preserve a room name starting with an article', () async {
    final catalog = await load({'a': 'Den', 'b': 'The Den'});
    final choices = catalog.targets.values.toList();
    expect(catalog.roomAnswer('The Den', choices)?.id, 'b');
    expect(catalog.roomAnswer('in the den please', choices)?.id, 'b');
    expect(catalog.roomAnswer('Den', choices)?.id, 'a');
    expect(catalog.roomAnswer('The Den', [choices.first]), choices.first);
  });

  test('clarifying one room keeps the other ambiguous names in the request',
      () async {
    final catalog =
        await load({'b': 'Office', 'a': 'Office', 'c': 'Hall', 'd': 'Hall'});
    expect(catalog.rooms.map((t) => t.id), ['c', 'd', 'a', 'b']);
    const prompt = 'Dim OFFICE and Hall';
    final office = catalog.ambiguousRooms(prompt).last;
    final partial = catalog.resolveRoomMention(prompt, office);
    expect(partial, 'Dim ${office.ref} and Hall');
    final hall = catalog.ambiguousRooms(partial).first;
    final resolved = catalog.resolveRoomMention(partial, hall);
    expect(catalog.redact(resolved), 'Dim ${office.ref} and ${hall.ref}');
    expect(catalog.ambiguousRooms(resolved), isEmpty);
  });

  test('scope changes and exclusions override the last room', () async {
    final catalog = await load({'secret-a': 'Studio', 'secret-b': 'Hall'});
    final studio = catalog.roomsMatching('studio').single;
    final hall = catalog.roomsMatching('hall').single;
    Future<List<String>> included(String text) async {
      final context = await catalog.context(catalog.redact(text), studio.ref);
      expect(jsonEncode(context), isNot(matches('secret-|Studio|Hall')));
      return [
        for (final t in context)
          if (t['included'] == true) t['ref'] as String
      ];
    }

    expect(await included('Leave it a bit warmer'), [studio.ref]);
    expect(await included('Dim the whole house except Studio'), [hall.ref]);
    expect(await included('Dim everywhere but Studio'), [hall.ref]);
    expect(await included('All rooms except the Sunroom'), isEmpty);
    expect(
        await included('The whole house but leave the Sunroom alone'), isEmpty);
    expect(
        LightingChatHub.requestsSceneDesign(
            'Create a scene for the whole house except the nursery'),
        isFalse);
    expect(await included('Dim throughout our home'),
        unorderedEquals([studio.ref, hall.ref]));
  });

  test('whole-home requests and standalone whole-home replies', () {
    for (final text in [
      'Dim everywhere',
      'Create a house-wide vibe',
      'Dim throughout the apartment',
      'Set warm lights across my home',
      'Turn up the whole house',
      'Everywhere in our home'
    ]) {
      expect(LightingChatHub.refersToWholeHome(text), isTrue, reason: text);
    }
    for (final text in ['The whole home', 'All rooms please', 'everywhere']) {
      expect(LightingChatHub.isWholeHomeAnswer(text), isTrue, reason: text);
    }
    for (final text in [
      'all lights in the kitchen',
      'everywhere in the kitchen',
      'all rooms on this floor'
    ]) {
      expect(LightingChatHub.refersToWholeHome(text), isFalse, reason: text);
    }
    expect(LightingChatHub.isWholeHomeAnswer('Dim the whole house'), isFalse);
    expect(LightingChatHub.isWholeHomeAnswer('Everywhere except the study'),
        isFalse);
  });
}
