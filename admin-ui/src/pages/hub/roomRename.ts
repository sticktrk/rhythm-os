import {
  isRoomKind,
  topologyItemsFromPayload
} from './topologyMembership.ts';

export type RoomRenameProposal = {
  roomId: string;
  before: string;
  candidate: string;
};

export function buildRoomRenameProposal(
  roomId: string,
  before: string,
  draft: string
): RoomRenameProposal | null {
  const candidate = draft.trim();
  if (roomId.trim() === '' || candidate === '' || candidate === before) {
    return null;
  }
  return { roomId, before, candidate };
}

export function roomNameFromTopology(
  payload: unknown,
  roomId: string
): string | null {
  const room = topologyItemsFromPayload(payload).find(
    (item) => item.id === roomId && isRoomKind(item.kind)
  );
  return room?.name ?? null;
}

export function roomRenameConfirmationStatus(
  after: string | null,
  candidate: string
): 'confirmed' | 'conflict' {
  return after === candidate ? 'confirmed' : 'conflict';
}

export function createRoomRenameRequestId(): string {
  if (typeof crypto !== 'undefined' && typeof crypto.randomUUID === 'function') {
    return `admin-room-rename:${crypto.randomUUID()}`;
  }
  return `admin-room-rename:${Date.now()}:${Math.random().toString(16).slice(2)}`;
}
