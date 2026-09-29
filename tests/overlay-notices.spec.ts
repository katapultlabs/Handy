import { expect, test, type Page } from "@playwright/test";

type FixtureCall = {
  command: string;
  payload?: Record<string, unknown>;
};

const fixtureCalls = (page: Page) =>
  page.evaluate(
    () =>
      (
        window as unknown as {
          overlayNoticeFixture: { calls: FixtureCall[] };
        }
      ).overlayNoticeFixture.calls,
  );

test.beforeEach(async ({ page }) => {
  await page.goto("/tests/fixtures/overlay-notices.html");
  await expect(
    page.getByText("Always replace catapult with Katapult?", { exact: true }),
  ).toBeVisible();
});

test("replays the current notice and acknowledges it only after render", async ({
  page,
}) => {
  await expect
    .poll(async () => await fixtureCalls(page))
    .toContainEqual({
      command: "acknowledge_correction_notice",
      payload: { token: 100 },
    });
  await expect(page.getByText("2 more", { exact: true })).toBeVisible();
});

test("discards a stale event snapshot after a newer revision", async ({
  page,
}) => {
  await page.evaluate(() =>
    (
      window as unknown as {
        overlayNoticeFixture: {
          publishFreshThenStale(): Promise<void>;
        };
      }
    ).overlayNoticeFixture.publishFreshThenStale(),
  );
  await expect(
    page.getByText("Always replace fresh phrase with Fresh Phrase?", {
      exact: true,
    }),
  ).toBeVisible();
  await expect(page.getByText("fresh phrase", { exact: false })).toBeVisible();
  await expect(page.getByText("catapult", { exact: false })).toHaveCount(0);
});

test("handles every correction in a queued batch", async ({ page }) => {
  await page.getByRole("button", { name: "Always replace" }).click();
  await expect(
    page.getByText("Always replace sense again with Sensei Gen?", {
      exact: true,
    }),
  ).toBeVisible();
  await page.getByRole("button", { name: "Ignore", exact: true }).click();
  await expect(
    page.getByText("Learned: codexx → Codex", { exact: true }),
  ).toBeVisible();
  await expect(page.getByRole("button", { name: "Next" })).toHaveCount(0);
  await expect(page.getByRole("button", { name: "Dismiss" })).toBeVisible();
  await page.getByRole("button", { name: "Undo", exact: true }).click();
  await expect(page.locator(".correction-card")).toHaveCount(0);

  const calls = await fixtureCalls(page);
  expect(
    calls.filter((call) => call.command === "act_on_correction_notice"),
  ).toEqual([
    {
      command: "act_on_correction_notice",
      payload: { token: 100, action: "accept" },
    },
    {
      command: "act_on_correction_notice",
      payload: { token: 101, action: "reject" },
    },
    {
      command: "act_on_correction_notice",
      payload: { token: 102, action: "reject" },
    },
  ]);
});

test("Next exposes the following notice without rejecting the pair", async ({
  page,
}) => {
  await page.getByRole("button", { name: "Next", exact: true }).click();
  await expect(
    page.getByText("Always replace sense again with Sensei Gen?", {
      exact: true,
    }),
  ).toBeVisible();
  const calls = await fixtureCalls(page);
  expect(calls).toContainEqual({
    command: "dismiss_correction_notice",
    payload: { token: 100 },
  });
  expect(
    calls.filter((call) => call.command === "act_on_correction_notice"),
  ).toHaveLength(0);
});

test("keeps a failed action visible with a retry error", async ({ page }) => {
  await page.goto("/tests/fixtures/overlay-notices.html?fail=accept");
  await expect(
    page.getByText("Always replace catapult with Katapult?", { exact: true }),
  ).toBeVisible();
  await page.getByRole("button", { name: "Always replace" }).click();
  await expect(page.getByRole("alert")).toHaveText(
    "Could not update the Dictionary. Please try again.",
  );
  await expect(
    page.getByText("Always replace catapult with Katapult?", { exact: true }),
  ).toBeVisible();
  await page.getByRole("button", { name: "Always replace" }).click();
  await expect(page.getByText("sense again", { exact: false })).toBeVisible();
});

test("suppresses a notice during recording and restores it afterward", async ({
  page,
}) => {
  await page.evaluate(() =>
    (
      window as unknown as {
        overlayNoticeFixture: {
          setRecording(recording: boolean): Promise<void>;
        };
      }
    ).overlayNoticeFixture.setRecording(true),
  );
  await expect(page.locator(".correction-card")).toHaveCount(0);
  await page.evaluate(() =>
    (
      window as unknown as {
        overlayNoticeFixture: {
          setRecording(recording: boolean): Promise<void>;
        };
      }
    ).overlayNoticeFixture.setRecording(false),
  );
  await expect(
    page.getByText("Always replace catapult with Katapult?", { exact: true }),
  ).toBeVisible();
  await expect
    .poll(async () => await fixtureCalls(page))
    .toContainEqual({
      command: "acknowledge_correction_notice",
      payload: { token: 1100 },
    });
  const calls = await fixtureCalls(page);
  expect(
    calls.filter(
      (call) =>
        call.command === "acknowledge_correction_notice" &&
        call.payload?.token === 1100,
    ),
  ).toHaveLength(1);
});

test("pauses expiry while the notice is hovered", async ({ page }) => {
  const card = page.locator(".correction-card");
  await card.hover();
  await expect
    .poll(async () => await fixtureCalls(page))
    .toContainEqual({
      command: "pause_correction_notice",
      payload: { token: 100, paused: true },
    });
  await page.mouse.move(0, 0);
  await expect
    .poll(async () => await fixtureCalls(page))
    .toContainEqual({
      command: "pause_correction_notice",
      payload: { token: 100, paused: false },
    });
});

test("keeps actions and an error visible in the 560 by 112 native window", async ({
  page,
}) => {
  await page.setViewportSize({ width: 560, height: 112 });
  await page.goto("/tests/fixtures/overlay-notices.html?fail=accept");
  await page.getByRole("button", { name: "Always replace" }).click();
  await expect(page.getByRole("alert")).toBeVisible();
  await expect(
    page.getByRole("button", { name: "Always replace" }),
  ).toBeVisible();
  await expect(page.getByRole("button", { name: "Ignore" })).toBeVisible();
  await expect(page.getByRole("button", { name: "Next" })).toBeVisible();
  const bounds = await page.locator(".correction-card").boundingBox();
  expect(bounds).not.toBeNull();
  expect(bounds!.y).toBeGreaterThanOrEqual(0);
  expect(bounds!.y + bounds!.height).toBeLessThanOrEqual(112);
  await page.screenshot({
    path: "test-results/correction-notice-560x112.png",
  });
});

test("keeps long correction pairs scrollable and actions reachable", async ({
  page,
}) => {
  await page.setViewportSize({ width: 560, height: 112 });
  await page.goto("/tests/fixtures/overlay-notices.html?long=1");
  await expect(
    page.getByRole("button", { name: "Always replace" }),
  ).toBeVisible();
  await expect(page.getByRole("button", { name: "Ignore" })).toBeVisible();
  await expect(page.getByRole("button", { name: "Next" })).toBeVisible();
  const dimensions = await page
    .locator(".correction-copy")
    .evaluate((element) => ({
      clientHeight: element.clientHeight,
      scrollHeight: element.scrollHeight,
      tabIndex: (element as HTMLElement).tabIndex,
    }));
  expect(dimensions.scrollHeight).toBeGreaterThan(dimensions.clientHeight);
  expect(dimensions.tabIndex).toBe(0);
  const bounds = await page.locator(".correction-card").boundingBox();
  expect(bounds).not.toBeNull();
  expect(bounds!.y + bounds!.height).toBeLessThanOrEqual(112);
});
