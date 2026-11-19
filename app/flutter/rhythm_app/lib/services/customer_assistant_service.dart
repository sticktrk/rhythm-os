import 'package:rhythm_core/rhythm_core.dart';
import 'package:rhythm_sdk/rhythm_sdk.dart';
import '../backend/backend.dart';
import '../providers/home_provider.dart';
import '../providers/server_sync_provider.dart';
import 'cloud_home_join_service.dart';
import 'debug_bundle_submission_service.dart';
import 'remote_access_service.dart';
import 'server_endpoint_resolver.dart';
import 'support_access_service.dart';

typedef CustomerAction = Map<String, dynamic>;

bool _accountChange(CustomerAction action) =>
    action['change'] == 'remove_member' || action['change'] == 'revoke_support';

/// Account-bound transport. Only the claim/complete calls coordinate local
/// work. The owner key, signed LAN proof and diagnostic bytes stay with the
/// services that already handle them.
class CustomerAssistantService {
  CustomerAssistantService(this.post, this.isCurrentAccount,
      {this.accountChanges = const Stream.empty()});

  static CustomerAssistantService? signedIn() {
    if (!BackendProvider.isInitialized) return null;
    final auth = BackendProvider.instance.auth;
    if (auth is! SupabaseAuthBackend || !auth.isSignedIn || auth.isAnonymous) {
      return null;
    }
    final account = auth.currentUserId;
    if (account == null) return null;
    return CustomerAssistantService((body) async {
      final result =
          await auth.client.functions.invoke('customer-assistant', body: body);
      if (result.status != 200 || result.data is! Map) {
        throw StateError('Customer operation unavailable');
      }
      return CustomerAction.from(result.data as Map);
    }, () => auth.currentUserId == account && !auth.isAnonymous,
        accountChanges: auth.authStateChanges.map<void>((_) {}));
  }

  final Stream<void> accountChanges;
  final Future<CustomerAction> Function(CustomerAction) post;
  final bool Function() isCurrentAccount;

  Future<CustomerAction> _request(CustomerAction body) async {
    if (!isCurrentAccount()) throw StateError('Account changed');
    final response = await post(body);
    if (!isCurrentAccount() || response.containsKey('error')) {
      throw StateError('The account request could not be verified');
    }
    return response;
  }

  Future<RhythmAssistantContract> discover() async =>
      RhythmAssistantContract.fromJson(await _request({'contract': true}));

  Future<CustomerAction> call(String operation,
          [CustomerAction arguments = const {}]) =>
      _request({'operation': operation, 'arguments': arguments});

  static const _reviewFields = [
    'plan_id',
    'change',
    'home_id',
    'hub_id',
    'member_id',
    'server_instance_id',
    'report_summary',
    'include_bundle'
  ];

  /// A single-use lease prevents two phones or a stale conversation from
  /// executing a reviewed action twice, even after a lost response.
  Future<CustomerAction> confirm(
    CustomerAction review, {
    required Future<void> Function(CustomerAction) checkDevice,
    required Future<bool> Function(CustomerAction) executeDevice,
  }) async {
    final id = review['plan_id'];
    if (id is! String ||
        id.isEmpty ||
        !{'review', 'awaiting_device'}.contains(review['status'])) {
      throw StateError('Request a fresh review');
    }
    final result = review['status'] == 'review'
        ? await call('apply_customer_plan', {'plan_id': id, 'confirmed': true})
        : review;
    if (result['status'] != 'awaiting_device') return result;
    if (!_reviewFields.every((key) => result[key] == review[key])) {
      throw StateError('Device handoff changed');
    }
    await checkDevice(review);
    final claim = await _request({'device_claim': id});
    var acknowledged = false;
    try {
      if (claim['status'] == 'claimed' &&
          _reviewFields.every((key) => claim[key] == review[key])) {
        await checkDevice(claim);
        acknowledged = await executeDevice(claim);
      }
    } catch (_) {
      acknowledged = false;
    }
    // A lost completion receipt is ambiguous even when the device succeeded.
    // Do not send it from a new account or repeat the claimed action.
    try {
      final receipt = await _request({
        'device_complete': id,
        'outcome': acknowledged ? 'acknowledged' : 'uncertain'
      });
      if (acknowledged &&
          receipt['plan_id'] == id &&
          receipt['status'] ==
              (_accountChange(review) ? 'applied' : 'acknowledged')) {
        return {...receipt, 'change': review['change']};
      }
    } catch (_) {
      // The claim might have completed before the response was lost.
    }
    return {'status': 'uncertain', 'change': review['change'], 'plan_id': id};
  }
}

class ResidentAssistantDevice {
  ResidentAssistantDevice(this.homes, this.sync, this.isCurrentAccount);
  final HomeProvider homes;
  final ServerSyncProvider sync;
  final bool Function() isCurrentAccount;

  String? get connectedHomeId {
    final hub = homes.activeServerHub;
    if (hub == null ||
        !isCurrentAccount() ||
        !sync.connection.connected ||
        sync.connectedServerHub?.homeId != hub.homeId ||
        sync.connectedServerHub?.id != hub.id ||
        sync.connectedServerInstanceId == null) {
      return null;
    }
    return hub.homeId;
  }

  Hub _matched(CustomerAction args) {
    final hub = homes.activeServerHub;
    if (!isCurrentAccount() ||
        hub == null ||
        !sync.connection.connected ||
        sync.connectedServerHub?.id != hub.id ||
        sync.connectedServerHub?.homeId != hub.homeId ||
        sync.connectedServerInstanceId == null ||
        sync.connectedServerInstanceId != args['server_instance_id']) {
      throw StateError('Connect to the reviewed Box on this device');
    }
    if (args['change'] != 'join_home' && !_sameHub(hub, args)) {
      throw StateError('Select the reviewed Home and Box');
    }
    return hub;
  }

  bool _sameHub(Hub hub, CustomerAction a) {
    final home = a['home_id'], cloudId = a['hub_id'];
    return home is String &&
        cloudId is String &&
        hub.homeId == home &&
        (hub.id == cloudId ||
            hub.id == homeScopedServerHubId(homeId: home, cloudHubId: cloudId));
  }

  /// Select an exact saved account hub. The normal connection provider holds
  /// its key and produces a fresh authoritative hello before hub tools open.
  Future<void> select(CustomerAction handoff) async {
    if (!isCurrentAccount() ||
        handoff['status'] != 'handoff' ||
        handoff['actor'] != 'resident_app' ||
        handoff['server_instance_id'] is! String) {
      throw StateError('Hub selection requires a fresh handoff');
    }
    Hub? find() {
      final matches = homes.homeServerHubSnapshots
          .expand((s) => s.serverHubs)
          .where((h) =>
              _sameHub(h, handoff) &&
              (h.serverInstanceId == null ||
                  h.serverInstanceId == handoff['server_instance_id']))
          .toList();
      return matches.length == 1 ? matches.single : null;
    }

    var hub = find();
    if (hub == null) {
      final entries = await homes.loadAccountHomes();
      final selected = entries
          .where((e) =>
              e.home.id == handoff['home_id'] &&
              e.serverHubs.any((h) =>
                  h.id == handoff['hub_id'] &&
                  h.serverInstanceId == handoff['server_instance_id']))
          .single;
      if (!isCurrentAccount()) throw StateError('Account changed');
      await homes.enterHome(selected);
      hub = find();
    }
    if (!isCurrentAccount() || hub == null || hub.token?.isNotEmpty != true) {
      throw StateError('Pair this phone with the selected Box');
    }
    homes.switchHome(hub.homeId);
    if (await homes.activateServerHub(hub) == null) {
      throw StateError('Hub unavailable');
    }
    sync.connectIfAvailable();
    if (!await sync.refreshForHomeEntry(homeName: homes.currentHome!.name)) {
      throw StateError('Could not reach the selected Box');
    }
    _matched(handoff);
  }

  Future<RhythmCloudJoinProof> _proof(Hub hub, String instance) async {
    final proof =
        await RhythmAuthApi(baseUrl: hub.endpoint.baseUrl, authToken: hub.token)
            .createCloudJoinProof()
            .timeout(const Duration(seconds: 5));
    if (!isCurrentAccount() ||
        proof.serverInstanceId != instance ||
        sync.connectedServerInstanceId != instance ||
        homes.activeServerHub?.id != hub.id) {
      throw StateError('The local Box changed');
    }
    return proof;
  }

  Future<CustomerAction> joinTarget() async {
    final instance = sync.connectedServerInstanceId;
    final hub =
        _matched({'change': 'join_home', 'server_instance_id': instance});
    final proof = await _proof(hub, instance!);
    return {
      'home_id': proof.homeId,
      'hub_id': proof.hubId,
      'server_instance_id': instance
    };
  }

  Future<ResolvedServerEndpoint> _grantEndpoint(
      Hub hub, CustomerAction review) async {
    final endpoint =
        await ServerEndpointResolver.resolve(hub, syncProvider: sync);
    if (endpoint.isRemote || endpoint.endpoint == hub.remoteEndpoint) {
      throw StateError(
          'Join the reviewed Box’s local network to grant support access.');
    }
    final hello =
        await RhythmConfigApi(baseUrl: endpoint.baseUrl, authToken: hub.token)
            .getState();
    _matched(review);
    if (hello.serverInstanceId != review['server_instance_id']) {
      throw StateError('The LAN endpoint identifies a different Box.');
    }
    return endpoint;
  }

  Future<void> check(CustomerAction review) async {
    if (!isCurrentAccount()) throw StateError('Account changed');
    if (_accountChange(review) || _textOnlyReport(review)) return;
    final hub = _matched(review);
    if (review['change'] == 'join_home') {
      final target = await joinTarget();
      if (target.keys
          .any((key) => review[key] != null && review[key] != target[key])) {
        throw StateError('The LAN proof identifies a different Home');
      }
    } else if (hub.token?.isNotEmpty != true) {
      throw StateError('Pair this app with the reviewed Box');
    } else if (review['change'] == 'grant_support') {
      await _grantEndpoint(hub, review);
    }
  }

  bool _textOnlyReport(CustomerAction action) =>
      action['change'] == 'submit_report' && action['include_bundle'] == false;

  Future<bool> execute(CustomerAction claim) async {
    if (!isCurrentAccount()) throw StateError('Account changed');
    if (_accountChange(claim)) return true;
    if (_textOnlyReport(claim)) {
      await DebugBundleSubmissionService.instance.submitTextOnly(
          submissionId: claim['plan_id'] as String,
          reportKind: SupportReportKind.bug,
          summary: claim['report_summary'] as String,
          attachAppLogs: false);
      return true;
    }
    final hub = _matched(claim);
    final instance = claim['server_instance_id'] as String;
    final change = claim['change'];
    if (change == 'join_home') {
      final proof = await _proof(hub, instance);
      if ((claim['home_id'] != null && claim['home_id'] != proof.homeId) ||
          (claim['hub_id'] != null && claim['hub_id'] != proof.hubId)) {
        return false;
      }
      final joined = await CloudHomeJoinService.instance.joinByLocalDeviceProof(
          serverInstanceId: instance,
          joinProof: proof,
          host: hub.endpoint.host,
          port: hub.endpoint.port,
          ownerToken: hub.token,
          hubName: hub.name);
      if (!isCurrentAccount() ||
          joined.home?.home.id != proof.homeId ||
          joined.home?.serverHubs.any((h) => h.id == proof.hubId) != true) {
        return false;
      }
      return await homes.enterHome(joined.home!) != null;
    }
    final endpoint = change == 'grant_support'
        ? await _grantEndpoint(hub, claim)
        : await ServerEndpointResolver.resolve(hub, syncProvider: sync);
    final hello =
        await RhythmConfigApi(baseUrl: endpoint.baseUrl, authToken: hub.token)
            .getState();
    _matched(claim);
    if (hello.serverInstanceId != instance) return false;
    // Local aliases can differ from the canonical hub row for shared Homes.
    final cloudHub = hub.copyWith(id: claim['hub_id'] as String);
    switch (change) {
      case 'grant_support':
        await SupportAccessService.instance.grantForHub(
            cloudHub.copyWith(endpoint: endpoint.endpoint),
            assistantPlanId: claim['plan_id'] as String);
        return true;
      case 'enable_remote':
        final result = await RemoteAccessService.instance.enableForHub(hub,
            reviewedCloudHubId: cloudHub.id,
            home: homes.currentHome,
            explicitUserEnable: true,
            autoGrantSupport: false,
            expectedServerInstanceId: instance,
            assistantPlanId: claim['plan_id'] as String,
            verifyRemoteRoute: true);
        _matched(claim);
        return await homes.updateHub(result.updatedHub) && result.routeVerified;
      case 'submit_report':
        final reports = DebugBundleSubmissionService.instance;
        final id = claim['plan_id'] as String;
        final summary = claim['report_summary'] as String;
        if (claim['include_bundle'] != true) return false;
        if (sync.serverCapabilities
                ?.supportsFeature(RhythmFeature.asyncDebugBundleUpload) ==
            true) {
          await reports.submitViaServerQueue(
              submissionId: id,
              reportKind: SupportReportKind.bug,
              serverHub: cloudHub,
              deviceClient: endpoint.diagnosticsApi(),
              serverVersion: sync.firmwareVersion,
              serverPlatformContext: sync.serverPlatformContext,
              summary: summary);
        } else {
          final bundle = await endpoint.diagnosticsApi().downloadDebugBundle();
          _matched(claim);
          await reports.submit(
              submissionId: id,
              reportKind: SupportReportKind.bug,
              serverHub: cloudHub,
              bundle: bundle,
              serverVersion: sync.firmwareVersion,
              serverPlatformContext: sync.serverPlatformContext,
              summary: summary);
        }
        return true;
      default:
        return false;
    }
  }
}
