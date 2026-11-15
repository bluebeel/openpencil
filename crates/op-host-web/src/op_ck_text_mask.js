// A single bounded lexical recording; never a global/font-size bounds cache.
const LIMIT = 16 * 1024 * 1024;
const releaseHandle=value=>{try{value?.delete?.();}catch(_cleanupError){}};
const bitsEqual=(a,b)=>{
  if(!a||!b||a.length!==b.length)return false;
  for(let i=0;i<a.length;i++)if(!Object.is(a[i],b[i]))return false;
  return true;
};
const finiteAffine=m=>{
  if(m?.length!==9)return false;
  for(let i=0;i<9;i++)if(!Number.isFinite(m[i]))return false;
  return m[6]===0&&m[7]===0&&m[8]===1&&m[0]*m[4]-m[1]*m[3]!==0;
};
function observedSupport(bounds, clip) {
  if(!bounds||bounds.length!==4)return null;
  for(let i=0;i<4;i++)if(!Number.isFinite(bounds[i]))return null;
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
  let minX=Infinity,minY=Infinity,maxX=-Infinity,maxY=-Infinity;
  for(let i=0;i<4;i++){
    const x=(i%2===0?left:right)-m[2],y=(i<2?top:bottom)-m[5];
    const px=(m[4]*x-m[1]*y)/det,py=(m[0]*y-m[3]*x)/det;
    minX=Math.min(minX,px);minY=Math.min(minY,py);
    maxX=Math.max(maxX,px);maxY=Math.max(maxY,py);
  }
  const f = new Float32Array(1), bits = new Uint32Array(f.buffer);
  const outward = (v, up) => {
    f[0] = v;
    if (!Number.isFinite(f[0])) return NaN;
    if (f[0] === 0) bits[0] = up ? 1 : 0x80000001;
    else bits[0] += ((f[0] > 0) === up) ? 1 : -1;
    return f[0];
  };
  const l = outward(minX, false);
  const t = outward(minY, false);
  const r = outward(maxX, true);
  const b = outward(maxY, true);
  const result = [l, t, outward(r - l, true), outward(b - t, true)];
  return result.every(Number.isFinite) ? result : null;
}
// Exactly the existing browser-mask placement, shared by recording/replay and
// the unprepared path. Preparing the image/font is not repeated during replay.
export function drawTextMaskImage(canvas, entry, paint, x, y) {
  const ss = entry.ss || 1;
  const offsetX=entry.offsetX??2;
  if (ss !== 1) {
    canvas.save();
    try {
      canvas.translate(x-offsetX,y-entry.baseline);
      canvas.scale(1/ss,1/ss);
      if(paint)canvas.drawImage(entry.image,0,0,paint);
      else canvas.drawImage(entry.image,0,0);
    }finally{canvas.restore();}
  }else if(paint)canvas.drawImage(entry.image,x-offsetX,y-entry.baseline,paint);
  else canvas.drawImage(entry.image,x-offsetX,y-entry.baseline);
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
    let recorder=null,paint=null,picture=null;
    try {
      recorder=new CK.PictureRecorder();paint=new CK.Paint();
      const c = recorder.beginRecording(CK.LTRBRect(...s.clip), true);
      if (!c) return null;
      c.concat(s.matrix); paint.setAntiAlias(false);
      c.drawRect(CK.XYWHRect(...area), paint);
      picture = recorder.finishRecordingAsPicture();
      if (!picture || typeof picture.cullRect !== 'function') return null;
      const mapped = picture.cullRect();
      return mapped[0] <= target[0] && mapped[1] <= target[1]
        && mapped[2] >= target[2] && mapped[3] >= target[3] ? area : null;
    }catch(_boundsRefusal){return null;}
    finally {releaseHandle(picture);releaseHandle(paint);releaseHandle(recorder);}
  }
  function release(s) {
    releaseHandle(s.recorder);releaseHandle(s.picture);
    for(const packet of s.packets){try{packet.release();}catch(_cleanupError){}}
  }
  function cancel() {
    const s = state;
    state = null;
    if (!s) return;
    try{if(s.phase==='recording')setCanvas(s.original);}
    finally{release(s);}
  }
  function charge(bytes) {
    const s = state;
    if (s?.phase !== 'recording') return true;
    s.bytes += bytes;
    s.draws++;
    s.discarded ||= !Number.isSafeInteger(s.bytes) || s.bytes > LIMIT || s.draws > 4096;
    return !s.discarded;
  }
  function append(draw,dispose) {
    const packets=state.packets,packet={draw,release:dispose};
    try {packets.push(packet);draw(getCanvas());}
    catch(error){
      if(state)state.discarded=true;
      const index=packets.lastIndexOf(packet);if(index>=0)packets.splice(index,1);
      dispose();throw error;
    }
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
      let image=null,tint=null;
      try {
        image=entry.image.clone();tint=paint?.copy()||null;
        const prepared={image,ss:entry.ss,baseline:entry.baseline,offsetX:entry.offsetX};
        const ink=tint,pinned=image;image=null;tint=null;
        s.unproven ||= Boolean(entry.emoji);
        append(c=>drawTextMaskImage(c,prepared,ink,x,y),()=>{
          releaseHandle(ink);releaseHandle(pinned);
        });
      }finally{releaseHandle(tint);releaseHandle(image);}
      return true;
    },
    glyphs(text,x,y,paint,font,typeface=null,size=0,skew=0) {
      const s = state;
      if (s?.phase !== 'recording') return false;
      // Legacy platform text/color fonts have no proven ink support here.
      // Retain the same prepared font/paint, but keep the full128 layer bounds.
      s.unproven = true;
      if (!charge(text.length * 512 + 1024)) return true;
      const exactConstructor=typeface&&Number.isFinite(size)&&Number.isFinite(skew)&&typeof CK.Font==='function';
      if ((typeof font.copy!=='function'&&!exactConstructor)||typeof paint.copy!=='function') {
        s.discarded = true;
        return true;
      }
      let retained=null,ink=null;
      try {
        // Embind clone shares a mutable Font object. A real native copy or
        // the actual caller's default-constructor+skew receipt is required.
        if(typeof font.copy==='function')retained=font.copy();
        else {retained=new CK.Font(typeface,size);if(skew)retained.setSkewX(skew);}
        ink=paint.copy();
        const pinned=retained,color=ink;retained=null;ink=null;
        append(c=>c.drawText(text,x,y,color,pinned),()=>{
          releaseHandle(color);releaseHandle(pinned);
        });
      }finally{releaseHandle(ink);releaseHandle(retained);}
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
      append(c => c.drawLine(x1, y1, x2, y2, ink), () => releaseHandle(ink));
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
        let recorder=null;
        try {
          recorder=new CK.PictureRecorder();
          const recorded=recorder.beginRecording(CK.LTRBRect(...bounds),true);
          if(!recorded)return empty();
          recorded.concat(matrix);
          if(!bitsEqual(matrix,recorded.getTotalMatrix()))return empty();
          const result=Float32Array.of(1);
          state={phase:'recording',original,matrix,clip:bounds,recorder,packets:[],
            generation:generation(),bytes:1024,draws:0,unproven:false,discarded:false};
          setCanvas(recorded);recorder=null;return result;
        }catch(_recordRefusal){state=null;try{setCanvas(original);}catch(_restoreRefusal){}return empty();}
        finally{releaseHandle(recorder);}
      }
      if (action === 1) {
        const s = state;
        if (s?.phase !== 'recording') return empty();
        setCanvas(s.original);
        state = null;
        try { s.picture = s.recorder.finishRecordingAsPicture(); }
        catch (_) { release(s); return empty(); }
        releaseHandle(s.recorder);s.recorder=null;
        if (!s.picture || typeof s.picture.approximateBytesUsed !== 'function') {
          release(s); return empty();
        }
        try{s.bytes+=s.picture.approximateBytesUsed();}
        catch(_sizeRefusal){release(s);return empty();}
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
        try {
          const canvas=getCanvas(),bounds=clip(canvas);
          const expanded=bounds[0]<s.clip[0]||bounds[1]<s.clip[1]||bounds[2]>s.clip[2]||bounds[3]>s.clip[3];
          if(canvas!==s.original||s.generation!==generation()||expanded||!bitsEqual(s.matrix,canvas.getTotalMatrix()))return empty();
          for(const packet of s.packets)packet.draw(canvas);
          return Float32Array.of(4);
        }finally{release(s);}
      }
      return empty();
    },
  };
}
