import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import test from 'node:test';
const source = await readFile(new URL('../src/op_ck_bridge.js', import.meta.url), 'utf8');
const { opCkCreateRuntimeEffectCache: cacheFor, opCkRoundedShadowGeometry: geometry,
  opCkDrawRoundedShadow: draw } = await import(`data:text/javascript;base64,${Buffer.from(source).toString('base64')}`);
function engine() {
  const state = { compiled: [], deleted: [], shaders: [], paints: [], draws: [], saves:0, clips:[] };
  const CK = { Color4f:(...c)=>c,RRectXY:(rect,x,y)=>({rect,x,y}),
    PaintStyle:{Fill:1},ClipOp:{Intersect:1},BlendMode:{DstOut:2},BlurStyle:{Normal:1},
    MaskFilter:{MakeBlur:(_kind,sigma,respect)=>({sigma,respect,delete(){state.deleted.push('filter')}})},
    RuntimeEffect:{Make(source){state.compiled.push(source);if(source==='bad')return null;
      const names=['bounds','radius','blur','inner','tint'],arity=[4,1,1,1,4],slots=[0,4,5,6,7];
      return { getUniformCount:()=>5,getUniformName:i=>names[i],
        getUniform:i=>({columns:arity[i],rows:1,slot:slots[i],isInteger:false}),getUniformFloatCount:()=>11,
        makeShader(data){const shader={data:Array.from(data),delete(){state.deleted.push('shader')}};state.shaders.push(shader);return shader;},
        delete(){state.deleted.push(source);} };
    }},
  };
  CK.Paint=class {constructor(){state.paints.push(this)}setColor(v){this.color=v}setAntiAlias(v){this.aa=v}setStyle(v){this.style=v}
    setShader(v){this.shader=v}setMaskFilter(v){this.filter=v}setBlendMode(v){this.blend=v}delete(){state.deleted.push('paint')}};
  const canvas={save(){state.saves++},restore(){state.saves--},saveLayer(){state.saves++},
    clipRRect(v){state.clips.push(v)},drawRect(rect,paint){state.draws.push({rect,paint})},drawRRect(rect,paint){state.draws.push({rect,paint})}};
  return { CK,state,canvas };
}
const values=(inner=false,kernel=1)=>[10,20,100,80,12,3,4,8,-5,.2,.3,.4,.1,inner,kernel];
test('rounded geometry applies signed spread and zero blur retains Gaussian coverage',()=>{
  const shape=geometry(values());assert.deepEqual(shape.bounds,[18,29,108,99]);assert.equal(shape.corner,7);
  const inset=geometry(values(true));assert.deepEqual(inset.bounds,[8,19,118,109]);assert.equal(inset.corner,17);
  const zero=values();zero[7]=0;assert.equal(geometry(zero).kernel,0);
  const collapsed=values();collapsed[8]=-50;assert.equal(geometry(collapsed).bounds,null);
  const invalid=values();invalid[0]=Infinity;assert.equal(geometry(invalid),null);
});
test('triangular shader binds premultiplied color once and restores/disposes transient state',()=>{
  const {CK,state,canvas}=engine(),cache=cacheFor(CK);
  assert.equal(draw(CK,canvas,cache,'triangle',values()),true);
  assert.deepEqual(state.paints[0].color,[1,1,1,1]);
  assert.equal(state.shaders[0].data[10],Math.fround(.1));
  assert.equal(state.shaders[0].data[7],Math.fround(.2*.1));
  assert.equal(state.saves,0);assert.ok(state.deleted.includes('shader'));assert.ok(state.deleted.includes('paint'));
  assert.equal(draw(CK,canvas,cache,'triangle',values(true)),true);
  assert.equal(state.compiled.length,1);assert.equal(state.clips.length,1);assert.equal(state.saves,0);
});
test('Gaussian outer and inset use a CTM-aware half-radius filter with balanced layers',()=>{
  const {CK,state,canvas}=engine(),cache=cacheFor(CK);
  for(const inner of[false,true])assert.equal(draw(CK,canvas,cache,'triangle',values(inner,0)),true);
  assert.equal(state.compiled.length,0);assert.equal(state.saves,0);
  const filters=state.paints.filter(x=>x.filter).map(x=>x.filter);assert.equal(filters.length,2);
  assert.ok(filters.every(x=>x.sigma===4&&x.respect===true));
});
test('shader cache evicts least recently used programs and bounded failed compiles',()=>{
  const {CK,state}=engine(),cache=cacheFor(CK,{maxEntries:2,maxSourceBytes:32});
  cache.get('a');cache.get('b');cache.get('a');cache.get('bad');
  assert.deepEqual(state.deleted,['b']);cache.get('bad');assert.equal(state.compiled.filter(x=>x==='bad').length,1);
  cache.get('c');assert.ok(state.deleted.includes('a'));assert.equal(cache.snapshot().entries,2);
  cache.dispose();assert.equal(cache.snapshot().entries,0);assert.equal(cache.snapshot().sourceBytes,0);
});
test('shader cache enforces source charge before compilation and caps caller budgets',()=>{
  const {CK,state}=engine(),cache=cacheFor(CK,{maxEntries:2,maxSourceBytes:8});
  cache.get('ab');cache.get('cd');cache.get('ef');assert.deepEqual(state.deleted,['ab']);
  assert.equal(cache.get('oversized'),null);assert.equal(state.compiled.length,3);
  assert.equal(cache.snapshot().sourceBytes,8);
  const fixed=cacheFor(CK,{maxEntries:1000,maxSourceBytes:1e9});
  assert.equal(fixed.snapshot().maxEntries,128);assert.equal(fixed.snapshot().maxSourceBytes,2*1024*1024);
});
test('failed shader leaves canvas state unchanged and returns an unsupported result',()=>{
  const {CK,state,canvas}=engine();assert.equal(draw(CK,canvas,cacheFor(CK),'bad',values()),false);
  assert.equal(state.saves,0);assert.equal(state.paints.length,0);
});
