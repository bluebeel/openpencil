import fs from 'node:fs';
import path from 'node:path';
import { createHash } from 'node:crypto';
import { chromium } from 'playwright';

const [referenceFile, candidateFile, outputPrefix] = process.argv.slice(2);
if (!referenceFile || !candidateFile || !outputPrefix) throw new Error('Usage: compare-png.mjs reference.png candidate.png output-prefix');
const dataUrl = (filename) => `data:image/png;base64,${fs.readFileSync(filename).toString('base64')}`;
const fileHash = (filename) => createHash('sha256').update(fs.readFileSync(filename)).digest('hex');
const browser = await chromium.launch({ headless: true });
try {
  const page = await browser.newPage();
  const result = await page.evaluate(async ({ reference, candidate }) => {
    async function load(url) {
      const image = new Image();
      image.src = url;
      await image.decode();
      const canvas = document.createElement('canvas');
      canvas.width = image.width;
      canvas.height = image.height;
      const context = canvas.getContext('2d', { colorSpace: 'srgb', willReadFrequently: true });
      context.drawImage(image, 0, 0);
      return { image, canvas, context, pixels: context.getImageData(0, 0, image.width, image.height).data };
    }
    const a = await load(reference);
    const b = await load(candidate);
    const width = a.image.width;
    const height = a.image.height;
    if (width !== b.image.width || height !== b.image.height) throw new Error(`Image dimensions differ: ${width}x${height} versus ${b.image.width}x${b.image.height}`);
    if (width < 11 || height < 11) throw new Error('SSIM comparison requires images at least 11x11');
    const count = width * height;
    const left = new Float64Array(count);
    const right = new Float64Array(count);
    const difference = new Uint8ClampedArray(count * 4);
    const overlayPixels = new Uint8ClampedArray(count * 4);
    let absolute = 0;
    let squared = 0;
    let alphaAbsolute = 0;
    let changed = 0;
    let over8 = 0;
    const histogram = new Uint32Array(256);
    for (let pixel = 0; pixel < count; pixel++) {
      const offset = pixel * 4;
      let maximum = 0;
      const colors = [[], []];
      const alphaSum = a.pixels[offset + 3] + b.pixels[offset + 3];
      for (let channel = 0; channel < 3; channel++) {
        const delta = Math.abs(a.pixels[offset + channel] - b.pixels[offset + channel]);
        absolute += delta;
        squared += delta * delta;
        maximum = Math.max(maximum, delta);
        overlayPixels[offset + channel] = alphaSum ? (a.pixels[offset + channel] * a.pixels[offset + 3] + b.pixels[offset + channel] * b.pixels[offset + 3]) / alphaSum : 0;
        colors[0].push(a.pixels[offset + channel] * a.pixels[offset + 3] / 255 + 255 - a.pixels[offset + 3]);
        colors[1].push(b.pixels[offset + channel] * b.pixels[offset + 3] / 255 + 255 - b.pixels[offset + 3]);
      }
      const alphaDelta = Math.abs(a.pixels[offset + 3] - b.pixels[offset + 3]);
      overlayPixels[offset + 3] = alphaSum / 2;
      alphaAbsolute += alphaDelta;
      maximum = Math.max(maximum, alphaDelta);
      histogram[maximum]++;
      if (maximum > 0) changed++;
      if (maximum > 8) over8++;
      left[pixel] = 0.2126 * colors[0][0] + 0.7152 * colors[0][1] + 0.0722 * colors[0][2];
      right[pixel] = 0.2126 * colors[1][0] + 0.7152 * colors[1][1] + 0.0722 * colors[1][2];
      const strength = Math.min(255, maximum * 4);
      difference[offset] = 255;
      difference[offset + 1] = 255 - strength;
      difference[offset + 2] = 255 - strength;
      difference[offset + 3] = 255;
    }
    const weights = Array.from({ length: 11 }, (_, index) => Math.exp(-((index - 5) ** 2) / (2 * 1.5 ** 2)));
    const weightSum = weights.reduce((sum, value) => sum + value, 0);
    for (let index = 0; index < weights.length; index++) weights[index] /= weightSum;
    function blur(input) {
      const horizontal = new Float64Array(count);
      const output = new Float64Array(count);
      for (let y = 0; y < height; y++) {
        for (let x = 0; x < width; x++) {
          let value = 0;
          for (let tap = -5; tap <= 5; tap++) value += input[y * width + Math.max(0, Math.min(width - 1, x + tap))] * weights[tap + 5];
          horizontal[y * width + x] = value;
        }
      }
      for (let y = 0; y < height; y++) {
        for (let x = 0; x < width; x++) {
          let value = 0;
          for (let tap = -5; tap <= 5; tap++) value += horizontal[Math.max(0, Math.min(height - 1, y + tap)) * width + x] * weights[tap + 5];
          output[y * width + x] = value;
        }
      }
      return output;
    }
    const meanLeft = blur(left);
    const meanRight = blur(right);
    const product = new Float64Array(count);
    const leftSquared = new Float64Array(count);
    const rightSquared = new Float64Array(count);
    for (let index = 0; index < count; index++) {
      product[index] = left[index] * right[index];
      leftSquared[index] = left[index] ** 2;
      rightSquared[index] = right[index] ** 2;
    }
    const momentProduct = blur(product);
    const momentLeft = blur(leftSquared);
    const momentRight = blur(rightSquared);
    let ssimSum = 0;
    let windows = 0;
    for (let y = 5; y < height - 5; y++) {
      for (let x = 5; x < width - 5; x++) {
        const i = y * width + x;
        const l = meanLeft[i];
        const r = meanRight[i];
        const varianceLeft = Math.max(0, momentLeft[i] - l * l);
        const varianceRight = Math.max(0, momentRight[i] - r * r);
        const covariance = momentProduct[i] - l * r;
        ssimSum += ((2 * l * r + 6.5025) * (2 * covariance + 58.5225)) / ((l * l + r * r + 6.5025) * (varianceLeft + varianceRight + 58.5225));
        windows++;
      }
    }
    a.context.putImageData(new ImageData(difference, width, height), 0, 0);
    const heatmap = a.canvas.toDataURL('image/png');
    a.context.putImageData(new ImageData(overlayPixels, width, height), 0, 0);
    const overlay = a.canvas.toDataURL('image/png');
    let cumulative = 0;
    let p95 = 0;
    for (; p95 < 255; p95++) {
      cumulative += histogram[p95];
      if (cumulative >= count * 0.95) break;
    }
    return {
      metrics: { width, height, pixels: count, changedPixels: changed, changedFraction: changed / count, pixelsOverDelta8: over8, fractionOverDelta8: over8 / count, meanAbsoluteRgbError: absolute / (count * 3), rootMeanSquaredRgbError: Math.sqrt(squared / (count * 3)), meanAbsoluteAlphaError: alphaAbsolute / count, p95MaxChannelDelta: p95, ssimLuma: ssimSum / windows, ssimMethod: 'sRGB luma composited on white; Gaussian11x11 sigma1.5; population moments; 5px border excluded', resized: false },
      heatmap,
      overlay,
    };
  }, { reference: dataUrl(referenceFile), candidate: dataUrl(candidateFile) });
  fs.mkdirSync(path.dirname(outputPrefix), { recursive: true });
  for (const kind of ['heatmap', 'overlay']) fs.writeFileSync(`${outputPrefix}-${kind}.png`, Buffer.from(result[kind].split(',')[1], 'base64'));
  const metrics = { reference: path.resolve(referenceFile), candidate: path.resolve(candidateFile), referenceSha256: fileHash(referenceFile), candidateSha256: fileHash(candidateFile), comparatorSha256: fileHash(new URL(import.meta.url)), browserVersion: browser.version(), ...result.metrics };
  fs.writeFileSync(`${outputPrefix}.json`, JSON.stringify(metrics, null, 2) + '\n');
  console.log(JSON.stringify(metrics));
} finally {
  await browser.close();
}
