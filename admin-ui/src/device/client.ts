import { runDeviceAdminProxy } from '../api';
import type {
  DeviceAdminMethod,
  DeviceAdminProxyResponse
} from '../types';

export class DeviceHttpError extends Error {
  readonly statusCode: number;
  readonly body: unknown;
  readonly route?: 'remote' | 'local';

  constructor(
    statusCode: number,
    body: unknown,
    route?: 'remote' | 'local'
  ) {
    super(deviceErrorMessage(statusCode, body));
    this.name = 'DeviceHttpError';
    this.statusCode = statusCode;
    this.body = body;
    this.route = route;
  }
}

function deviceErrorMessage(statusCode: number, body: unknown): string {
  if (typeof body === 'object' && body !== null) {
    const record = body as Record<string, unknown>;
    for (const key of ['error', 'message', 'detail']) {
      const value = record[key];
      if (typeof value === 'string' && value.trim().length > 0) {
        return `Device HTTP ${statusCode}: ${value.trim()}`;
      }
    }
  }
  if (typeof body === 'string' && body.trim().length > 0) {
    return `Device HTTP ${statusCode}: ${body.trim().slice(0, 200)}`;
  }
  if (statusCode === 404) {
    return 'Device HTTP 404 — endpoint not implemented on this firmware, or the target id does not exist.';
  }
  return `Device request failed with HTTP ${statusCode}.`;
}

/** Resolve to a marker object instead of rejecting when the device lacks the
    route, so read cards can render a muted "not implemented" note. */
export async function orNotImplemented<T>(
  promise: Promise<T>
): Promise<T | { __not_implemented: true }> {
  try {
    return await promise;
  } catch (error) {
    if (isNotImplemented(error)) return { __not_implemented: true };
    throw error;
  }
}

/** True when the device answered but has no route for the endpoint —
    i.e. this firmware build doesn't implement it. Pages should render a
    muted "not available" state instead of an error. */
export function isNotImplemented(error: unknown): boolean {
  return error instanceof DeviceHttpError && error.statusCode === 404;
}

export type DeviceRequestOptions = {
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

export class DeviceClient {
  constructor(
    private readonly accessToken: string,
    readonly hubId: string
  ) {}

  async request<T = unknown>(
    method: DeviceAdminMethod,
    path: string,
    options: DeviceRequestOptions = {}
  ): Promise<T> {
    const response = await this.requestReceipt(method, path, options);
    return response.body as T;
  }

  async requestReceipt(
    method: DeviceAdminMethod,
    path: string,
    options: DeviceRequestOptions = {}
  ): Promise<DeviceAdminProxyResponse> {
    const response = await runDeviceAdminProxy(this.accessToken, this.hubId, {
      method,
      path,
      ...(options.query && Object.keys(options.query).length > 0
        ? { query: options.query }
        : {}),
      ...(options.body === undefined ? {} : { body: options.body }),
      timeoutSeconds: options.timeoutSeconds ?? 15,
      ...(options.requestId ? { requestId: options.requestId } : {}),
      ...(options.expectedServerInstanceId
        ? { expectedServerInstanceId: options.expectedServerInstanceId }
        : {}),
      ...(options.resourcePrecondition
        ? { resourcePrecondition: options.resourcePrecondition }
        : {})
    });
    if (response.statusCode >= 400) {
      throw new DeviceHttpError(
        response.statusCode,
        response.body,
        response.route
      );
    }
    return response;
  }

  get<T = unknown>(path: string, options?: DeviceRequestOptions) {
    return this.request<T>('GET', path, options);
  }

  getReceipt(path: string, options?: DeviceRequestOptions) {
    return this.requestReceipt('GET', path, options);
  }

  post<T = unknown>(path: string, options?: DeviceRequestOptions) {
    return this.request<T>('POST', path, options);
  }

  put<T = unknown>(path: string, options?: DeviceRequestOptions) {
    return this.request<T>('PUT', path, options);
  }

  putReceipt(path: string, options?: DeviceRequestOptions) {
    return this.requestReceipt('PUT', path, options);
  }

  delete<T = unknown>(path: string, options?: DeviceRequestOptions) {
    return this.request<T>('DELETE', path, options);
  }
}
