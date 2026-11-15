// Checked single-draw lattice geometry. Existing u16 grids keep their triangle
// order; large grids expand vertices rather than wrapping native indices.
const clamp=value=>{const n=Number(value);return Number.isFinite(n)?Math.max(0,Math.min(1,n)):0;};
const MAX_PREPARED_BYTES=128*1024*1024;
export function opCkBuildMeshGradientData(x,y,w,h,rows,cols,colors) {
  rows=Math.trunc(Number(rows));cols=Math.trunc(Number(cols));
  const count=rows*cols,triangles=(rows-1)*(cols-1)*6;
  if(![x,y,w,h].every(Number.isFinite)||rows<2||cols<2
      ||!Number.isSafeInteger(count)||!Number.isSafeInteger(triangles)
      ||colors.length!==count*4||triangles>0x7fffffff)return null;
  const indexed=count<=65536,drawCount=indexed?count:triangles;
  const bytes=drawCount*24+(indexed?triangles*2:count*16)+(rows+cols)*4;
  if(!Number.isSafeInteger(bytes)||bytes>MAX_PREPARED_BYTES)return null;
  try {
    const xs=new Float32Array(cols),ys=new Float32Array(rows);
    for(let c=0;c<cols;c++)xs[c]=x+(c/(cols-1))*w;
    for(let r=0;r<rows;r++)ys[r]=y+(r/(rows-1))*h;
    const positions=new Float32Array(drawCount*2),rgba=new Float32Array(drawCount*4);
    if(indexed) {
      for(let r=0;r<rows;r++)for(let c=0;c<cols;c++) {
        const i=r*cols+c;positions[i*2]=xs[c];positions[i*2+1]=ys[r];
        for(let channel=0;channel<4;channel++)rgba[i*4+channel]=clamp(colors[i*4+channel]);
      }
      const indices=new Uint16Array(triangles);let at=0;
      for(let r=0;r<rows-1;r++)for(let c=0;c<cols-1;c++) {
        const tl=r*cols+c,tr=tl+1,bl=tl+cols,br=bl+1;
        indices[at++]=tl;indices[at++]=tr;indices[at++]=bl;
        indices[at++]=tr;indices[at++]=br;indices[at++]=bl;
      }
      return {positions,rgba,indices};
    }
    const source=new Float32Array(count*4);
    for(let i=0;i<source.length;i++)source[i]=clamp(colors[i]);
    let at=0;
    const append=i=>{
      positions[at*2]=xs[i%cols];positions[at*2+1]=ys[Math.floor(i/cols)];
      for(let c=0;c<4;c++)rgba[at*4+c]=source[i*4+c];at++;
    };
    for(let r=0;r<rows-1;r++)for(let c=0;c<cols-1;c++) {
      const tl=r*cols+c,tr=tl+1,bl=tl+cols,br=bl+1;
      append(tl);append(tr);append(bl);append(tr);append(br);append(bl);
    }
    return {positions,rgba,indices:null};
  }catch(_allocationError){return null;}
}
