import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { spawnSync } from 'node:child_process';
import { chromium } from 'playwright';

test('measures pixel error, preserves transparent overlays and rejects dimension mismatches', async () => {
  const directory = fs.mkdtempSync(path.join(os.tmpdir(), 'openpencil-png-compare-'));
  const browser = await chromium.launch({ headless: true });
  try {
    const page = await browser.newPage();
    const fixtures = await page.evaluate(() => {
      function png(width, color, transparent = false) {
        const canvas = document.createElement('canvas');
        canvas.width = width;
        canvas.height = 16;
        const context = canvas.getContext('2d');
        context.fillStyle = color;
        context.fillRect(0, 0, width, 16);
        if (transparent) context.clearRect(0, 0, 4, 4);
        return canvas.toDataURL('image/png');
      }
      return { black: png(16, '#000'), white: png(16, '#fff'), alpha: png(16, 'rgba(200,100,50,.5)', true), different: png(24, '#fff') };
    });
    for (const [name, data] of Object.entries(fixtures)) {
      fs.writeFileSync(path.join(directory, `${name}.png`), Buffer.from(data.split(',')[1], 'base64'));
    }
    const script = fileURLToPath(new URL('./compare-png.mjs', import.meta.url));
    function compare(left, right, name) {
      const prefix = path.join(directory, name);
      const run = spawnSync(process.execPath, [script, path.join(directory, `${left}.png`), path.join(directory, `${right}.png`), prefix], { encoding: 'utf8', timeout: 30000 });
      return { ...run, prefix };
    }
    const contrast = compare('black', 'white', 'contrast');
    assert.equal(contrast.status, 0, contrast.stderr);
    const metrics = JSON.parse(fs.readFileSync(`${contrast.prefix}.json`, 'utf8'));
    assert.equal(metrics.meanAbsoluteRgbError, 255);
    assert.equal(metrics.rootMeanSquaredRgbError, 255);
    assert.equal(metrics.fractionOverDelta8, 1);
    assert.equal(metrics.p95MaxChannelDelta, 255);
    assert.equal(metrics.meanAbsoluteAlphaError, 0);
    assert.ok(Math.abs(metrics.ssimLuma - 6.5025 / (255 ** 2 + 6.5025)) < 1e-10);
    assert.equal(metrics.resized, false);
    assert.match(metrics.comparatorSha256, /^[a-f0-9]{64}$/);

    const identical = compare('alpha', 'alpha', 'identical');
    assert.equal(identical.status, 0, identical.stderr);
    const identity = JSON.parse(fs.readFileSync(`${identical.prefix}.json`, 'utf8'));
    assert.equal(identity.changedPixels, 0);
    assert.equal(identity.meanAbsoluteAlphaError, 0);
    assert.ok(Math.abs(identity.ssimLuma - 1) < 1e-10);
    const overlay = fs.readFileSync(`${identical.prefix}-overlay.png`).toString('base64');
    const overlayAlpha = await page.evaluate(async (base64) => {
      const image = new Image();
      image.src = `data:image/png;base64,${base64}`;
      await image.decode();
      const canvas = document.createElement('canvas');
      canvas.width = canvas.height = 16;
      const context = canvas.getContext('2d');
      context.drawImage(image, 0, 0);
      return [context.getImageData(0, 0, 1, 1).data[3], context.getImageData(8, 8, 1, 1).data[3]];
    }, overlay);
    assert.deepEqual(overlayAlpha, [0, 128]);

    const mismatch = compare('black', 'different', 'mismatch');
    assert.equal(mismatch.status, 1);
    assert.match(mismatch.stderr, /Image dimensions differ/);
    assert.equal(fs.existsSync(`${mismatch.prefix}.json`), false);
  } finally {
    await browser.close();
    fs.rmSync(directory, { recursive: true, force: true });
  }
});
