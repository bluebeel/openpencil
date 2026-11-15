// Explicit affine image fields. Matrices use SVG column order in float64 until
// the same CanvasKit/Skia float32 backend boundary as ordinary image paints.
const mul=(a,b)=>[a[0]*b[0]+a[2]*b[1],a[1]*b[0]+a[3]*b[1],a[0]*b[2]+a[2]*b[3],a[1]*b[2]+a[3]*b[3],a[0]*b[4]+a[2]*b[5]+a[4],a[1]*b[4]+a[3]*b[5]+a[5]];
const inv=a=>{const d=a[0]*a[3]-a[1]*a[2];return d!==0&&Number.isFinite(d)?[a[3]/d,-a[1]/d,-a[2]/d,a[0]/d,(a[2]*a[5]-a[3]*a[4])/d,(a[1]*a[4]-a[0]*a[5])/d]:null;};
const pointXY=(m,x,y)=>[m[0]*x+m[2]*y+m[4],m[1]*x+m[3]*y+m[5]];
const point=(m,p)=>pointXY(m,p[0],p[1]);
const corners=m=>[pointXY(m,0,0),pointXY(m,1,0),pointXY(m,1,1),pointXY(m,0,1)];
function bounds(ps) {
  if (!ps.length) return [0,0,0,0];
  let left=Infinity,top=Infinity,right=-Infinity,bottom=-Infinity;
  for (const p of ps) {
    left=Math.min(left,p[0]);top=Math.min(top,p[1]);
    right=Math.max(right,p[0]);bottom=Math.max(bottom,p[1]);
  }
  return [left,top,right,bottom];
}
const values=m=>[m[0],m[2],m[1],m[3],m[4],m[5]];
function clip(ps,m,axes){for(let axis=0;axis<2;axis++)if(axes[axis])for(let edge=0;edge<2;edge++){
  if(!ps.length)return ps;const output=[],inside=v=>edge===0?v>=0:v<=1;let previous=ps[ps.length-1],pv=point(m,previous)[axis];
  for(const current of ps){const cv=point(m,current)[axis];if(inside(pv)!==inside(cv)){const t=(edge-pv)/(cv-pv);output.push([previous[0]+(current[0]-previous[0])*t,previous[1]+(current[1]-previous[1])*t]);}if(inside(cv))output.push(current);previous=current;pv=cv;}ps=output;
}return ps;}
function setup(space,g,iw,ih){
  if(g.length!==28||!g.every(Number.isFinite))throw new Error('invalid explicit image geometry');
  let placement=[g[0],g[1],g[2],g[3],g[4],g[5]],basis=[space[2],0,0,space[3],space[0],space[1]],pattern=g[8]!==0;
  let vw=g[19],vh=g[20];if(g[18]!==0){if(vw===0&&vh===0){vw=iw;vh=ih;}else if(vw===0){vw=vh*iw/ih;}else if(vh===0){vh=vw*ih/iw;}}else{vw=iw;vh=ih;}
  let fit=[1,0,0,1,0,0];if(g[18]!==0&&g[21]!==0){const s=g[21]===1?Math.min(vw/iw,vh/ih):Math.max(vw/iw,vh/ih),w=iw*s/vw,h=ih*s/vh;fit=[w,0,0,h,(1-w)*g[22],(1-h)*g[23]];}
  if(g[25]!==0){const d=[vw,0,0,vh,0,0];placement=g[25]===1?mul(placement,d):mul(inv(d),mul(placement,d));}
  const tile=pattern?mul(basis,[g[9],g[10],g[11],g[12],g[13],g[14]]):basis,viewport=mul(tile,placement),image=mul(viewport,fit),toTile=inv(tile),toImage=inv(image),toViewport=inv(viewport);
  if(!toTile||!toImage||!toViewport)throw new Error('singular explicit image geometry');
  let support=corners(mul(placement,fit));if(g[18]!==0&&g[24]!==0)support=clip(support,inv(placement),[true,true]);
  return {image,viewport,tile,toTile,toImage,toViewport,support:bounds(support),pattern,clipViewport:g[18]!==0&&g[24]!==0,clipTile:g[17]!==0};
}
function releaseHandle(handle){try{handle?.delete?.();}catch(_error){}}
export function drawExplicitImageField(CK,canvas,image,coverage,space,g,adjustments,opacity,radius,blend,nearest,sourceSource,patternSource,effects,adjustmentMatrix,blendFor){
  if(g[18]!==0&&(g[21]!==0||g[19]===0||g[20]===0)&&(!(g[26]>0)||!(g[27]>0)))throw new Error('SVG image intrinsic source is pending');
  const s=setup(space,g,g[26]>0?g[26]:image.width(),g[27]>0?g[27]:image.height()),iw=image.width(),ih=image.height();
  const tile=mode=>[CK.TileMode.Clamp,CK.TileMode.Clamp,CK.TileMode.Repeat,CK.TileMode.Mirror][mode];
  const m=s.image,local=[m[0]/iw,m[2]/ih,m[4],m[1]/iw,m[3]/ih,m[5],0,0,1];
  let shader=image.makeShaderOptions(tile(g[6]),tile(g[7]),nearest?CK.FilterMode.Nearest:CK.FilterMode.Linear,CK.MipmapMode.Linear??CK.MipmapMode.None,local);
  if(!shader)throw new Error('image field shader unavailable');
  const owned=[shader];let paint=null,colorFilter=null,saved=false;
  try{
    paint=new CK.Paint();canvas.save();saved=true;
    let draw=coverage;
    if(s.pattern){
      const source=effects.get(sourceSource),data=new Float32Array([...values(s.toImage),...values(s.toViewport),g[6]===0?1:0,g[7]===0?1:0,s.clipViewport?1:0]);
      shader=source?.effect.makeShaderWithChildren(data,[shader],null);if(!shader)throw new Error('image support effect unavailable');owned.push(shader);
      const pattern=effects.get(patternSource),pd=new Float32Array([...values(s.toTile),...values(s.tile),...s.support,g[15],g[16],s.clipTile?1:0]);
      shader=pattern?.effect.makeShaderWithChildren(pd,[shader],null);if(!shader)throw new Error('image tile effect unavailable');owned.push(shader);
    }else{
      if(g[18]!==0&&g[24]===0&&g[6]===0&&g[7]===0){let ps=corners(s.image);if(s.clipViewport)ps=clip(ps,s.toViewport,[true,true]);const b=bounds(ps);draw=[b[0],b[1],b[2]-b[0],b[3]-b[1]];}
      let ps=[[draw[0],draw[1]],[draw[0]+draw[2],draw[1]],[draw[0]+draw[2],draw[1]+draw[3]],[draw[0],draw[1]+draw[3]]];ps=clip(ps,s.toImage,[g[6]===0,g[7]===0]);if(s.clipViewport)ps=clip(ps,s.toViewport,[true,true]);
      if(ps.length<3)return;const path=new CK.Path();try{path.moveTo(...ps[0]);for(let index=1;index<ps.length;index++)path.lineTo(...ps[index]);path.close();canvas.clipPath(path,CK.ClipOp.Intersect,true);}finally{path.delete();}
      draw=[draw[0]-1,draw[1]-1,draw[2]+2,draw[3]+2];
    }
    if(radius>0){const[x,y,w,h]=coverage;canvas.clipRRect(CK.RRectXY(CK.LTRBRect(x,y,x+w,y+h),radius,radius),CK.ClipOp.Intersect,true);}
    paint.setShader(shader);paint.setAntiAlias(true);paint.setAlphaf(Math.max(0,Math.min(1,opacity)));paint.setBlendMode(blendFor(blend));const cm=adjustmentMatrix(adjustments);if(cm){colorFilter=CK.ColorFilter.MakeMatrix(cm);paint.setColorFilter(colorFilter);}
    const[x,y,w,h]=draw;canvas.drawRect(CK.LTRBRect(x,y,x+w,y+h),paint);
  }finally{
    // Resource cleanup also owns failures during Paint creation/canvas.save.
    // A restore failure must not skip disposal of the already-owned handles.
    try { if(saved)canvas.restore(); }
    finally {
      releaseHandle(paint);releaseHandle(colorFilter);
      for(let i=owned.length-1;i>=0;i--)releaseHandle(owned[i]);
    }
  }
}
