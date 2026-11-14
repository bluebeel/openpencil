//! Typed imported gradients use the existing portable runtime-shader seam.
//! Legacy angle/center-only gradients never enter this module.

use crate::payload::{ShaderPayload, ShaderUniformPayload};
use jian_ops_schema::style::{
    GradientAlphaInterpolation, GradientInterpolation, GradientPaint, GradientShape,
    GradientSpread, PenFill,
};
use std::fmt::Write;

pub(crate) fn paint(fill: &PenFill) -> Option<GradientPaint> {
    let (geometry, stops, opacity) = match fill {
        PenFill::LinearGradient(body) => (body.geometry.as_deref()?, &body.stops, body.opacity),
        PenFill::RadialGradient(body) => (body.geometry.as_deref()?, &body.stops, body.opacity),
        _ => return None,
    };
    Some(GradientPaint {
        geometry: geometry.clone(),
        stops: stops.clone(),
        opacity: opacity.unwrap_or(1.0).clamp(0.0, 1.0),
    })
}

pub(crate) fn shader(paint: GradientPaint) -> Option<ShaderPayload> {
    if paint.stops.is_empty() {
        return None;
    }
    let inverse = inverse(paint.geometry.transform);
    let mut uniforms = vec![uniform("size", vec![1.0, 1.0])];
    let mut source =
        String::from("uniform float2 size;\nuniform float3 row0;\nuniform float3 row1;\n");
    let inv = inverse.unwrap_or([0.0; 6]);
    uniforms.push(uniform(
        "row0",
        vec![inv[0] as f32, inv[2] as f32, inv[4] as f32],
    ));
    uniforms.push(uniform(
        "row1",
        vec![inv[1] as f32, inv[3] as f32, inv[5] as f32],
    ));
    let mut last = 0.0_f64;
    let mut fallback = [0.0; 4];
    for (i, stop) in paint.stops.iter().enumerate() {
        let color = crate::style_payload::parse_color(&stop.color).unwrap_or([0.0, 0.0, 0.0, 1.0]);
        if i == 0 {
            fallback = color;
        }
        let offset = if stop.offset.is_finite() {
            stop.offset.clamp(0.0, 1.0).max(last)
        } else {
            last
        };
        last = offset;
        writeln!(source, "uniform float o{i}; uniform float4 c{i};").expect("String write");
        uniforms.push(uniform(&format!("o{i}"), vec![offset as f32]));
        uniforms.push(uniform(&format!("c{i}"), color.to_vec()));
    }
    source.push_str("float lin(float c) { return c <= 0.04045 ? c/12.92 : pow((c+0.055)/1.055,2.4); }\nfloat enc(float c) { return c <= 0.0031308 ? c*12.92 : 1.055*pow(max(c,0.0),1.0/2.4)-0.055; }\n");
    // Geometry does not choose interpolation policy. SVG explicitly uses
    // straight RGB/alpha; Figma retains its existing premultiplied policy.
    // Both branches return premultiplied output to Skia/CanvasKit.
    // https://www.w3.org/TR/SVG2/pservers.html#StopColorProperty
    source.push_str("float4 working(float4 c) {\n");
    if paint.geometry.interpolation == GradientInterpolation::LinearRgb {
        source.push_str("c.rgb=float3(lin(c.r),lin(c.g),lin(c.b));\n");
    }
    if paint.geometry.alpha_interpolation == GradientAlphaInterpolation::Premultiplied {
        source.push_str("c.rgb*=c.a;\n");
    }
    source.push_str("return c; }\nfloat4 outputColor(float4 c) {\n");
    if paint.geometry.interpolation == GradientInterpolation::LinearRgb {
        if paint.geometry.alpha_interpolation == GradientAlphaInterpolation::Premultiplied {
            source.push_str("if(c.a>0.0) c.rgb/=c.a;\n");
        }
        source.push_str("c.rgb=float3(enc(c.r),enc(c.g),enc(c.b));\n");
        if paint.geometry.alpha_interpolation == GradientAlphaInterpolation::Premultiplied {
            source.push_str("c.rgb*=c.a;\n");
        }
    }
    if paint.geometry.alpha_interpolation == GradientAlphaInterpolation::Straight {
        source.push_str("c.rgb*=c.a;\n");
    }
    source.push_str("return c; }\nfloat4 sampleColor(float4 a,float4 b,float t) { return outputColor(mix(working(a),working(b),t)); }\n");
    source.push_str("float4 stopColor(float t) {\n");
    writeln!(source, "if(t<o0) return sampleColor(c0,c0,0.0);").expect("String write");
    for i in 1..paint.stops.len() {
        let prev = i - 1;
        writeln!(source, "if(t<o{i}) return sampleColor(c{prev},c{i},clamp((t-o{prev})/(o{i}-o{prev}),0.0,1.0));").expect("String write");
    }
    writeln!(
        source,
        "return sampleColor(c{0},c{0},0.0); }}",
        paint.stops.len() - 1
    )
    .expect("String write");
    source.push_str("float4 averageColor() { float4 c=working(c0)*o0;\n");
    for i in 1..paint.stops.len() {
        let prev = i - 1;
        writeln!(
            source,
            "c+=(working(c{prev})+working(c{i}))*0.5*(o{i}-o{prev});"
        )
        .expect("String write");
    }
    writeln!(
        source,
        "c+=working(c{0})*(1.0-o{0}); return outputColor(c); }}",
        paint.stops.len() - 1
    )
    .expect("String write");
    let parameter = match &paint.geometry.shape {
        GradientShape::Linear { start, end } => {
            source.push_str("uniform float4 line;\nfloat2 parameter(float2 p) { float2 v=line.zw-line.xy; float d=dot(v,v); return float2(d>0.0 ? dot(p-line.xy,v)/d : 1.0,1.0); }\n");
            uniforms.push(uniform(
                "line",
                vec![
                    start[0] as f32,
                    start[1] as f32,
                    end[0] as f32,
                    end[1] as f32,
                ],
            ));
            "parameter(p)"
        }
        GradientShape::Radial {
            center,
            radius,
            focal,
            focal_radius,
        } => {
            let radius = radius.max(0.0);
            let fr = focal_radius.max(0.0);
            let focal = *focal;
            uniforms.push(uniform(
                "centers",
                vec![
                    focal[0] as f32,
                    focal[1] as f32,
                    center[0] as f32,
                    center[1] as f32,
                ],
            ));
            uniforms.push(uniform("radii", vec![fr as f32, radius as f32]));
            source.push_str("uniform float4 centers; uniform float2 radii;\nfloat2 parameter(float2 p) { float2 dc=centers.zw-centers.xy; float2 v=p-centers.xy; float dr=radii.y-radii.x; float a=dot(dc,dc)-dr*dr; float b=-2.0*(dot(v,dc)+radii.x*dr); float c=dot(v,v)-radii.x*radii.x; if(dot(dc,dc)==0.0 && dr==0.0) return float2(0.0,0.0); if(a==0.0) { if(b==0.0) return float2(1.0,2.0); float t=-c/b; return radii.x+t*dr>=0.0 ? float2(t,1.0) : float2(1.0,2.0); } float disc=b*b-4.0*a*c; if(disc<0.0) return float2(0.0,0.0); float root=sqrt(disc); float t0=(-b-root)/(2.0*a); float t1=(-b+root)/(2.0*a); bool valid0=radii.x+t0*dr>=0.0; bool valid1=radii.x+t1*dr>=0.0; if(!valid0 && !valid1) return float2(0.0,0.0); return float2(valid0 && valid1 ? max(t0,t1) : (valid0 ? t0 : t1),1.0); }\n");
            "parameter(p)"
        }
    };
    source.push_str("half4 main(float2 coord) { float2 uv=coord/size; float3 q=float3(uv,1.0); float2 p=float2(dot(row0,q),dot(row1,q));\n");
    if inverse.is_none() {
        source.push_str("return half4(0.0); }\n");
    } else {
        writeln!(
            source,
            "float2 gradientSample={parameter}; if(gradientSample.y==0.0) return half4(0.0);"
        )
        .expect("String write");
        match paint.geometry.spread {
            GradientSpread::Pad => source.push_str("float t=gradientSample.y==2.0 ? 1.0 : gradientSample.x;\n"),
            GradientSpread::Repeat | GradientSpread::Reflect => source
                .push_str("if(gradientSample.y==2.0) return half4(averageColor()); float t=gradientSample.x;\n"),
        }
        match paint.geometry.spread {
            GradientSpread::Pad => source.push_str("t=clamp(t,0.0,1.0);\n"),
            GradientSpread::Repeat => source.push_str("t=t-floor(t);\n"),
            GradientSpread::Reflect => source.push_str("t=1.0-abs((t-2.0*floor(t*0.5))-1.0);\n"),
        }
        source.push_str("return half4(stopColor(t)); }\n");
    }
    if uniforms
        .iter()
        .any(|u| u.values.iter().any(|v| !v.is_finite()))
    {
        return None;
    }
    let opacity = paint.opacity as f32;
    Some(ShaderPayload {
        gradient: Some(Box::new(paint)),
        sksl: source,
        uniforms,
        opacity,
        fallback,
    })
}

fn uniform(name: &str, values: Vec<f32>) -> ShaderUniformPayload {
    ShaderUniformPayload {
        name: name.to_string(),
        values,
    }
}

fn inverse([a, b, c, d, e, f]: [f64; 6]) -> Option<[f64; 6]> {
    let det = a * d - b * c;
    if !det.is_finite() || det == 0.0 {
        return None;
    }
    let out = [
        d / det,
        -b / det,
        -c / det,
        a / det,
        (c * f - d * e) / det,
        (b * e - a * f) / det,
    ];
    out.iter().all(|v| v.is_finite()).then_some(out)
}
