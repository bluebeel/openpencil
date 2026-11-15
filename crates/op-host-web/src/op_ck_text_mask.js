// A single bounded lexical recording; never a global/font-size bounds cache.
const LIMIT = 16 * 1024 * 1024;
const bitsEqual = (a, b) => a?.length === b?.length
  && Array.from(a).every((v, i) => Object.is(v, b[i]));
const finiteAffine = m => m?.length === 9 && Array.from(m).every(Number.isFinite)
  && m[6] === 0 && m[7] === 0 && m[8] === 1 && m[0] * m[4] - m[1] * m[3] !== 0;
function observedSupport(bounds, clip) {
  if (!bounds || bounds.length !== 4 || !Array.from(bounds).every(Number.isFinite)) return null;
  // Include the entire touched device pixel cells; this comes from recorded
  // draw support, not an em-size or line-box estimate.
  const [left, top, right, bottom] = [Math.max(Math.floor(bounds[0]), clip[0]),
    Math.max(Math.floor(bounds[1]), clip[1]), Math.min(Math.ceil(bounds[2]), clip[2]),
    Math.min(Math.ceil(bounds[3]), clip[3])];
  if (!(right > left && bottom > top)) return [0, 0, 0, 0];
  return [left, top, right, bottom];
}
function localBounds(matrix, bounds, clip) {
  const support = observedSupport(bounds, clip);
  if (!support) return null;
  const [left, top, right, bottom] = support;
  if (!(right > left && bottom > top)) return [0, 0, 0, 0];
  const m = matrix, det = m[0] * m[4] - m[1] * m[3];
  const points = [[left, top], [right, top], [left, bottom], [right, bottom]].map(([x, y]) => {
    x -= m[2]; y -= m[5];
    return [(m[4] * x - m[1] * y) / det, (m[0] * y - m[3] * x) / det];
  });
  const f = new Float32Array(1), bits = new Uint32Array(f.buffer);
  const outward = (v, up) => {
    f[0] = v;
    if (!Number.isFinite(f[0])) return NaN;
    if (f[0] === 0) bits[0] = up ? 1 : 0x80000001;
    else bits[0] += ((f[0] > 0) === up) ? 1 : -1;
    return f[0];
  };
  const l = outward(Math.min(...points.map(v => v[0])), false);
  const t = outward(Math.min(...points.map(v => v[1])), false);
  const r = outward(Math.max(...points.map(v => v[0])), true);
  const b = outward(Math.max(...points.map(v => v[1])), true);
  const result = [l, t, outward(r - l, true), outward(b - t, true)];
  return result.every(Number.isFinite) ? result : null;
}
// Exactly the existing browser-mask placement, shared by recording/replay and
// the unprepared path. Preparing the image/font is not repeated during replay.
export function drawTextMaskImage(canvas, entry, paint, x, y) {
  const ss = entry.ss || 1;
  if (ss !== 1) {
    canvas.save();
    canvas.translate(x - 2, y - entry.baseline);
    canvas.scale(1 / ss, 1 / ss);
    if (paint) canvas.drawImage(entry.image, 0, 0, paint);
    else canvas.drawImage(entry.image, 0, 0);
    canvas.restore();
  } else if (paint) canvas.drawImage(entry.image, x - 2, y - entry.baseline, paint);
  else canvas.drawImage(entry.image, x - 2, y - entry.baseline);
}
export function createTextMaskCapture(CK, getCanvas, setCanvas, dimensions, generation) {
  let state = null;
  const empty = () => Float32Array.of(0);
  function clip(canvas) {
    if (typeof canvas.getDeviceClipBounds === 'function') {
      const r = canvas.getDeviceClipBounds();
      if (r?.length === 4 && Array.from(r).every(Number.isFinite)) return Array.from(r);
    }
    const [w, h] = dimensions();
    return [0, 0, w, h];
  }
  function provenLocalBounds(s) {
    if (s.unproven || typeof s.picture.cullRect !== 'function') return null;
    const raw = s.picture.cullRect(), target = observedSupport(raw, s.clip);
    const area = localBounds(s.matrix, raw, s.clip);
    if (!target || !area || (area[2] === 0 && area[3] === 0)) return area;
    // Verify inverse-rounding using Skia's actual recorded rect map, rather
    // than guessing JS-vs-wasm matrix/FMA rounding. This tiny picture has no
    // glyphs, images or retained payload and is discarded immediately.
    const recorder = new CK.PictureRecorder(), paint = new CK.Paint();
    let picture = null;
    try {
      const c = recorder.beginRecording(CK.LTRBRect(...s.clip), true);
      if (!c) return null;
      c.concat(s.matrix); paint.setAntiAlias(false);
      c.drawRect(CK.XYWHRect(...area), paint);
      picture = recorder.finishRecordingAsPicture();
      if (!picture || typeof picture.cullRect !== 'function') return null;
      const mapped = picture.cullRect();
      return mapped[0] <= target[0] && mapped[1] <= target[1]
        && mapped[2] >= target[2] && mapped[3] >= target[3] ? area : null;
    } finally {
      if (picture) picture.delete();
      paint.delete(); recorder.delete();
    }
  }
  function release(s) {
    if (s.recorder?.delete) s.recorder.delete();
    if (s.picture?.delete) s.picture.delete();
    for (const packet of s.packets) packet.release();
  }
  function cancel() {
    const s = state;
    state = null;
    if (!s) return;
    if (s.phase === 'recording') setCanvas(s.original);
    release(s);
  }
  function charge(bytes) {
    const s = state;
    if (s?.phase !== 'recording') return true;
    s.bytes += bytes;
    s.draws++;
    s.discarded ||= !Number.isSafeInteger(s.bytes) || s.bytes > LIMIT || s.draws > 4096;
    return !s.discarded;
  }
  function append(draw, dispose) {
    state.packets.push({ draw, release: dispose });
    draw(getCanvas());
  }
  return {
    cancel,
    skipping: () => state?.phase === 'recording' && state.discarded,
    text: t => charge(String(t).length * 512 + 1024),
    image(entry, paint, x, y) {
      const s = state;
      if (s?.phase !== 'recording') return false;
      if (!charge(entry.image.width() * entry.image.height() * 4 + 1024)) return true;
      // Embind handle clones pin immutable resources independently of LRU
      // eviction; Paint.copy snapshots the otherwise shared mutable tint.
      if (typeof entry.image.clone !== 'function' || (paint && typeof paint.copy !== 'function')) {
        s.discarded = true;
        return true;
      }
      const image = entry.image.clone(), tint = paint?.copy() || null;
      const prepared = { image, ss: entry.ss, baseline: entry.baseline };
      s.unproven ||= Boolean(entry.emoji);
      append(c => drawTextMaskImage(c, prepared, tint, x, y), () => {
        if (tint) tint.delete();
        image.delete();
      });
      return true;
    },
    glyphs(text, x, y, paint, font) {
      const s = state;
      if (s?.phase !== 'recording') return false;
      // Legacy platform text/color fonts have no proven ink support here.
      // Retain the same prepared font/paint, but keep the full128 layer bounds.
      s.unproven = true;
      if (!charge(text.length * 512 + 1024)) return true;
      if (typeof font.clone !== 'function' || typeof paint.copy !== 'function') {
        s.discarded = true;
        return true;
      }
      const retained = font.clone(), ink = paint.copy();
      append(c => c.drawText(text, x, y, ink, retained), () => {
        ink.delete(); retained.delete();
      });
      return true;
    },
    line(x1, y1, x2, y2, paint) {
      const s = state;
      if (s?.phase !== 'recording') return false;
      if (!charge(1024)) return true;
      if (typeof paint.copy !== 'function') { s.discarded = true; return true; }
      const ink = paint.copy();
      // BBH observes this exact stroke/cap/join paint, including decoration
      // endpoints; no line-height or font-metric proxy is used for its support.
      append(c => c.drawLine(x1, y1, x2, y2, ink), () => ink.delete());
      return true;
    },
    command(action) {
      if (action === 3) { cancel(); return empty(); }
      if (action === 0) {
        if (state) return empty();
        const original = getCanvas(), matrix = Float32Array.from(original.getTotalMatrix());
        if (!finiteAffine(matrix) || typeof CK.PictureRecorder !== 'function') return empty();
        const bounds = clip(original);
        if (!bounds.every(Number.isFinite) || !(bounds[2] > bounds[0] && bounds[3] > bounds[1])) return empty();
        const recorder = new CK.PictureRecorder();
        const recorded = recorder.beginRecording(CK.LTRBRect(...bounds), true);
        if (!recorded) { recorder.delete(); return empty(); }
        recorded.concat(matrix);
        if (!bitsEqual(matrix, recorded.getTotalMatrix())) {
          recorder.delete(); return empty();
        }
        state = { phase: 'recording', original, matrix, clip: bounds, recorder, packets: [],
          generation: generation(), bytes: 1024, draws: 0, unproven: false, discarded: false };
        setCanvas(recorded);
        return Float32Array.of(1);
      }
      if (action === 1) {
        const s = state;
        if (s?.phase !== 'recording') return empty();
        setCanvas(s.original);
        state = null;
        try { s.picture = s.recorder.finishRecordingAsPicture(); }
        catch (_) { release(s); return empty(); }
        s.recorder.delete(); s.recorder = null;
        if (!s.picture || typeof s.picture.approximateBytesUsed !== 'function') {
          release(s); return empty();
        }
        s.bytes += s.picture.approximateBytesUsed();
        if (s.discarded || s.bytes > LIMIT || s.generation !== generation()) {
          release(s); return empty();
        }
        const bounds = provenLocalBounds(s);
        s.phase = 'ready'; state = s;
        return bounds ? Float32Array.of(3, ...bounds) : Float32Array.of(2);
      }
      if (action === 2) {
        const s = state;
        if (s?.phase !== 'ready') return empty();
        state = null;
        const canvas = getCanvas(), bounds = clip(canvas);
        const expanded = bounds[0] < s.clip[0] || bounds[1] < s.clip[1]
          || bounds[2] > s.clip[2] || bounds[3] > s.clip[3];
        // No inverse concatenation: CK has no set/resetMatrix. Re-execute the
        // immutable prepared draw packets at the exact unchanged original CTM,
        // including each image's original translate/scale sequence. The final
        // canvas still owns its complete complex clip, never the recorder bbox.
        if (canvas !== s.original || s.generation !== generation() || expanded
          || !bitsEqual(s.matrix, canvas.getTotalMatrix())) { release(s); return empty(); }
        try { for (const packet of s.packets) packet.draw(canvas); }
        finally { release(s); }
        return Float32Array.of(4);
      }
      return empty();
    },
  };
}
