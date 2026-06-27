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

export type ProbeInventory = {
  lights: number;
  buttons: number;
  motionSensors: number;
  otherDevices: number;
  total: number;
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

export type HomeListItem = {
  customer: SupportCustomer;
  home: SupportHome;
  hubs: SupportHub[];
  searchText: string;
};
