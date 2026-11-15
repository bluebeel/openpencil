// Exact local-gradient reuse owned by one live CanvasKit bridge. Returned
// clone() wrappers preserve existing draw-site delete() ownership.
const MAX_ENTRIES=256,MAX_BYTES=4*1024*1024,MAX_STOPS=256;
const release=value=>{try{value?.delete?.();}catch(_cleanupError){}};
export function createGradientShaderCache() {
  const entries=new Map(),scratch=new DataView(new ArrayBuffer(8));let retained=0;
  const hashRequest=(kind,geometry,stops,opacity)=>{
    let hash=2166136261;
    const word=n=>{hash=Math.imul(hash^n,16777619);};
    const number=n=>{
      if(!Number.isFinite(n))return false;
      scratch.setFloat64(0,n,true);word(scratch.getUint32(0,true));word(scratch.getUint32(4,true));return true;
    };
    word(kind);word(geometry.length);word(stops.length);
    if(!number(opacity))return null;
    for(const n of geometry)if(!number(n))return null;
    for(const n of stops)if(!number(n))return null;
    return hash;
  };
  const matches=(entry,kind,geometry,stops,opacity)=>{
    if(entry.kind!==kind||entry.geometry!==geometry.length||entry.stops!==stops.length||!Object.is(entry.key[0],opacity))return false;
    for(let i=0;i<geometry.length;i++)if(!Object.is(entry.key[i+1],geometry[i]))return false;
    for(let i=0;i<stops.length;i++)if(!Object.is(entry.key[i+1+geometry.length],stops[i]))return false;
    return true;
  };
  const remove=hash=>{
    const entry=entries.get(hash);if(!entry)return;
    entries.delete(hash);retained-=entry.bytes;release(entry.shader);
  };
  return {
    get(kind,geometry,stops,opacity,build) {
      const hash=stops.length<=MAX_STOPS*5?hashRequest(kind,geometry,stops,opacity):null;
      const entry=hash===null?null:entries.get(hash);
      if(entry&&matches(entry,kind,geometry,stops,opacity)) {
        try{return entry.shader.clone();}catch(_staleOwner){remove(hash);}
      }
      const shader=build();
      if(!shader||hash===null||typeof shader.clone!=='function')return shader;
      let owner=null;
      try {
        const key=new Float64Array(1+geometry.length+stops.length);key[0]=opacity;
        key.set(geometry,1);key.set(stops,1+geometry.length);
        const bytes=key.byteLength+Math.ceil(stops.length/5)*64+512;
        if(bytes>MAX_BYTES)return shader;
        owner=shader.clone();
        remove(hash);
        while(entries.size>=MAX_ENTRIES||retained>MAX_BYTES-bytes)remove(entries.keys().next().value);
        entries.set(hash,{kind,geometry:geometry.length,stops:stops.length,key,bytes,shader:owner});
        retained+=bytes;owner=null;
      }catch(_cacheAdmissionFailure){release(owner);}
      return shader;
    },
    dispose(){for(const entry of entries.values())release(entry.shader);entries.clear();retained=0;},
  };
}
