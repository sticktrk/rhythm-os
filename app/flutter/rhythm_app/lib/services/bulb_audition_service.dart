import 'matter_bulb_tester_service.dart';

typedef BulbAuditionCloudResult = MatterBulbTesterCloudResult;

abstract interface class BulbAuditionReportService {
  Future<void> saveLocalReport(Map<String, dynamic> report);

  Future<BulbAuditionCloudResult> submitCloudReport({
    required Map<String, dynamic> report,
    String? serverVersion,
    String? serverPlatformContext,
  });
}

/// Canonical report store/uploader. It deliberately reuses the prior local
/// key so upgrading does not strand unsent schema-v2 evidence.
class BulbAuditionService implements BulbAuditionReportService {
  BulbAuditionService._();

  static final BulbAuditionService instance = BulbAuditionService._();

  @override
  Future<void> saveLocalReport(Map<String, dynamic> report) {
    return MatterBulbTesterService.instance.saveLocalReport(report);
  }

  @override
  Future<BulbAuditionCloudResult> submitCloudReport({
    required Map<String, dynamic> report,
    String? serverVersion,
    String? serverPlatformContext,
  }) {
    return MatterBulbTesterService.instance.submitCloudReport(
      report: report,
      serverVersion: serverVersion,
      serverPlatformContext: serverPlatformContext,
    );
  }
}
