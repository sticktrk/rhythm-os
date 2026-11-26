import 'package:flutter_test/flutter_test.dart';
import 'package:rhythm_app/services/customer_assistant_service.dart';
import 'package:rhythm_app/services/home_agent_catalog.dart';
import 'package:rhythm_app/services/home_agent_client.dart';
import 'package:rhythm_app/services/lighting_chat_service.dart';

void main() {
  const plan = '18371ef9-7e58-4b02-98d9-876ea6e99aa5';
  const tools = {
    'read_automation_settings',
    'plan_scene_edit',
    'apply_scene_edit',
    'plan_home_scene_edit',
    'apply_home_scene_edit'
  };
  const palette = {
    'color_1': '#A31513',
    'color_2': '#00bc01',
    'brightness_percent': 52,
  };
  late HomeAgentClient client;
  late List<String> calls;
  CustomerAction? definition, saved, hubProposal;
  late CustomerAction before;
  setUp(() {
    client = HomeAgentClient(Uri.https('fixture.invalid'), 'home', 'fixture',
        () => 'token', () => true)
      ..session = 'session';
    calls = [];
    definition = null;
    saved = null;
    before = {'current': null};
    hubProposal = null;
  });
  tearDown(() => client.close());

  Future<HomeAgentCatalog> load([Set<String> available = tools]) =>
      HomeAgentCatalog.load(
          LightingChatHub([], {}, (op, args) async {
            calls.add(op);
            if (op == 'read_automation_settings') {
              return {
                'scenes': [if (saved != null) saved]
              };
            }
            if (op == 'plan_scene_edit') {
              definition = args['definition'];
              return {
                'plan_id': 'fresh-hub-plan',
                'server_instance_id': 'fixture',
                'before': before,
                'proposal': {
                  'scene': {
                    'id': args['scene_id'],
                    'definition': hubProposal ?? definition,
                  },
                },
                'affected_rooms': [],
              };
            }
            if (op == 'plan_home_scene_edit') {
              return {
                'plan_id': 'apply-plan',
                'server_instance_id': 'fixture',
                'before': before,
                'affected_rooms': [],
              };
            }
            throw StateError('Unexpected hub operation $op');
          }, () {}, catalog: available),
          client);

  Future<AgentReview> review(HomeAgentCatalog catalog,
          [CustomerAction args = palette, String id = plan]) =>
      catalog.review({
        'target': catalog.generatedSceneTarget!.ref,
        'operation': 'plan_generated_home_scene',
        'arguments': args,
      }, 'fixture', id);

  test('a draft target requires the entire hub workflow and explicit scope',
      () async {
    for (final missing in tools) {
      final catalog = await load(tools.difference({missing}));
      expect(catalog.canCreateHomeScene, isFalse, reason: missing);
    }
    final catalog = await load();
    expect(catalog.canCreateHomeScene, isTrue);
    expect(catalog.generatedSceneTarget!.kind, 'd');
    expect(catalog.generatedSceneTarget!.tools, {'plan_generated_home_scene'});
    expect(
        await catalog.context('Set lights in the whole house', null), isEmpty);
    expect(catalog.targets, isEmpty);
  });

  test('a palette is reviewed against an empty hub slot before saving',
      () async {
    final catalog = await load();
    final preview = await review(catalog);
    expect(calls, ['read_automation_settings', 'plan_scene_edit']);
    expect(preview.title, 'Red & Green');
    expect(preview.detail, contains('52%'));
    expect(preview.detail, contains('#A31513'));
    expect(preview.operation, 'apply_scene_edit');
    expect(preview.arguments, {'plan_id': 'fresh-hub-plan', 'confirmed': true});
    expect(definition!['id'], 'chat-$plan');
    expect(definition!['light']['palette_mode'], 'cycle');
    final four = {...palette, 'color_3': '#ffffff', 'color_4': '#0000ff'};
    await review(catalog, four);
    expect(definition!['light']['palette'], hasLength(4));
    expect(calls.where((op) => op.startsWith('apply_')), isEmpty);
    for (final evidence in <CustomerAction>[
      {},
      {
        'current': {'id': 'chat-$plan'}
      }
    ]) {
      before = evidence;
      await expectLater(review(catalog), throwsFormatException);
    }
    before = {'current': null};
    hubProposal = {};
    await expectLater(review(catalog), throwsFormatException);
  });

  test('Rust scene serialization matches both reviews and catalog readback',
      () async {
    // Rust SceneDefinition and LightSceneLayer omit absent optional fields.
    // This fixture is independent of the SDK's outgoing scene definition.
    const serialized = {
      'id': 'chat-$plan',
      'name': 'Red & Green',
      'source': {'kind': 'user'},
      'light': {
        'palette': [
          {
            'power': 'on',
            'brightness': 52,
            'color': {
              'kind': 'rgb',
              'rgb': {'r': 163, 'g': 21, 'b': 19}
            },
          },
          {
            'power': 'on',
            'brightness': 52,
            'color': {
              'kind': 'rgb',
              'rgb': {'r': 0, 'g': 188, 'b': 1}
            },
          },
        ],
        'palette_mode': 'cycle',
        'entries': [],
      },
      'extensions': {'whole_home': true},
    };
    hubProposal = serialized;
    expect((await review(await load())).operation, 'apply_scene_edit');
    saved = serialized;
    final refreshed = await load();
    final target = refreshed.savedGeneratedScene(plan, palette)!;
    before = {'current': serialized, 'room_ids': [], 'bound_nodes': {}};
    final apply = await refreshed.review({
      'target': target.ref,
      'operation': 'plan_home_scene_edit',
      'arguments': {},
    }, 'fixture', 'next');
    expect(apply.operation, 'apply_home_scene_edit');
  });

  test('malformed or unbounded model arguments never request a hub plan',
      () async {
    final catalog = await load();
    for (final args in <CustomerAction>[
      {...palette}..remove('color_2'),
      {...palette, 'color_4': '#fff000'},
      {...palette, 'color_3': 'red'},
      {...palette, 'color_2': '#00000000'},
      {...palette, 'brightness_percent': 0},
      {...palette, 'brightness_percent': 101},
      {...palette, 'scene_id': 'existing'},
    ]) {
      await expectLater(review(catalog, args), throwsFormatException);
    }
    await expectLater(
        review(catalog, palette, 'existing'), throwsFormatException);
    expect(calls, ['read_automation_settings']);
  });

  test('home apply uses the exact newly saved palette and a fresh hub preview',
      () async {
    final initial = await load();
    await review(initial);
    expect(initial.savedGeneratedScene(plan, palette), isNull);
    for (final changed in <CustomerAction>[
      {...definition!, 'id': 'another-scene'},
      {
        ...definition!,
        'source': {'kind': 'imported', 'provider': 'hue'}
      },
      {
        ...definition!,
        'extensions': {'whole_home': false}
      },
      {
        ...definition!,
        'light': {...definition!['light'], 'palette_mode': 'spread'}
      },
      {...definition!, 'name': 'Edited scene'},
    ]) {
      saved = changed;
      expect((await load()).savedGeneratedScene(plan, palette), isNull);
    }
    saved = definition;
    final refreshed = await load();
    expect(
        refreshed
            .savedGeneratedScene(plan, {...palette, 'brightness_percent': 51}),
        isNull);
    final target = refreshed.savedGeneratedScene(plan, palette)!;
    expect(target.ref, client.reference('c', 'chat-$plan'));
    final apply = {
      'target': target.ref,
      'operation': 'plan_home_scene_edit',
      'arguments': {}
    };
    before = {
      'current': {...definition!, 'name': 'Changed after refresh'}
    };
    await expectLater(
        refreshed.review(apply, 'fixture', 'next'), throwsFormatException);
    before = {'current': definition};
    final preview = await refreshed.review(apply, 'fixture', 'next');
    expect(preview.arguments, {'plan_id': 'apply-plan', 'confirmed': true});
    expect(calls.where((op) => op.startsWith('apply_')), isEmpty);
  });
}
