import { expect, test, type Page } from "@playwright/test";

type TestWindow = Window & {
  copiedText?: string;
  deferClipboard?: boolean;
  completeClipboard?: () => void;
};

async function copiedDocument(page: Page) {
  await page.keyboard.press("Control+a");
  await page.keyboard.press("Control+c");
  return page.evaluate(() => (window as TestWindow).copiedText ?? "");
}

async function nextPaint(page: Page) {
  await page.evaluate(() => new Promise<void>((resolve) => {
    requestAnimationFrame(() => requestAnimationFrame(() => resolve()));
  }));
}

test.beforeEach(async ({ page }) => {
  // このページからのコピーだけを捕捉し、OSのclipboardは変更しない。
  await page.addInitScript(() => {
    Object.defineProperty(navigator, "clipboard", { value: {
      writeText: async (text: string) => {
        (window as TestWindow).copiedText = text;
      },
      read: async () => {
        if ((window as TestWindow).deferClipboard) {
          await new Promise<void>((resolve) => { (window as TestWindow).completeClipboard = resolve; });
        }
        return [{
          types: ["text/plain"],
          getType: async () => new Blob(["Clipboard paste test"], { type: "text/plain" }),
        }];
      },
    } });
  });
});

test("a delayed paste cannot overwrite a different document", async ({ page }) => {
  await page.goto("/?backend=webgl");
  const canvas = page.locator("canvas");
  await expect(canvas).toBeVisible({ timeout: 30_000 });
  await canvas.click({ position: { x: 500, y: 120 } });
  await expect.poll(() => copiedDocument(page)).toContain("Lapis Web prototype");
  await page.evaluate(() => { (window as TestWindow).deferClipboard = true; });
  await page.keyboard.press("Control+v");
  await page.waitForFunction(() => typeof (window as TestWindow).completeClipboard === "function");
  await canvas.click({ position: { x: 80, y: 147 } });
  await page.evaluate(() => (window as TestWindow).completeClipboard!());
  await nextPaint(page);
  await expect.poll(() => copiedDocument(page)).toContain("# Notes");
  await expect.poll(() => copiedDocument(page)).not.toContain("Clipboard paste test");
});

for (const backend of ["auto", "webgl"]) {
  test(`${backend}: Desktop UI supports editing, undo and page-local save`, async ({ page }, testInfo) => {
    const errors: string[] = [];
    page.on("pageerror", (error) => errors.push(error.message));
    await page.goto(`/?backend=${backend}`);
    const canvas = page.locator("canvas");
    await expect(canvas).toBeVisible({ timeout: 30_000 });
    await canvas.click({ position: { x: 500, y: 120 } });
    await expect.poll(() => copiedDocument(page), { timeout: 30_000 }).toContain("Lapis Web prototype");
    await page.keyboard.press("ArrowRight");
    await page.screenshot({ path: testInfo.outputPath("initial.png") });

    await page.keyboard.press("ArrowRight");
    await page.keyboard.type("\nBrowser edit test");
    await expect.poll(() => copiedDocument(page)).toContain("Browser edit test");
    await page.keyboard.press("Control+z");
    await expect.poll(() => copiedDocument(page)).not.toContain("Browser edit test");
    await page.keyboard.press("Control+y");
    await expect.poll(() => copiedDocument(page)).toContain("Browser edit test");
    await page.keyboard.press("ArrowRight");
    await page.keyboard.press("Control+v");
    // 非同期pasteの完了前にCtrl+Aで選択を変えると、意図どおりpasteが取り消される。
    await nextPaint(page);
    await expect.poll(() => copiedDocument(page)).toContain("Clipboard paste test");
    await page.keyboard.press("Control+s");
    // 保存してtabを閉じ、file treeから再度開く。
    await canvas.click({ position: { x: 434, y: 55 } });
    await canvas.click({ position: { x: 80, y: 120 } });
    await canvas.click({ position: { x: 500, y: 120 } });
    await expect.poll(() => copiedDocument(page)).toContain("Browser edit test");
    await page.keyboard.press("ArrowRight");
    await page.screenshot({ path: testInfo.outputPath("edited.png") });

    await page.reload();
    await expect(canvas).toBeVisible({ timeout: 30_000 });
    await canvas.click({ position: { x: 500, y: 120 } });
    await expect.poll(() => copiedDocument(page), { timeout: 30_000 }).toContain("Lapis Web prototype");
    await expect.poll(() => copiedDocument(page)).not.toContain("Browser edit test");
    expect(errors).toEqual([]);
  });
}

test("command search, file switching and layout remain interactive", async ({ page }, testInfo) => {
  const errors: string[] = [];
  page.on("pageerror", (error) => errors.push(error.message));
  await page.goto("/?backend=webgl");
  const canvas = page.locator("canvas");
  await expect(canvas).toBeVisible({ timeout: 30_000 });
  await canvas.click({ position: { x: 500, y: 120 } });
  await expect.poll(() => copiedDocument(page)).toContain("Lapis Web prototype");
  const original = await copiedDocument(page);

  await page.keyboard.press("Control+Shift+k");
  await page.keyboard.type("toggle assistant");
  await page.screenshot({ path: testInfo.outputPath("command-search.png") });
  await page.keyboard.press("Escape");
  await expect.poll(() => copiedDocument(page)).toBe(original);

  await canvas.click({ position: { x: 80, y: 147 } });
  await expect.poll(() => copiedDocument(page)).toContain("# Notes");
  await page.keyboard.press("ArrowRight");
  await page.keyboard.press("Control+j");
  await page.mouse.move(332, 300);
  await page.mouse.down();
  await page.mouse.move(460, 300, { steps: 6 });
  await page.mouse.up();
  await page.screenshot({ path: testInfo.outputPath("panels.png") });

  await page.setViewportSize({ width: 900, height: 700 });
  await expect.poll(async () => (await canvas.boundingBox())?.width).toBe(900);
  await expect.poll(() => copiedDocument(page)).toContain("# Notes");
  await page.screenshot({ path: testInfo.outputPath("resized.png") });
  expect(errors).toEqual([]);
});
