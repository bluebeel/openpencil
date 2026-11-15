// Bounded immutable string/scalar tuple witnesses. Hashes only select an entry;
// exact parts (including signed zero) authorize reuse, never delimiter strings.
export function createValueCache(maxEntries,maxBytes,dispose=()=>{}) {
  const entries=new Map(),scratch=new DataView(new ArrayBuffer(8));let bytes=0;
  const release=value=>{try{dispose(value);}catch(_cleanupError){}};
  const request=parts=>{
    let hash=2166136261,storage=128;const word=n=>{hash=Math.imul(hash^n,16777619);};
    word(parts.length);
    for(const value of parts){
      if(typeof value==='string'){
        storage+=value.length*2+16;if(storage>8192)return null;
        word(1);word(value.length);for(let i=0;i<value.length;i++)word(value.charCodeAt(i));
      }else if(typeof value==='number'&&Number.isFinite(value)){
        storage+=16;word(2);scratch.setFloat64(0,value,true);word(scratch.getUint32(0,true));word(scratch.getUint32(4,true));
      }else if(typeof value==='boolean'){storage+=8;word(value?3:4);}
      else return null;
    }
    return {hash,storage};
  };
  const remove=hash=>{const entry=entries.get(hash);if(!entry)return;entries.delete(hash);bytes-=entry.bytes;release(entry.value);};
  return {
    get(parts){
      const key=request(parts);if(!key)return undefined;
      const entry=entries.get(key.hash);
      if(!entry||entry.parts.length!==parts.length||!parts.every((value,i)=>Object.is(value,entry.parts[i])))return undefined;
      entries.delete(key.hash);entries.set(key.hash,entry);return entry.value;
    },
    set(parts,value,payloadBytes=0){
      const key=request(parts),owned=key&&key.storage+payloadBytes;
      if(!key||!Number.isSafeInteger(owned)||owned<0||owned>maxBytes||maxEntries<1)return false;
      try {
        const witness=parts.slice();remove(key.hash);
        while(entries.size>=maxEntries||bytes>maxBytes-owned)remove(entries.keys().next().value);
        entries.set(key.hash,{parts:witness,value,bytes:owned});bytes+=owned;return true;
      }catch(_admissionFailure){return false;}
    },
    clear(){for(const entry of entries.values())release(entry.value);entries.clear();bytes=0;},
  };
}
