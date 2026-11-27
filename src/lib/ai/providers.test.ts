import { describe, expect, it } from "vitest";
import { PROVIDERS, SUPPORTED_APIS, getProvider } from "./providers";

describe("AI providers", () => {
  it("resolves known providers and falls back to openrouter", () => {
    expect(getProvider("anthropic").id).toBe("anthropic");
    expect(getProvider("google").id).toBe("google");
    expect(getProvider("openai").id).toBe("openai");
    expect(getProvider("unknown-provider").id).toBe("openrouter");
    expect(getProvider(null).id).toBe("openrouter");
  });

  it("loads chat models with supported APIs for every registered provider", async () => {
    expect(PROVIDERS.length).toBe(34);
    for (const p of PROVIDERS) {
      const models = await p.loadCatalog();
      expect(models.length, `${p.id} should have chat models`).toBeGreaterThan(0);
      for (const m of models) {
        expect(SUPPORTED_APIS.has(m.api), `${p.id}/${m.id} has unsupported api ${m.api}`).toBe(true);
        expect(m.baseUrl.startsWith("https://"), `${p.id}/${m.id} should use https`).toBe(true);
      }
    }
  });
});
