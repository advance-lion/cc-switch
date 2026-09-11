import { http, HttpResponse } from "msw";
import { describe, expect, it } from "vitest";
import { providerCenterApi } from "@/lib/api/providerCenter";
import { server } from "../msw/server";

const TAURI_ENDPOINT = "http://tauri.local";

describe("providerCenterApi import contract", () => {
  it("sends the explicit decision payload when committing a candidate", async () => {
    let received: Record<string, unknown> | null = null;
    server.use(
      http.post(
        `${TAURI_ENDPOINT}/commit_provider_center_import_candidate`,
        async ({ request }) => {
          received = (await request.json()) as Record<string, unknown>;
          return HttpResponse.json({
            action: "merge",
            repeated: false,
          });
        },
      ),
    );

    const result = await providerCenterApi.commitImportCandidate(
      "session-1",
      "candidate-1",
      ["codex", "openclaw"],
      { action: "merge", targetProviderId: "shared-1", expectedRevision: 3 },
    );

    expect(received).toEqual({
      sessionId: "session-1",
      candidateId: "candidate-1",
      appTypes: ["codex", "openclaw"],
      decision: {
        action: "merge",
        targetProviderId: "shared-1",
        expectedRevision: 3,
      },
    });
    expect(result).toEqual({ action: "merge", repeated: false });
  });

  it("returns the repeated flag for idempotent re-commits", async () => {
    server.use(
      http.post(
        `${TAURI_ENDPOINT}/commit_provider_center_import_candidate`,
        () => HttpResponse.json({ action: "createCopy", repeated: true }),
      ),
    );

    const result = await providerCenterApi.commitImportCandidate(
      "session-1",
      "candidate-1",
      ["codex"],
      { action: "createCopy" },
    );

    expect(result.repeated).toBe(true);
  });

  it("surfaces structured import failures from the session payload", async () => {
    server.use(
      http.post(`${TAURI_ENDPOINT}/start_provider_center_import_session`, () =>
        HttpResponse.json({
          id: "session-1",
          state: "readyWithErrors",
          candidates: [],
          errors: [
            {
              appType: "codex",
              sourceRef: "live:codex",
              code: "IMPORT_LIVE_SCAN_FAILED",
              stage: "scanLive",
              message: "config parse failed",
            },
          ],
          createdAt: 1,
          expiresAt: 2,
        }),
      ),
    );

    const session = await providerCenterApi.startImportSession();

    expect(session.state).toBe("readyWithErrors");
    expect(session.errors).toEqual([
      {
        appType: "codex",
        sourceRef: "live:codex",
        code: "IMPORT_LIVE_SCAN_FAILED",
        stage: "scanLive",
        message: "config parse failed",
      },
    ]);
  });
});
