import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import test from 'node:test';

const source = await readFile(new URL('../src/op_ck_bridge.js', import.meta.url), 'utf8');
const { opCkShadowOffsetInLocalSpace: localOffset } = await import(
  `data:text/javascript;base64,${Buffer.from(source).toString('base64')}`
);
const matrix = (a, b, c, d, tx = 0, ty = 0) => [a, b, tx, c, d, ty, 0, 0, 1];
const identity = matrix(1, 0, 0, 1);
const vector = (m, [x, y]) => [m[0] * x + m[1] * y, m[3] * x + m[4] * y];
const multiply = (a, b) => Array.from({ length: 9 }, (_, index) => {
  const row = Math.floor(index / 3), column = index % 3;
  return [0, 1, 2].reduce((sum, k) => sum + a[row * 3 + k] * b[k * 3 + column], 0);
});
const approximately = (actual, expected) => actual.forEach((value, index) => {
  assert.ok(Math.abs(value - expected[index]) <= 2e-5 * Math.max(1, Math.abs(expected[index])),
    `${actual} should equal ${expected}`);
});

test('unrotated shadow keeps exact logical values across DPR and translation', () => {
  for (const dpr of [1, 1.25, 2, 3]) {
    const base = matrix(dpr, 0, 0, dpr);
    const current = matrix(dpr, 0, 0, dpr, 1e8, -1e8);
    assert.deepEqual(Array.from(localOffset(3.25, -6.5, current, base)), [3.25, -6.5]);
  }
});

test('rotated shape maps canvas-down shadow without rotating its device direction', () => {
  const base = matrix(2, 0, 0, 2);
  const rotated = matrix(0, -2, 2, 0, 90, 60);
  assert.deepEqual(Array.from(localOffset(0, 8, rotated, base)), [8, 0]);
});

test('flip then rotate obeys noncommutative Canvas postconcat order', () => {
  const flip = matrix(-1, 0, 0, 1);
  const rotation = matrix(0, -1, 1, 0);
  const base = matrix(2, 0, 0, 2);
  const current = multiply(base, multiply(flip, rotation));
  assert.deepEqual(Array.from(localOffset(3, 5, current, base)), [5, 3]);
  const reversed = multiply(base, multiply(rotation, flip));
  assert.deepEqual(Array.from(localOffset(3, 5, reversed, base)), [-5, -3]);
});

test('nested rotations flips shear and nonuniform scale preserve the initial device vector', () => {
  for (const dpr of [1, 1.5, 2, 3]) {
    for (const angle of [-2.4, -0.3, 0, 0.7, 2.1]) {
      const base = matrix(dpr * 1.25, dpr * 0.125, dpr * -0.2, dpr * 0.75);
      const rotation = matrix(Math.cos(angle), -Math.sin(angle), Math.sin(angle), Math.cos(angle));
      const shape = multiply(rotation, matrix(-2, 0.35, 0, 0.5));
      const current = multiply(base, shape);
      current[2] = 12345; current[5] = -6789;
      const offset = [-7.25, 4.5];
      const local = Array.from(localOffset(...offset, current, base));
      approximately(vector(current, local), vector(base, offset));
    }
  }
});

test('invalid singular perspective or overflowing transforms fall back to the original vector', () => {
  const invalid = [
    null, [], matrix(0, 0, 0, 0), matrix(1, 2, 2, 4),
    matrix(Number.NaN, 0, 0, 1), matrix(1, 0, 0, Number.POSITIVE_INFINITY),
    [1, 0, 0, 0, 1, 0, 0.1, 0, 1], matrix(1e-40, 0, 0, 1e-40),
  ];
  for (const current of invalid) {
    assert.deepEqual(Array.from(localOffset(10, -5, current, identity)), [10, -5]);
  }
  assert.deepEqual(Array.from(localOffset(10, -5, matrix(0, -1, 1, 0), matrix(Infinity, 0, 0, 1))), [10, -5]);
});
