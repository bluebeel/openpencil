// Fonts loaded only into CanvasKit are invisible to browser2D shaping. Keep
// binary aliases private to one bridge and ignore stale/disposed completions.
const key=Symbol.for('openpencil.private-browser-font-names.v1');
if(!globalThis[key])Object.defineProperty(globalThis,key,{value:{next:0n,names:new Set()}});
const names=globalThis[key];
const quote=name=>'"'+name.replace(/["\\\n\r\f]/g,ch=>ch==='"'||ch==='\\'?'\\'+ch:'\\'+ch.charCodeAt(0).toString(16)+' ')+'"';
export function createBrowserFontAliases(changed) {
  const records=new Set();let closed=false;
  const remove=record=>{
    if(!record||!record.active)return;
    record.active=false;records.delete(record);names.names.delete(record.alias);
    try{document.fonts.delete(record.face);}catch(_removedDocument){}
  };
  return {
    add(bytes,style,weight,variations) {
      if(closed||typeof FontFace!=='function'||typeof document==='undefined'||!document.fonts||typeof style!=='string'||typeof weight!=='string'||typeof variations!=='string')return null;
      let record=null;
      try {
        const alias='opck_private_'+(names.next++).toString(36);
        const source=bytes.buffer.slice(bytes.byteOffset,bytes.byteOffset+bytes.byteLength);
        const face=new FontFace(alias,source,{style,weight,variationSettings:variations});
        record={alias,face,active:true,ready:false};records.add(record);names.names.add(alias);
        // Buffer sources load asynchronously too. No URL/network fetch is used.
        face.loaded.then(()=>{
          if(closed||!record.active)return;
          try{document.fonts.add(face);record.ready=true;changed();}
          catch(_fontSetRefusal){remove(record);}
        },()=>remove(record));
        return record;
      }catch(_unsupportedFace){remove(record);return null;}
    },
    remove(record){const ready=Boolean(record?.ready);remove(record);if(ready&&!closed)changed();},
    alias(record){return record?.active&&record.ready?record.alias:null;},
    // Authored names can never address another viewer's private aliases.
    authoredToken(name,generic){return names.names.has(name.toLowerCase())?'':generic?name:quote(name);},
    token(name){return quote(name);},
    dispose(){closed=true;for(const record of records)remove(record);records.clear();},
  };
}
