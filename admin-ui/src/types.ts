export type StaffStatus = {
  role: string | null;
  enabled: boolean;
  isActive: boolean;
  isAdmin: boolean;
};

export type AdminUser = {
  id: string;
  email?: string;
  createdAt?: string;
};

export type MeResponse = {
  user: AdminUser;
  staffStatus: StaffStatus;
};

export type AdminApiHealth = {
  ok: boolean;
  service: string;
  serviceRoleConfigured: boolean;
  supportAccessConfigured: boolean;
};

export type AdminApiReadiness = {
  ok: boolean;
  service: string;
  remoteDebugReady: boolean;
  checks: Record<string, boolean>;
  missing: string[];
  supportAccessKeyId?: string;
  notes: string[];
};

export type HubEndpoint = {
  host: string;
  port: number;
  useSsl: boolean;
  baseUrl: string;
};

export type SupportHome = {
  id: string;
  name: string;
  ownerId: string;
  memberIds: string[];
  locationCity?: string;
  timezone?: string;
  createdAt?: string;
  updatedAt?: string;
};

export type SupportHub = {
  id: string;
  homeId: string;
  type: string;
  name: string;
  endpoint: HubEndpoint;
  remoteEndpoint?: HubEndpoint;
  enabled: boolean;
  hasLegacyToken: boolean;
  hasEncryptedToken: boolean;
  serverInstanceId?: string;
  lastConnected?: string;
  createdAt?: string;
  updatedAt?: string;
};

export type SupportCustomerHome = {
  home: SupportHome;
  hubs: SupportHub[];
};

export type SupportCustomer = {
  ownerId: string;
  customerLabel: string;
  customerEmail?: string;
  customerName?: string;
  secondaryLabel?: string;
  homes: SupportCustomerHome[];
};

export type SupportSnapshot = {
  staffStatus: StaffStatus;
  totals: {
    customers: number;
    homes: number;
    hubs: number;
  };
  customers: SupportCustomer[];
};

export type DeleteHubResult = {
  deleted: true;
  hub: {
    id: string;
    homeId: string;
    name: string;
  };
  homeDeleted: false;
};

export type ProbeInventory = {
  lights: number;
  buttons: number;
  motionSensors: number;
  otherDevices: number;
  total: number;
};

export type DeviceStateSummary = {
  serverVersion: string;
  serverInstanceId?: string;
  platformType: string;
  platformContext: string;
  listenPort?: number;
  nodeCount: number;
  hubCount: number;
  lastTickEpochMs?: number;
  inventory?: ProbeInventory;
  activeMode?: string;
  lightRuntime?: string;
};

export type ProbeResult = {
  hubId: string;
  status: 'online' | 'offline' | 'auth_required';
  route?: 'remote' | 'local';
  baseUrl?: string;
  checkedAt: string;
  tokenAvailable: boolean;
  hasEncryptedToken: boolean;
  inventory?: ProbeInventory;
  serverVersion?: string;
  serverInstanceId?: string;
  message?: string;
};

export type DebugBundleDownload = {
  blob: Blob;
  fileName: string;
  route?: 'remote' | 'local';
  baseUrl?: string;
};

export type DeviceLogSource = {
  id: string;
  fileName: string;
  bytes: number;
  modifiedAt?: string;
};

export type DeviceLogSources = {
  hubId: string;
  route: 'remote' | 'local';
  baseUrl: string;
  fetchedAt: string;
  sources: DeviceLogSource[];
};

export type DeviceLogTailLine = {
  source: string;
  lineNumber: number;
  text: string;
};

export type DeviceLogTail = {
  hubId: string;
  route: 'remote' | 'local';
  baseUrl: string;
  fetchedAt: string;
  source: DeviceLogSource;
  lines: DeviceLogTailLine[];
  requestedLines: number;
  returnedLines: number;
};

export type DeviceStatus = {
  hubId: string;
  route: 'remote' | 'local';
  baseUrl: string;
  checkedAt: string;
  tokenAvailable: boolean;
  hasEncryptedToken: boolean;
  health?: Record<string, unknown>;
  state?: DeviceStateSummary;
  remoteAccess?: Record<string, unknown>;
  auth?: Record<string, unknown>;
  ota?: Record<string, unknown>;
  errors: Record<string, string>;
};

export type DeviceOtaAction = {
  hubId: string;
  route: 'remote' | 'local';
  baseUrl: string;
  action: 'check' | 'update';
  completedAt: string;
  tokenAvailable: boolean;
  hasEncryptedToken: boolean;
  result: Record<string, unknown>;
};

export type DeviceAdminMethod = 'GET' | 'POST' | 'PUT' | 'PATCH' | 'DELETE';

export type DeviceAdminProxyRequest = {
  method: DeviceAdminMethod;
  path: string;
  query?: Record<string, string>;
  body?: unknown;
  timeoutSeconds?: number;
  requestId?: string;
  expectedServerInstanceId?: string;
  resourcePrecondition?: {
    path: string;
    query?: Record<string, string>;
    bodySha256: string;
  };
};

export type DeviceAdminProxyResponse = {
  hubId: string;
  route: 'remote' | 'local';
  baseUrl: string;
  method: DeviceAdminMethod;
  path: string;
  queryParameters?: Record<string, string>;
  statusCode: number;
  completedAt: string;
  tokenAvailable: boolean;
  hasEncryptedToken: boolean;
  requestId?: string;
  verifiedServerInstanceId?: string;
  bodySha256?: string;
  preconditionBodySha256?: string;
  body: unknown;
};

export type HomeListItem = {
  customer: SupportCustomer;
  home: SupportHome;
  hubs: SupportHub[];
  searchText: string;
};
