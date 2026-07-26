import type {
  SupportCustomer,
  SupportHome,
  SupportHub,
  SupportSnapshot
} from '../types';

export type HomeDirectoryItem = {
  home: SupportHome;
  email: string | null;
  searchText: string;
};

export type HomeLookup = {
  home: SupportHome;
  hubs: SupportHub[];
  customer: SupportCustomer;
  email: string | null;
};

export function customerEmail(customer: SupportCustomer): string | null {
  const email = customer.customerEmail?.trim();
  if (email) return email;

  const label = customer.customerLabel.trim();
  return label.includes('@') ? label : null;
}

export function flattenHomes(
  snapshot: SupportSnapshot | null
): HomeDirectoryItem[] {
  if (!snapshot) return [];

  return snapshot.customers.flatMap((customer) => {
    const email = customerEmail(customer);
    return customer.homes.map((entry) => {
      const searchable = [
        entry.home.id,
        entry.home.name,
        entry.home.locationCity,
        entry.home.timezone,
        email
      ];
      return {
        home: entry.home,
        email,
        searchText: searchable.filter(Boolean).join(' ').toLowerCase()
      };
    });
  });
}

export function findHome(
  snapshot: SupportSnapshot | null,
  homeId: string | undefined
): HomeLookup | null {
  if (!snapshot || !homeId) return null;

  for (const customer of snapshot.customers) {
    const entry = customer.homes.find(
      (candidate) => candidate.home.id === homeId
    );
    if (entry) {
      return {
        customer,
        home: entry.home,
        hubs: entry.hubs,
        email: customerEmail(customer)
      };
    }
  }
  return null;
}
