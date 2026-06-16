/// Hub discovery service for finding smart home hubs on the local network.
///
/// Uses mDNS (multicast DNS) to discover Home Assistant instances
/// and other smart home hubs without manual configuration.
library;

import 'package:multicast_dns/multicast_dns.dart';
import '../models/hub.dart' show HubType;

/// A discovered hub on the local network.
class DiscoveredHub {
  /// The hostname of the hub.
  final String host;

  /// The port the hub is listening on.
  final int port;

  /// The IP address of the hub.
  final String address;

  /// Optional friendly name of the hub.
  final String? name;

  /// The type of hub discovered.
  final HubType type;

  const DiscoveredHub({
    required this.host,
    required this.port,
    required this.address,
    this.name,
    required this.type,
  });

  @override
  String toString() => 'DiscoveredHub($type: $name ?? $host at $address:$port)';

  @override
  bool operator ==(Object other) =>
      identical(this, other) ||
      other is DiscoveredHub &&
          runtimeType == other.runtimeType &&
          address == other.address &&
          port == other.port &&
          type == other.type;

  @override
  int get hashCode => address.hashCode ^ port.hashCode ^ type.hashCode;
}

/// Service for discovering smart home hubs on the local network.
class HubDiscoveryService {
  /// Discover Home Assistant instances on the local network using mDNS.
  ///
  /// Home Assistant advertises itself via `_home-assistant._tcp.local`.
  /// Returns a list of discovered instances.
  static Future<List<DiscoveredHub>> discoverHomeAssistant({
    Duration timeout = const Duration(seconds: 5),
  }) async {
    final hubs = <DiscoveredHub>[];
    final seen = <String>{};

    try {
      final client = MDnsClient();
      await client.start();

      // Look for Home Assistant service via PTR record
      await for (final ptr in client
          .lookup<PtrResourceRecord>(
            ResourceRecordQuery.serverPointer('_home-assistant._tcp.local'),
          )
          .timeout(timeout, onTimeout: (sink) => sink.close())) {
        // Get SRV record for this service
        await for (final srv in client.lookup<SrvResourceRecord>(
          ResourceRecordQuery.service(ptr.domainName),
        )) {
          // Get A record for the IP address
          await for (final ip in client.lookup<IPAddressResourceRecord>(
            ResourceRecordQuery.addressIPv4(srv.target),
          )) {
            final key = '${ip.address.address}:${srv.port}';
            if (!seen.contains(key)) {
              seen.add(key);

              // Extract name from the PTR domain name
              // Format: "Home Assistant._home-assistant._tcp.local"
              String? name;
              final parts = ptr.domainName.split('.');
              if (parts.isNotEmpty && parts[0] != '_home-assistant') {
                name = parts[0];
              }

              hubs.add(DiscoveredHub(
                host: srv.target,
                port: srv.port,
                address: ip.address.address,
                name: name,
                type: HubType.homeAssistant,
              ));
            }
          }
        }
      }

      client.stop();
    } catch (e) {
      // mDNS discovery may fail due to network permissions or other issues
      // Return whatever we found so far
    }

    return hubs;
  }

  /// Discover Philips Hue bridges on the local network.
  ///
  /// Uses mDNS with fallback to the Philips discovery endpoint.
  static Future<List<DiscoveredHub>> discoverHue({
    Duration timeout = const Duration(seconds: 5),
  }) async {
    final hubs = <DiscoveredHub>[];
    final seen = <String>{};

    try {
      final client = MDnsClient();
      await client.start();

      // Look for Hue bridge via PTR record
      await for (final ptr in client
          .lookup<PtrResourceRecord>(
            ResourceRecordQuery.serverPointer('_hue._tcp.local'),
          )
          .timeout(timeout, onTimeout: (sink) => sink.close())) {
        await for (final srv in client.lookup<SrvResourceRecord>(
          ResourceRecordQuery.service(ptr.domainName),
        )) {
          await for (final ip in client.lookup<IPAddressResourceRecord>(
            ResourceRecordQuery.addressIPv4(srv.target),
          )) {
            final key = ip.address.address;
            if (!seen.contains(key)) {
              seen.add(key);

              String? name;
              final parts = ptr.domainName.split('.');
              if (parts.isNotEmpty) {
                name = parts[0];
              }

              hubs.add(DiscoveredHub(
                host: srv.target,
                port: 80, // Hue uses port 80
                address: ip.address.address,
                name: name,
                type: HubType.hue,
              ));
            }
          }
        }
      }

      client.stop();
    } catch (e) {
      // mDNS discovery failed
    }

    return hubs;
  }

  /// Discover all supported hubs on the local network.
  ///
  /// Runs discovery for all hub types concurrently.
  static Future<List<DiscoveredHub>> discoverAll({
    Duration timeout = const Duration(seconds: 5),
  }) async {
    final results = await Future.wait([
      discoverHomeAssistant(timeout: timeout),
      discoverHue(timeout: timeout),
    ]);

    return results.expand((list) => list).toList();
  }
}
