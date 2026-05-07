const apiBase = "https://console.vast.ai/api/v0";

const env = process.env;

type JsonObject = Record<string, unknown>;
type VastTemplate = JsonObject & {
  id?: string | number;
  hash_id?: string;
};

function requiredEnv(name: string): string {
  const value = env[name];
  if (!value) throw new Error(`${name} is required`);
  return value;
}

function optionalEnv(name: string): string | undefined {
  return env[name];
}

function optionalNumber(name: string, defaultValue: number): number {
  const value = optionalEnv(name);
  if (!value) return defaultValue;
  const parsed = Number.parseFloat(value);
  if (!Number.isFinite(parsed)) throw new Error(`${name} must be a number`);
  return parsed;
}

function optionalObject(name: string, defaultValue: JsonObject): JsonObject {
  const value = optionalEnv(name);
  if (!value) return defaultValue;
  const parsed = JSON.parse(value);
  if (!parsed || typeof parsed !== "object" || Array.isArray(parsed)) {
    throw new Error(`${name} must be a JSON object`);
  }
  return parsed as JsonObject;
}

function withoutUndefined<T extends JsonObject>(object: T): JsonObject {
  return Object.fromEntries(Object.entries(object).filter(([, value]) => value !== undefined));
}

function optionalString(value: unknown): string | undefined {
  if (typeof value === "string") return value;
  if (typeof value === "number") return String(value);
  return undefined;
}

function dockerEnv(vars: Record<string, string | undefined>): string {
  return Object.entries(vars)
    .filter(([, value]) => value !== undefined)
    .map(([key, value]) => `-e ${key}=${shellQuote(value!)}`)
    .join(" ");
}

function shellQuote(value: string): string {
  if (/^[A-Za-z0-9_./:@=-]+$/.test(value)) return value;
  return `'${value.replaceAll("'", "'\\''")}'`;
}

async function vast(method: string, body: JsonObject): Promise<JsonObject> {
  const response = await fetch(`${apiBase}/template/`, {
    method,
    headers: {
      authorization: `Bearer ${requiredEnv("VAST_API_KEY")}`,
      "content-type": "application/json",
    },
    body: JSON.stringify(body),
  });

  const text = await response.text();
  const payload = text ? JSON.parse(text) : {};
  if (!response.ok) {
    throw new Error(`Vast ${method} /template failed with HTTP ${response.status}: ${text}`);
  }
  return payload;
}

async function main() {
  const image = requiredEnv("VAST_TEMPLATE_IMAGE");
  const name = optionalEnv("VAST_TEMPLATE_NAME") ?? "lumen-render-vast";
  const tag = optionalEnv("VAST_TEMPLATE_TAG");
  const apiToken = optionalEnv("LUMEN_VAST_API_TOKEN");
  const extraFilters = optionalObject("VAST_TEMPLATE_EXTRA_FILTERS", {
    gpu_name: {
      in: [
        "RTX 5070 Ti",
        "RTX 5080",
        "RTX 5090",
        "RTX 4070 Ti",
        "RTX 4080",
        "RTX 4090",
        "L4",
        "RTX 4000 Ada",
        "RTX 4500 Ada",
        "RTX 5000 Ada",
        "RTX 6000 Ada",
      ],
    },
    dph_total: { lte: optionalNumber("VAST_TEMPLATE_MAX_DPH_TOTAL", 0.7) },
    direct_port_count: { gte: 1 },
    cuda_max_good: { gte: 12.8 },
    rentable: { eq: true },
  });
  const templateBody = withoutUndefined({
    name,
    image,
    tag,
    desc: "Lumen GPU render server for Vast instances.",
    readme:
      "Runs lumen-vast on port 8080. POST RenderJobInput JSON to /render; GET /health for readiness.",
    env: [
      "-p 8080:8080",
      dockerEnv({
        LUMEN_VAST_API_TOKEN: apiToken,
        LUMEN_VIDEO_ENCODER: optionalEnv("LUMEN_VIDEO_ENCODER"),
        NVIDIA_DRIVER_CAPABILITIES: optionalEnv("NVIDIA_DRIVER_CAPABILITIES") ?? "all",
        NVIDIA_VISIBLE_DEVICES: optionalEnv("NVIDIA_VISIBLE_DEVICES") ?? "all",
        XDG_RUNTIME_DIR: optionalEnv("XDG_RUNTIME_DIR") ?? "/tmp/lumen-runtime",
      }),
    ]
      .filter(Boolean)
      .join(" "),
    runtype: "args",
    args_str: "",
    extra_filters: extraFilters,
    recommended_disk_space: optionalNumber("VAST_TEMPLATE_DISK_GB", 64),
    private: optionalEnv("VAST_TEMPLATE_PUBLIC") !== "true",
  });

  const existingHashId = optionalEnv("VAST_TEMPLATE_HASH_ID");
  const payload = existingHashId
    ? await vast("PUT", { ...templateBody, hash_id: existingHashId })
    : await vast("POST", templateBody);
  const template = payload.template as VastTemplate | undefined;
  const templateId = optionalString(template?.id) ?? "";
  const templateHashId = optionalString(template?.hash_id) ?? existingHashId ?? "";

  console.log(`VAST_TEMPLATE_ID=${templateId}`);
  console.log(`VAST_TEMPLATE_HASH_ID=${templateHashId}`);
}

main().catch((error) => {
  console.error(error);
  process.exitCode = 1;
});
