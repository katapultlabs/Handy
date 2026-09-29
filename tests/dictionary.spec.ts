import { test, expect } from "@playwright/test";

test.beforeEach(async ({ page }) => {
  await page.goto("/tests/fixtures/dictionary.html");
  await expect(
    page.getByRole("heading", { name: "Suggested (3)" }),
  ).toBeVisible();
});

test("suggestions require approval, Ignore uses rejection, and approved entries can be disabled", async ({
  page,
}) => {
  const row = (text: string) =>
    page
      .locator("div.px-4.py-3")
      .filter({ has: page.getByText(text, { exact: true }) });
  await expect(
    page.getByRole("checkbox", { name: "Active", exact: true }),
  ).toHaveCount(0);
  await row("catapult").getByRole("button", { name: "Always replace" }).click();
  await expect(
    page.getByRole("heading", { name: "Your corrections (1)" }),
  ).toBeVisible();
  await expect(row("catapult").getByRole("checkbox")).toBeChecked();
  await row("sense again")
    .getByRole("button", { name: "Ignore", exact: true })
    .click();
  await row("enabled")
    .getByRole("button", { name: "Ignore", exact: true })
    .click();
  await expect(
    page.getByRole("heading", { name: "Suggested (0)" }),
  ).toBeVisible();
  await page.getByText("Ignored (2)", { exact: true }).click();
  await expect(
    row("sense again").getByRole("button", { name: "Always replace" }),
  ).toBeVisible();
  await row("catapult").getByRole("checkbox").uncheck();
  await expect(row("catapult").getByRole("checkbox")).not.toBeChecked();
  const calls = await page.evaluate(
    () => (window as unknown as { dictionaryCalls: string[] }).dictionaryCalls,
  );
  expect(
    calls.filter((call) => call === "confirm_dictionary_entry"),
  ).toHaveLength(1);
  expect(
    calls.filter((call) => call === "reject_dictionary_entry"),
  ).toHaveLength(2);
  expect(calls).not.toContain("delete_dictionary_entry");
});

test("editing a suggestion does not approve it; manual additions are active", async ({
  page,
}) => {
  const row = page.locator("div.px-4.py-3").nth(1);
  await row.getByRole("button", { name: "Edit", exact: true }).click();
  await row
    .getByRole("textbox", { name: "Correct", exact: true })
    .fill("Katapult Labs");
  await row.getByRole("button", { name: "Save", exact: true }).click();
  await expect(
    page.getByRole("heading", { name: "Suggested (3)" }),
  ).toBeVisible();
  await expect(
    page.getByRole("checkbox", { name: "Active", exact: true }),
  ).toHaveCount(0);
  await page
    .getByRole("textbox", { name: "Misheard", exact: true })
    .fill("codexx");
  await page
    .getByRole("textbox", { name: "Correct", exact: true })
    .fill("Codex");
  await page.getByRole("button", { name: "Add", exact: true }).click();
  await expect(
    page.getByRole("heading", { name: "Your corrections (1)" }),
  ).toBeVisible();
  await expect(
    page.getByRole("checkbox", { name: "Active", exact: true }),
  ).toBeChecked();
});

test("dictionary screen fits a settings window", async ({ page }) => {
  await page.setViewportSize({ width: 780, height: 900 });
  await expect(
    page.getByRole("button", { name: "Always replace" }),
  ).toHaveCount(3);
  expect(
    await page.evaluate(
      () => document.documentElement.scrollWidth <= window.innerWidth,
    ),
  ).toBe(true);
  await page.screenshot({
    path: "test-results/dictionary-suggestions.png",
    fullPage: true,
  });
});

test("automatic corrections have Undo without an approval step", async ({
  page,
}) => {
  await page.goto("/tests/fixtures/dictionary.html?automatic=1");
  const row = page
    .locator("div.px-4.py-3")
    .filter({ has: page.getByText("katapolt", { exact: true }) });
  await expect(row.getByText("Automatic", { exact: true })).toBeVisible();
  await expect(
    row.getByRole("checkbox", { name: "Active", exact: true }),
  ).toBeChecked();
  await expect(row.getByRole("button", { name: "Always replace" })).toHaveCount(
    0,
  );
  await page.screenshot({
    path: "test-results/dictionary-automatic.png",
    fullPage: true,
  });
  await row.getByRole("button", { name: "Undo", exact: true }).click();
  await expect(
    page.getByRole("heading", { name: "Your corrections (0)" }),
  ).toBeVisible();
  await page.getByText("Ignored (1)", { exact: true }).click();
  await expect(
    row.getByRole("button", { name: "Always replace" }),
  ).toBeVisible();
  const calls = await page.evaluate(
    () => (window as unknown as { dictionaryCalls: string[] }).dictionaryCalls,
  );
  expect(calls).toContain("reject_dictionary_entry");
  expect(calls).not.toContain("confirm_dictionary_entry");
  expect(calls).not.toContain("delete_dictionary_entry");
});
