const apiBase = "https://rest.runpod.io/v1";

const env = process.env;

type JsonObject = Record<string, unknown>;
type RunpodResource = JsonObject & { id?: string };

function requiredEnv(name: string): string {
  const value = env[name];
  if (!value) throw new Error(`${name} is required`);
  return value;
}

function optionalEnv(name: string): string | undefined {
  return env[name];
}

function optionalInt(name: string, defaultValue: number): number {
  const value = optionalEnv(name);
  if (!value) return defaultValue;

  const parsed = Number.parseInt(value, 10);
  if (!Number.isFinite(parsed)) {
    throw new Error(`${name} must be an integer`);
  }
  return parsed;
}

function optionalList(name: string, defaultValue: string[] = []): string[] {
  const value = optionalEnv(name);
  if (!value) return defaultValue;
  return value
    .split(",")
    .map((item) => item.trim())
    .filter(Boolean);
}

function optionalObject(name: string): JsonObject {
  const value = env[name];
  if (!value) return {};
  const parsed = JSON.parse(value);
  if (!parsed || typeof parsed !== "object" || Array.isArray(parsed)) {
    throw new Error(`${name} must be a JSON object`);
  }
  return parsed;
}

function withoutUndefined<T extends JsonObject>(object: T): JsonObject {
  return Object.fromEntries(Object.entries(object).filter(([, value]) => value !== undefined));
}

function requiredString(value: unknown, label: string): string {
  if (typeof value === "string" && value) return value;
  throw new Error(`${label} is missing`);
}

function assertNoKeys(label: string, object: JsonObject, keys: string[]) {
  const present = keys.filter((key) => Object.hasOwn(object, key));
  if (present.length) {
    throw new Error(`${label} body contains create-only keys: ${present.join(", ")}`);
  }
}

async function runpod(
  path: string,
  { method = "GET", body }: { method?: string; body?: JsonObject } = {},
): Promise<JsonObject> {
  const response = await fetch(`${apiBase}${path}`, {
    method,
    headers: body
      ? {
          authorization: `Bearer ${requiredEnv("RUNPOD_API_KEY")}`,
          "content-type": "application/json",
        }
      : { authorization: `Bearer ${requiredEnv("RUNPOD_API_KEY")}` },
    body: body ? JSON.stringify(body) : undefined,
  });

  const text = await response.text();
  const payload = text ? JSON.parse(text) : {};

  if (!response.ok) {
    throw new Error(
      `RunPod ${method} ${path} failed with HTTP ${response.status}: ${JSON.stringify(payload)}`,
    );
  }

  return payload;
}

async function main() {
  const endpointName = optionalEnv("RUNPOD_SERVERLESS_ENDPOINT_NAME") ?? "lumen-render";
  const templateName = optionalEnv("RUNPOD_SERVERLESS_TEMPLATE_NAME") ?? `${endpointName}-template`;
  const imageName = requiredEnv("RUNPOD_SERVERLESS_IMAGE");
  const concurrency = optionalInt("LUMEN_RUNPOD_CONCURRENCY", 2);
  const templateEnv = withoutUndefined({
    ...optionalObject("RUNPOD_SERVERLESS_TEMPLATE_ENV"),
    LUMEN_RUNPOD_CONCURRENCY: String(concurrency),
    LUMEN_VIDEO_ENCODER: optionalEnv("LUMEN_VIDEO_ENCODER"),
    NVIDIA_DRIVER_CAPABILITIES: optionalEnv("NVIDIA_DRIVER_CAPABILITIES") ?? "all",
    NVIDIA_VISIBLE_DEVICES: optionalEnv("NVIDIA_VISIBLE_DEVICES") ?? "all",
    XDG_RUNTIME_DIR: optionalEnv("XDG_RUNTIME_DIR") ?? "/tmp/lumen-runtime",
  });

  const templateUpdateBody = withoutUndefined({
    containerDiskInGb: optionalInt("RUNPOD_SERVERLESS_CONTAINER_DISK_GB", 64),
    containerRegistryAuthId:
      optionalEnv("RUNPOD_SERVERLESS_CONTAINER_REGISTRY_AUTH_ID") ??
      optionalEnv("RUNPOD_FLASH_CONTAINER_REGISTRY_AUTH_ID"),
    dockerStartCmd: ["/usr/local/bin/lumen-runpod"],
    env: templateEnv,
    imageName,
    isPublic: false,
    name: templateName,
    readme: "Lumen Rust RunPod Serverless render worker.",
  });
  const templateCreateBody = withoutUndefined({
    ...templateUpdateBody,
    category: optionalEnv("RUNPOD_SERVERLESS_CATEGORY") ?? "NVIDIA",
    isServerless: true,
  });
  assertNoKeys("template PATCH", templateUpdateBody, ["category", "isServerless"]);

  const existingTemplateId = optionalEnv("RUNPOD_SERVERLESS_TEMPLATE_ID");
  const template = (
    existingTemplateId
      ? await runpod(`/templates/${existingTemplateId}`, {
          method: "PATCH",
          body: templateUpdateBody,
        })
      : await runpod("/templates", { method: "POST", body: templateCreateBody })
  ) as RunpodResource;

  const templateId = template.id ?? requiredString(existingTemplateId, "RunPod template id");

  const endpointUpdateBody = withoutUndefined({
    dataCenterIds: optionalList("RUNPOD_SERVERLESS_DATA_CENTER_IDS"),
    executionTimeoutMs: optionalInt("RUNPOD_SERVERLESS_EXECUTION_TIMEOUT_MS", 1_800_000),
    flashboot: optionalEnv("RUNPOD_SERVERLESS_FLASHBOOT") !== "false",
    gpuCount: optionalInt("RUNPOD_SERVERLESS_GPU_COUNT", 1),
    gpuTypeIds: optionalList("RUNPOD_SERVERLESS_GPU_TYPES", [
      "NVIDIA RTX 2000 Ada Generation",
      "NVIDIA RTX 4000 Ada Generation",
      "NVIDIA L4",
    ]),
    idleTimeout: optionalInt("RUNPOD_SERVERLESS_IDLE_TIMEOUT", 5),
    minCudaVersion: optionalEnv("RUNPOD_SERVERLESS_MIN_CUDA_VERSION") ?? "12.8",
    name: endpointName,
    networkVolumeId: optionalEnv("RUNPOD_SERVERLESS_NETWORK_VOLUME_ID"),
    networkVolumeIds: optionalList("RUNPOD_SERVERLESS_NETWORK_VOLUME_IDS"),
    scalerType: optionalEnv("RUNPOD_SERVERLESS_SCALER_TYPE") ?? "REQUEST_COUNT",
    scalerValue: optionalInt("RUNPOD_SERVERLESS_SCALER_VALUE", 1),
    templateId,
    workersMax: optionalInt("RUNPOD_SERVERLESS_MAX_WORKERS", 2),
    workersMin: optionalInt("RUNPOD_SERVERLESS_MIN_WORKERS", 0),
  });
  const endpointCreateBody = withoutUndefined({
    ...endpointUpdateBody,
    computeType: optionalEnv("RUNPOD_SERVERLESS_COMPUTE_TYPE") ?? "GPU",
  });
  assertNoKeys("endpoint PATCH", endpointUpdateBody, ["computeType"]);

  const existingEndpointId =
    optionalEnv("RUNPOD_ENDPOINT_ID") ?? optionalEnv("RUNPOD_SERVERLESS_ENDPOINT_ID");
  const endpoint = (
    existingEndpointId
      ? await runpod(`/endpoints/${existingEndpointId}`, {
          method: "PATCH",
          body: endpointUpdateBody,
        })
      : await runpod("/endpoints", { method: "POST", body: endpointCreateBody })
  ) as RunpodResource;

  const endpointId = endpoint.id ?? requiredString(existingEndpointId, "RunPod endpoint id");

  console.log(`RUNPOD_TEMPLATE_ID=${templateId}`);
  console.log(`RUNPOD_ENDPOINT_ID=${endpointId}`);
  console.log(`RUNPOD_ENDPOINT_URL=https://api.runpod.ai/v2/${endpointId}`);
}

main().catch((error) => {
  console.error(error);
  process.exitCode = 1;
});
