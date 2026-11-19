import { type Fields, object, operations, Rejection } from "./contract.ts";
import { modelResult } from "./privacy.ts";

const headers = {
  "content-type": "application/json",
  "cache-control": "no-store",
  "access-control-allow-origin": "*",
  "access-control-allow-headers":
    "authorization,apikey,x-client-info,content-type,mcp-protocol-version",
  "access-control-allow-methods": "POST,OPTIONS",
};
export const response = (body: unknown, status = 200) =>
  new Response(body === null ? null : JSON.stringify(body), {
    status,
    headers,
  });
export const versions = ["2025-03-26", "2025-06-18", "2025-11-25"];
const uri = "rhythm://cloud/customer/contract";
const contractText = JSON.stringify({ schema_version: 1, operations });
const contractHash = crypto.subtle.digest(
  "SHA-256",
  new TextEncoder().encode(contractText),
).then((d) =>
  Array.from(new Uint8Array(d), (v) => v.toString(16).padStart(2, "0")).join("")
);

export async function contract() {
  return {
    schema_version: 1,
    location: "cloud",
    contract_sha256: await contractHash,
    operations,
  };
}
const modelErrorCodes = new Set([
  "access_required",
  "home_access_required",
  "hub_access_required",
  "live_hub_identity_required",
  "review_changed_or_expired",
  "invalid_arguments",
  "missing_arguments",
  "unknown_argument",
  "invalid_argument",
  "invalid_change_scope",
  "invalid_receipt",
  "unknown_operation",
  "sign_in",
]);
export async function mcp(
  body: Fields,
  execute: (name: string, args: unknown) => Promise<Fields>,
) {
  const id = body.id;
  const error = (code: number, message: string) =>
    response({ jsonrpc: "2.0", id: id ?? null, error: { code, message } });
  if (
    body.jsonrpc !== "2.0" || typeof body.method !== "string" ||
    (id !== undefined && typeof id !== "string" && !Number.isSafeInteger(id))
  ) return error(-32600, "Invalid request");
  if (id === undefined) return response(null, 202);
  const params = object(body.params) ? body.params : {};
  let result: unknown;
  switch (body.method) {
    case "initialize":
      result = {
        protocolVersion: versions.includes(String(params.protocolVersion))
          ? params.protocolVersion
          : versions[1],
        capabilities: { tools: {}, resources: {} },
        serverInfo: { name: "Rhythm account", version: "1" },
      };
      break;
    case "ping":
      result = {};
      break;
    case "tools/list":
      result = {
        tools: operations.map((op) => ({
          name: op.id,
          description: op.description,
          inputSchema: op.input_schema,
          annotations: { readOnlyHint: op.tier === "read" },
        })),
      };
      break;
    case "resources/list":
      result = {
        resources: [{
          uri,
          name: "Resident cloud tools",
          mimeType: "application/json",
        }],
      };
      break;
    case "resources/templates/list":
      result = { resourceTemplates: [] };
      break;
    case "resources/read":
      if (params.uri !== uri) return error(-32002, "Unknown resource");
      result = {
        contents: [{
          uri,
          mimeType: "application/json",
          text: JSON.stringify(await contract()),
        }],
      };
      break;
    case "tools/call": {
      const name = String(params.name);
      if (!operations.some((op) => op.id === name)) {
        return error(-32602, "Unknown tool");
      }
      try {
        const value = await execute(name, params.arguments ?? {});
        result = {
          isError: false,
          content: [{
            type: "text",
            text: JSON.stringify(modelResult(name, value)),
          }],
        };
      } catch (e) {
        result = {
          isError: true,
          content: [{
            type: "text",
            text: e instanceof Rejection && modelErrorCodes.has(e.message)
              ? e.message
              : "account_service_unavailable",
          }],
        };
      }
      break;
    }
    default:
      return error(-32601, "Method not found");
  }
  return response({ jsonrpc: "2.0", id, result });
}
