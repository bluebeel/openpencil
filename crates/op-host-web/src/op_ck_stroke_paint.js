// Exact curve stroke silhouettes and coordinate-stable image paint fields.
export function releasePaintHandle(handle){try{handle?.delete?.();}catch(_cleanupError){}}
function splitContours(CK,path) {
  const cmds=path.toCmds(),closed=[],open=[];let contour=[];
  const flush=target=>{for(const value of contour)target.push(value);contour=[];};
  for(let i=0;i<cmds.length;){
    const verb=cmds[i],n=[3,3,5,6,7,1][verb];
    if(!n||i+n>cmds.length)throw new Error('invalid CanvasKit path verb');
    if(verb===0&&contour.length)flush(open);
    for(let at=i;at<i+n;at++)contour.push(cmds[at]);
    i+=n;if(verb===5)flush(closed);
  }
  if(contour.length)flush(open);
  let closedPath=null;
  try {
    closedPath=closed.length?CK.Path.MakeFromCmds(closed):null;
    return [closedPath,open.length?CK.Path.MakeFromCmds(open):null];
  }catch(error){releasePaintHandle(closedPath);throw error;}
}
function rrect(CK,r,radii){
  const values=new Float32Array(12);values[0]=r[0];values[1]=r[1];values[2]=r[0]+r[2];values[3]=r[1]+r[3];
  for(let i=0;i<4;i++){values[4+i*2]=radii[i][0];values[5+i*2]=radii[i][1];}
  return values;
}
export function strokePaintGeometry(CK,canvas,cached,fit,kind,data,v,evenOdd,width,sides,placement,cap,join,dashes,phase,color) {
  if(!(width>0)||!Number.isFinite(width))return;
  let path=null,paint=null,dash=null,parts=[],ring=null;
  try {
  if(kind===2||kind===4){path=cached(data);if(path&&kind===2)fit(path,...v);}
  else if(kind===3)path=CK.Path.MakeFromCmds(v);
  else {path=new CK.Path();if(kind===1)path.addOval(CK.LTRBRect(v[0],v[1],v[0]+v[2],v[1]+v[3]));else path.addRRect(rrect(CK,v.slice(0,4),Array.from(v.slice(4,8),r=>[Math.max(0,r),Math.max(0,r)])));}
  if(!path)return;
  if(evenOdd)path.setFillType(CK.FillType.EvenOdd);
  paint=new CK.Paint();paint.setAntiAlias(true);paint.setColor(CK.Color4f(...color));paint.setStyle(CK.PaintStyle.Stroke);paint.setStrokeWidth(width);paint.setStrokeMiter(4);
  paint.setStrokeCap([CK.StrokeCap.Butt,CK.StrokeCap.Round,CK.StrokeCap.Square][cap]);paint.setStrokeJoin([CK.StrokeJoin.Miter,CK.StrokeJoin.Round,CK.StrokeJoin.Bevel][join]);
  dash=dashes.length?CK.PathEffect.MakeDash(Array.from(dashes),phase):null;if(dash)paint.setPathEffect(dash);
    if(kind===0&&sides.length===4&&sides.some(s=>s!==sides[0])) {
      const [top,right,bottom,left]=sides.map(s=>Math.max(0,s)),outside=[0,.5,1][placement],inside=[1,.5,0][placement];
      const corner=(base,inset,f)=>base===0&&f>0&&join!==1?0:base+inset*f;
      const expanded=f=>[[corner(v[4],left,f),corner(v[4],top,f)],[corner(v[5],right,f),corner(v[5],top,f)],[corner(v[6],right,f),corner(v[6],bottom,f)],[corner(v[7],left,f),corner(v[7],bottom,f)]].map(r=>r.map(n=>Math.max(0,n)));
      ring=new CK.Path();ring.setFillType(CK.FillType.EvenOdd);
      ring.addRRect(rrect(CK,[v[0]-left*outside,v[1]-top*outside,v[2]+(left+right)*outside,v[3]+(top+bottom)*outside],expanded(outside)));
      const inner=[v[0]+left*inside,v[1]+top*inside,v[2]-(left+right)*inside,v[3]-(top+bottom)*inside];
      if(inner[2]>0&&inner[3]>0)ring.addRRect(rrect(CK,inner,expanded(-inside)));
      if(!dash){paint.setStyle(CK.PaintStyle.Fill);canvas.drawPath(ring,paint);}
      else {canvas.save();try{canvas.clipPath(ring,CK.ClipOp.Intersect,true);paint.setStrokeWidth(Math.max(width,...sides)*2);canvas.drawPath(path,paint);}finally{canvas.restore();}}
    }else if(placement===1){canvas.drawPath(path,paint);}
    else {
      parts=splitContours(CK,path);
      if(parts[0]){parts[0].setFillType(path.getFillType());canvas.save();try{canvas.clipPath(parts[0],placement===0?CK.ClipOp.Intersect:CK.ClipOp.Difference,true);paint.setStrokeWidth(width*2);canvas.drawPath(parts[0],paint);}finally{canvas.restore();}}
      if(parts[1]){paint.setStrokeWidth(width);canvas.drawPath(parts[1],paint);}
    }
  } finally {for(const p of parts)releasePaintHandle(p);releasePaintHandle(ring);releasePaintHandle(dash);releasePaintHandle(paint);releasePaintHandle(path);}
}
export function imagePaintField(CK,canvas,image,coverage,space,mode,transform,adjustments,opacity,blendMode,originalWidth,originalHeight,tileScale,figmaMatrix,adjustmentMatrix,blendFor) {
  const[x,y,w,h]=space,iw=image.width(),ih=image.height();
  let matrix=mode===3?null:figmaMatrix(x,y,w,h,iw,ih,transform);
  if(!matrix){let dw=w,dh=h;
    if(mode===1||mode===0||mode===2){const s=mode===1?Math.min(w/iw,h/ih):Math.max(w/iw,h/ih);dw=iw*s;dh=ih*s;}
    if(mode===3){const scale=Number.isFinite(tileScale)&&tileScale>0?tileScale:1;dw=(originalWidth>0?originalWidth:iw)*scale;dh=(originalHeight>0?originalHeight:ih)*scale;}
    matrix=[dw/iw,0,x+(w-dw)/2,0,dh/ih,y+(h-dh)/2,0,0,1];
  }
  const tile=mode===3?CK.TileMode.Repeat:mode===1?CK.TileMode.Decal:CK.TileMode.Clamp;
  const shader=image.makeShaderOptions(tile,tile,CK.FilterMode.Linear,CK.MipmapMode.None,matrix);if(!shader)return;
  let paint=null,filter=null;
  try {paint=new CK.Paint();const m=adjustmentMatrix(adjustments);filter=m?CK.ColorFilter.MakeMatrix(m):null;paint.setShader(shader);paint.setAlphaf(Math.max(0,Math.min(1,opacity)));paint.setBlendMode(blendFor(blendMode));if(filter)paint.setColorFilter(filter);const[a,b,c,d]=coverage;canvas.drawRect(CK.LTRBRect(a,b,a+c,b+d),paint);}
  finally {releasePaintHandle(paint);releasePaintHandle(shader);releasePaintHandle(filter);}
}

// Extend the same piecewise-linear mesh as a clamped paint field. Row strips
// cap temporary vertex/index storage, including very wide valid meshes.
export function drawMeshPaintField(CK,canvas,coverage,space,rows,cols,colors,opacity) {
  rows=Math.trunc(rows);cols=Math.trunc(cols);
  const vertices=rows*cols;
  if(rows<2||cols<2||!Number.isSafeInteger(vertices)||vertices>4*1024*1024||colors.length!==vertices*4)return;
  let paint=null;
  const axis=(i,n,start,size,edgeStart,edgeSize)=>i===0?edgeStart:i===n+1?edgeStart+edgeSize:start+(i-1)*size/(n-1);
  try {
    paint=new CK.Paint();paint.setColor(CK.Color4f(1,1,1,Math.max(0,Math.min(1,opacity))));
    const maxCount=Math.min(8192,cols+2),positions=new Float32Array(maxCount*4),rgba=new Float32Array(maxCount*8),indices=new Uint16Array((maxCount-1)*6);
    for(let x=0;x<maxCount-1;x++){
      const at=x*6;indices[at]=x;indices[at+1]=x+1;indices[at+2]=maxCount+x;
      indices[at+3]=x+1;indices[at+4]=maxCount+x+1;indices[at+5]=maxCount+x;
    }
    const full={positions,rgba,indices};let tail=null;
    for(let row=0;row<rows+1;row++)for(let first=0;first<cols+1;first+=8191){
      const count=Math.min(8192,cols+2-first);
      for(let ry=0;ry<2;ry++)for(let cx=0;cx<count;cx++){
        const vertex=ry*count+cx,col=first+cx,r=row+ry;
        positions[vertex*2]=axis(col,cols,space[0],space[2],coverage[0],coverage[2]);positions[vertex*2+1]=axis(r,rows,space[1],space[3],coverage[1],coverage[3]);
        const source=(Math.max(0,Math.min(rows-1,r-1))*cols+Math.max(0,Math.min(cols-1,col-1)))*4;
        for(let channel=0;channel<4;channel++)rgba[vertex*4+channel]=Math.max(0,Math.min(1,colors[source+channel]));
      }
      // MakeVertices copies arrays into its native builder before returning.
      // Tail strips have a different row stride; build only their small index
      // projection rather than reusing a wrong maxCount offset.
      if(count!==maxCount&&!tail){
        const small=new Uint16Array((count-1)*6);
        for(let x=0;x<count-1;x++){const at=x*6;small[at]=x;small[at+1]=x+1;small[at+2]=count+x;small[at+3]=x+1;small[at+4]=count+x+1;small[at+5]=count+x;}
        tail={positions:positions.subarray(0,count*4),rgba:rgba.subarray(0,count*8),indices:small};
      }
      const strip=count===maxCount?full:tail;
      const mesh=CK.MakeVertices(CK.VertexMode.Triangles,strip.positions,null,strip.rgba,strip.indices,true);
      if(mesh)try{canvas.drawVertices(mesh,CK.BlendMode.Modulate,paint);}finally{releasePaintHandle(mesh);}
    }
  }finally{releasePaintHandle(paint);}
}
